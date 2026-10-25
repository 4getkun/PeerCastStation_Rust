//! Finite, renewable TCP mappings for the actual IPv4 listener.
use super::*;
use crate::runtime_config::PortMappingSettings;
use igd_next::aio::{
    tokio::{search_gateway, Tokio},
    Gateway, Provider,
};
use igd_next::{
    GetGenericPortMappingEntryError, PortMappingEntry, PortMappingProtocol, SearchOptions,
};
use tokio::net::UdpSocket;

type Router = Gateway<Tokio>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortMappingStatus {
    Disabled,
    Discovering,
    Mapped {
        internal: SocketAddr,
        external: SocketAddr,
    },
    Retrying {
        error: String,
        delay_secs: u64,
    },
    Stopped,
}

pub(super) async fn run(runtime: Arc<AppRuntime>) {
    let settings = &runtime.config.runtime.port_mapping;
    let mut shutdown = runtime.shutdown_tx.subscribe();
    let description = format!("PeerCastStation-{}", runtime.session_id.to_hex_lower());
    let mut retry = settings.retry_base_secs;
    loop {
        if *shutdown.borrow() {
            break;
        }
        *runtime.port_mapping.lock().await = PortMappingStatus::Discovering;
        let discovered = tokio::select! {
            result = discover(settings, &runtime.config.server.bind_address) => result,
            _ = shutdown.changed() => break,
        };
        let error = match discovered {
            Ok(gateway) => {
                let result = match route_local_addr(
                    &gateway,
                    &runtime.config.server.bind_address,
                    runtime.config.server.port,
                )
                .await
                {
                    Ok(local) => {
                        manage_mapping_session(&gateway, local, &description, settings, &runtime)
                            .await
                    }
                    Err(error) => Err(error),
                };
                if *shutdown.borrow() {
                    break;
                }
                if matches!(
                    *runtime.port_mapping.lock().await,
                    PortMappingStatus::Mapped { .. }
                ) {
                    retry = settings.retry_base_secs;
                }
                result
                    .err()
                    .unwrap_or_else(|| io::Error::other("mapping worker stopped"))
            }
            Err(error) => error,
        };
        runtime
            .log
            .log(LogLevel::Warn, "port-mapping", &error.to_string());
        *runtime.port_mapping.lock().await = PortMappingStatus::Retrying {
            error: error.to_string(),
            delay_secs: retry,
        };
        tokio::select! { _ = tokio::time::sleep(Duration::from_secs(retry)) => {}, _ = shutdown.changed() => break }
        retry = retry.saturating_mul(2).min(settings.retry_max_secs);
    }
    *runtime.port_mapping.lock().await = PortMappingStatus::Stopped;
}

async fn manage_mapping_session(
    gateway: &Router,
    local: SocketAddr,
    description: &str,
    settings: &PortMappingSettings,
    runtime: &AppRuntime,
) -> io::Result<()> {
    let mut shutdown = runtime.shutdown_tx.subscribe();
    if *shutdown.borrow() {
        return Ok(());
    }
    let shutdown_budget =
        Duration::from_secs(runtime.config.lifecycle.shutdown_timeout_secs).mul_f32(0.8);
    let operation = manage_lease(gateway, local, description, settings, runtime);
    tokio::pin!(operation);
    let mut shutdown_deadline = None;
    let result = tokio::select! {
        result = &mut operation => result,
        _ = shutdown.changed() => {
            let deadline = tokio::time::Instant::now() + shutdown_budget;
            shutdown_deadline = Some(deadline);
            // Dropping a sent SOAP request cannot cancel the router's mutation.
            // Let it settle before looking up the mapping for removal.
            tokio::time::timeout_at(deadline, &mut operation)
                .await
                .unwrap_or_else(|_| Err(io::Error::new(io::ErrorKind::TimedOut,
                    "mapping request did not settle before shutdown deadline")))
        },
    };
    // The unique session description prevents deleting another application's mapping.
    let deadline =
        shutdown_deadline.unwrap_or_else(|| tokio::time::Instant::now() + shutdown_budget);
    match tokio::time::timeout_at(
        deadline,
        remove_owned_mapping(gateway, local, description, settings),
    )
    .await
    {
        Ok(Ok(())) => {}
        result => runtime.log.log(
            LogLevel::Warn,
            "port-mapping",
            &format!("Mapping cleanup could not be confirmed: {result:?}"),
        ),
    }
    if *shutdown.borrow() {
        if let Err(error) = result {
            if error.kind() != io::ErrorKind::Interrupted {
                runtime.log.log(LogLevel::Warn, "port-mapping", &format!(
                    "Mapping request ended during shutdown: {error}; finite lease remains the fallback"
                ));
            }
        }
        Ok(())
    } else {
        result
    }
}

async fn discover(settings: &PortMappingSettings, bind: &str) -> io::Result<Router> {
    let timeout = Duration::from_millis(settings.discovery_timeout_ms);
    let mut options = SearchOptions::default();
    // Keep SSDP on the listener's interface instead of an unrelated VPN/NIC.
    options.bind_addr = SocketAddr::new(bind.parse().map_err(io::Error::other)?, 0);
    options.timeout = Some(timeout);
    options.single_search_timeout = Some(timeout);
    let gateway = tokio::time::timeout(timeout, search_gateway(options))
        .await
        .map_err(io::Error::other)?
        .map_err(io::Error::other)?;
    match gateway.addr.ip() {
        IpAddr::V4(ip) if ip.is_private() || ip.is_link_local() => Ok(gateway),
        _ => Err(io::Error::other(
            "UPnP gateway must be on the local IPv4 network",
        )),
    }
}

async fn route_local_addr(gateway: &Router, bind: &str, port: u16) -> io::Result<SocketAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0").await?;
    // UDP connect selects the Windows route/NIC without sending a packet.
    socket.connect(gateway.addr).await?;
    let selected = socket.local_addr()?.ip();
    let configured: IpAddr = bind.parse().map_err(io::Error::other)?;
    if !configured.is_unspecified() && configured != selected {
        return Err(io::Error::other(
            "discovered gateway does not serve the configured listener interface",
        ));
    }
    if selected.is_loopback() || selected.is_unspecified() {
        return Err(io::Error::other("no LAN interface selected"));
    }
    Ok(SocketAddr::new(selected, port))
}

async fn mapping_at(
    gateway: &Router,
    port: u16,
    settings: &PortMappingSettings,
) -> io::Result<Option<PortMappingEntry>> {
    if gateway
        .control_schema
        .contains_key("GetSpecificPortMappingEntry")
    {
        return tokio::time::timeout(
            Duration::from_millis(settings.request_timeout_ms),
            specific_mapping(gateway, port),
        )
        .await
        .map_err(io::Error::other)?;
    }
    let operation = async {
        for index in 0..settings.max_mapping_entries {
            match gateway.get_generic_port_mapping_entry(index).await {
                Ok(entry)
                    if entry.protocol == PortMappingProtocol::TCP
                        && entry.external_port == port =>
                {
                    return Ok(Some(entry))
                }
                Ok(_) => {}
                Err(GetGenericPortMappingEntryError::SpecifiedArrayIndexInvalid) => {
                    return Ok(None)
                }
                Err(error) => return Err(io::Error::other(error)),
            }
        }
        Err(io::Error::other(
            "UPnP mapping table exceeds configured scan limit",
        ))
    };
    tokio::time::timeout(
        Duration::from_millis(settings.request_timeout_ms),
        operation,
    )
    .await
    .map_err(io::Error::other)?
}

async fn specific_mapping(gateway: &Router, port: u16) -> io::Result<Option<PortMappingEntry>> {
    let service = &gateway.service_type;
    let action = format!("\"{service}#GetSpecificPortMappingEntry\"");
    let body = format!(
        r#"<?xml version="1.0"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body><u:GetSpecificPortMappingEntry xmlns:u="{service}"><NewRemoteHost></NewRemoteHost><NewExternalPort>{port}</NewExternalPort><NewProtocol>TCP</NewProtocol></u:GetSpecificPortMappingEntry></s:Body></s:Envelope>"#
    );
    let response = Tokio::send_async(&gateway.to_string(), &action, &body)
        .await
        .map_err(io::Error::other)?;
    parse_specific_mapping(&response, service, port)
}

fn parse_specific_mapping(
    response: &str,
    service: &str,
    port: u16,
) -> io::Result<Option<PortMappingEntry>> {
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid specific mapping response",
        )
    };
    let envelope = xmltree::Element::parse(response.as_bytes()).map_err(io::Error::other)?;
    if envelope.name != "Envelope"
        || envelope.namespace.as_deref() != Some("http://schemas.xmlsoap.org/soap/envelope/")
    {
        return Err(invalid());
    }
    let body = envelope.get_child("Body").ok_or_else(invalid)?;
    if let Some(fault) = body.get_child("Fault") {
        let code = fault
            .get_child("detail")
            .and_then(|detail| detail.get_child("UPnPError"))
            .and_then(|error| error.get_child("errorCode"))
            .and_then(|code| code.get_text())
            .ok_or_else(invalid)?;
        return if code.trim() == "714" {
            Ok(None)
        } else {
            Err(io::Error::other(format!(
                "UPnP specific mapping error: {code}"
            )))
        };
    }
    let entry = body
        .get_child("GetSpecificPortMappingEntryResponse")
        .ok_or_else(invalid)?;
    if entry.namespace.as_deref() != Some(service) {
        return Err(invalid());
    }
    let text = |name: &str| -> io::Result<String> {
        let field = entry.get_child(name).ok_or_else(invalid)?;
        Ok(field.get_text().unwrap_or_default().into_owned())
    };
    let enabled = match text("NewEnabled")?.trim() {
        "1" | "true" => true,
        "0" | "false" => false,
        _ => return Err(invalid()),
    };
    Ok(Some(PortMappingEntry {
        remote_host: String::new(),
        external_port: port,
        protocol: PortMappingProtocol::TCP,
        internal_port: text("NewInternalPort")?
            .trim()
            .parse()
            .map_err(io::Error::other)?,
        internal_client: text("NewInternalClient")?,
        enabled,
        port_mapping_description: text("NewPortMappingDescription")?,
        lease_duration: text("NewLeaseDuration")?
            .trim()
            .parse()
            .map_err(io::Error::other)?,
    }))
}

fn owned(entry: &PortMappingEntry, local: SocketAddr, description: &str) -> bool {
    entry.protocol == PortMappingProtocol::TCP
        && entry.external_port == local.port()
        && entry.internal_port == local.port()
        && entry.internal_client.parse::<IpAddr>().ok() == Some(local.ip())
        && entry.port_mapping_description == description
}

async fn acquire(
    gateway: &Router,
    local: SocketAddr,
    description: &str,
    settings: &PortMappingSettings,
    shutdown: &watch::Receiver<bool>,
) -> io::Result<u32> {
    let check_shutdown = || {
        if *shutdown.borrow() {
            Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "mapping worker is stopping",
            ))
        } else {
            Ok(())
        }
    };
    check_shutdown()?;
    if let Some(entry) = mapping_at(gateway, local.port(), settings).await? {
        if !owned(&entry, local, description) {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                "external TCP port already belongs to another mapping",
            ));
        }
    }
    check_shutdown()?;
    tokio::time::timeout(
        Duration::from_millis(settings.request_timeout_ms),
        gateway.add_port(
            PortMappingProtocol::TCP,
            local.port(),
            local,
            settings.lease_duration_secs,
            description,
        ),
    )
    .await
    .map_err(io::Error::other)?
    .map_err(io::Error::other)?;
    check_shutdown()?;
    match mapping_at(gateway, local.port(), settings).await? {
        Some(entry)
            if owned(&entry, local, description) && entry.enabled && entry.lease_duration > 0 =>
        {
            Ok(entry.lease_duration.min(settings.lease_duration_secs))
        }
        _ => Err(io::Error::other(
            "router did not confirm the requested finite mapping",
        )),
    }
}

async fn manage_lease(
    gateway: &Router,
    local: SocketAddr,
    description: &str,
    settings: &PortMappingSettings,
    runtime: &AppRuntime,
) -> io::Result<()> {
    let mut shutdown = runtime.shutdown_tx.subscribe();
    loop {
        if *shutdown.borrow() {
            return Ok(());
        }
        let granted_lease = acquire(gateway, local, description, settings, &shutdown).await?;
        let ip = tokio::select! {
            result = tokio::time::timeout(
                Duration::from_millis(settings.request_timeout_ms),
                gateway.get_external_ip(),
            ) => result.map_err(io::Error::other)?.map_err(io::Error::other)?,
            _ = shutdown.changed() => return Ok(()),
        };
        let external = SocketAddr::new(ip, local.port());
        *runtime.port_mapping.lock().await = PortMappingStatus::Mapped {
            internal: local,
            external,
        };
        runtime.log.log(LogLevel::Info, "port-mapping", &format!("TCP mapping registered: {external} -> {local}; external reachability remains unverified"));
        let lease_ms = u64::from(granted_lease) * 1000;
        let margin_ms = (u64::from(settings.renewal_margin_secs) * 1000).min(lease_ms / 2);
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(lease_ms - margin_ms)) => {},
            _ = shutdown.changed() => return Ok(()),
        }
    }
}

async fn remove_owned_mapping(
    gateway: &Router,
    local: SocketAddr,
    description: &str,
    settings: &PortMappingSettings,
) -> io::Result<()> {
    if let Some(entry) = mapping_at(gateway, local.port(), settings).await? {
        if owned(&entry, local, description) {
            tokio::time::timeout(
                Duration::from_millis(settings.request_timeout_ms),
                gateway.remove_port(PortMappingProtocol::TCP, local.port()),
            )
            .await
            .map_err(io::Error::other)?
            .map_err(io::Error::other)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
