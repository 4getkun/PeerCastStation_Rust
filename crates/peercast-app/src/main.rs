use std::path::PathBuf;
use std::time::Duration;

use peercast_app::{
    wait_for_os_shutdown_signal, AppCliArgs, AppConfig, AppLifecycleManager, AppMetrics,
    BoundedLogBuffer, ConfigManager, LogLevel, ShutdownReason,
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
        println!("PeerCastStation (Rust Edition) v{}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    // Determine configuration path
    let config_path = cli
        .config_path
        .clone()
        .unwrap_or_else(|| PathBuf::from("peercast.json"));

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
                eprintln!("Failed to save initial configuration to {}: {}", config_path.display(), e);
                std::process::exit(1);
            }
            c
        }
        Err(e) => {
            eprintln!("Failed to load configuration from {}: {}", config_path.display(), e);
            std::process::exit(1);
        }
    };

    if let Err(e) = config.validate() {
        eprintln!("Configuration validation error: {}", e);
        std::process::exit(1);
    }

    let log_buffer = BoundedLogBuffer::new(1000);
    let metrics = AppMetrics::new();
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

    // Monitor for shutdown signals (Windows Ctrl+C/Break/Close, Unix SIGINT/SIGTERM)
    let mut shutdown_rx = lifecycle.subscribe_shutdown();
    tokio::select! {
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
    if let Err(surviving_tasks) = lifecycle.complete_shutdown(timeout).await {
        log_buffer.log(
            LogLevel::Warn,
            "lifecycle",
            &format!("Shutdown drain timed out; {} tasks survived", surviving_tasks),
        );
        std::process::exit(2);
    }

    log_buffer.log(LogLevel::Info, "app", "PeerCastStation stopped cleanly");
    let snap = metrics.snapshot(lifecycle.task_tracker().active_tasks());
    println!(
        "Shutdown complete. Uptime: {}s, Active tasks: {}",
        snap.uptime_secs, snap.active_tasks
    );

    Ok(())
}
