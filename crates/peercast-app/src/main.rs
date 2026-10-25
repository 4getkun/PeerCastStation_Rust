use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use peercast_app::config::DEFAULT_CONFIG_FILENAME;
use peercast_app::{
    wait_for_os_shutdown_signal, AppCliArgs, AppConfig, AppLifecycleManager, AppMetrics,
    BoundedLogBuffer, ConfigManager, HttpServer, LogLevel, ShutdownReason, APPLICATION_NAME,
    APPLICATION_VERSION,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let cli = match AppCliArgs::parse_from(&args) {
        Ok(c) => c,
        Err(err) => {
            eprintln!("Error: {}", err);
            eprintln!("{}", AppCliArgs::help_text());
            std::process::exit(1);
        }
    };

    if cli.help {
        println!("{}", AppCliArgs::help_text());
        return Ok(());
    }

    if cli.version {
        println!("{} v{}", APPLICATION_NAME, APPLICATION_VERSION);
        return Ok(());
    }

    // Determine configuration path
    let config_path = cli
        .config_path
        .clone()
        .unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG_FILENAME));

    let config_mgr = ConfigManager::new(&config_path);
    let config = match config_mgr.load() {
        Ok(mut c) => {
            cli.apply_to_config(&mut c);
            c
        }
        Err(peercast_app::ConfigPersistenceError::NotFound(_)) => {
            let mut c = AppConfig::default();
            cli.apply_to_config(&mut c);
            if let Err(e) = c.validate() {
                eprintln!("Configuration validation error: {}", e);
                std::process::exit(1);
            }
            if let Err(e) = config_mgr.save(&c) {
                eprintln!(
                    "Failed to save initial configuration to {}: {}",
                    config_path.display(),
                    e
                );
                std::process::exit(1);
            }
            c
        }
        Err(e) => {
            eprintln!(
                "Failed to load configuration from {}: {}",
                config_path.display(),
                e
            );
            std::process::exit(1);
        }
    };

    if let Err(e) = config.validate() {
        eprintln!("Configuration validation error: {}", e);
        std::process::exit(1);
    }

    let mut secrets = Vec::new();
    if let Some(token) = config.server.auth_token.as_deref() {
        secrets.push(token);
    }
    for channel in &config.runtime.channels {
        if let peercast_app::runtime_config::ChannelSource::HttpPush {
            auth_token: Some(token),
            ..
        } = &channel.source
        {
            secrets.push(token.as_str());
        }
    }
    let log_buffer = Arc::new(BoundedLogBuffer::from_config(&config.logging, &secrets)?);
    let metrics = Arc::new(AppMetrics::new());
    let lifecycle = AppLifecycleManager::new();

    log_buffer.log(
        LogLevel::Info,
        "app",
        &format!(
            "Starting PeerCastStation on {}:{} (headless={})",
            config.server.bind_address, config.server.port, config.lifecycle.headless
        ),
    );

    // If --kill was requested, simulate clean inter-process stop
    if cli.kill {
        println!("Kill requested; signaling daemon to stop...");
        return Ok(());
    }

    lifecycle.mark_running().await;

    // Initialize RPC state with server configuration
    let mut rpc_state = peercast_rpc::server::RpcState {
        settings: config.to_servent_settings(),
        ..Default::default()
    };
    rpc_state.status.local_relay_end_point =
        Some((config.server.bind_address.clone(), config.server.port));
    rpc_state.status.local_direct_end_point =
        Some((config.server.bind_address.clone(), config.server.port));

    let server = match HttpServer::new(
        &config,
        rpc_state,
        Arc::clone(&metrics),
        Arc::clone(&log_buffer),
        lifecycle.task_tracker().clone(),
    ) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "Failed to initialize HTTP server on {}:{}: {}",
                config.server.bind_address, config.server.port, e
            );
            std::process::exit(1);
        }
    };

    println!("{} v{} started", APPLICATION_NAME, APPLICATION_VERSION);
    println!(
        "Web UI listening on http://{}:{}/",
        config.server.bind_address, config.server.port
    );
    println!("Press Ctrl+C to stop.");

    let server_shutdown_rx = lifecycle.subscribe_shutdown();
    let mut server_handle = tokio::spawn(async move {
        server
            .run(server_shutdown_rx)
            .await
            .map_err(|error| error.to_string())
    });
    let mut server_finished = false;
    let mut server_failure = None;

    // Monitor for shutdown signals (Windows Ctrl+C/Break/Close, Unix SIGINT/SIGTERM)
    let mut shutdown_rx = lifecycle.subscribe_shutdown();
    tokio::select! {
        result = &mut server_handle => {
            server_finished = true;
            let error = match result {
                Ok(Err(error)) => error,
                Err(error) => error.to_string(),
                Ok(Ok(())) => "HTTP server stopped unexpectedly".to_string(),
            };
            log_buffer.log(LogLevel::Error, "http", &error);
            lifecycle.request_shutdown(ShutdownReason::InternalError(error.clone())).await;
            server_failure = Some(error);
        }
        sig_name = wait_for_os_shutdown_signal() => {
            log_buffer.log(LogLevel::Info, "lifecycle", &format!("Received shutdown signal: {}", sig_name));
            lifecycle.request_shutdown(ShutdownReason::OsSignal(sig_name.to_string())).await;
        }
        reason = shutdown_rx.recv() => {
            if let Ok(r) = reason {
                log_buffer.log(LogLevel::Info, "lifecycle", &format!("Internal shutdown triggered: {:?}", r));
            }
        }
    }

    // Gracefully complete shutdown
    let timeout = Duration::from_secs(config.lifecycle.shutdown_timeout_secs);
    if !server_finished {
        if let Ok(Err(error)) = server_handle.await {
            server_failure = Some(error);
        }
    }
    if let Err(surviving_tasks) = lifecycle.complete_shutdown(timeout).await {
        log_buffer.log(
            LogLevel::Warn,
            "lifecycle",
            &format!(
                "Shutdown drain timed out; {} tasks survived",
                surviving_tasks
            ),
        );
        std::process::exit(2);
    }

    log_buffer.log(LogLevel::Info, "app", "PeerCastStation stopped cleanly");
    let snap = metrics.snapshot(lifecycle.task_tracker().active_tasks());
    println!(
        "Shutdown complete. Uptime: {}s, Active tasks: {}",
        snap.uptime_secs, snap.active_tasks
    );

    if let Some(error) = server_failure {
        return Err(std::io::Error::other(error).into());
    }
    Ok(())
}
