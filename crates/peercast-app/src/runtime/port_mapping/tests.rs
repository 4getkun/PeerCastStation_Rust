use super::*;
use std::collections::HashMap;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const SERVICE: &str = "urn:schemas-upnp-org:service:WANIPConnection:1";

async fn acquire(
    gateway: &Router,
    local: SocketAddr,
    description: &str,
    settings: &PortMappingSettings,
) -> io::Result<u32> {
    let (_sender, shutdown) = watch::channel(false);
    super::acquire(gateway, local, description, settings, &shutdown).await
}

fn gateway(addr: SocketAddr) -> Router {
    let mut control_schema = HashMap::new();
    control_schema.insert(
        "AddPortMapping".into(),
        vec![
            "NewRemoteHost".into(),
            "NewExternalPort".into(),
            "NewProtocol".into(),
            "NewInternalPort".into(),
            "NewInternalClient".into(),
            "NewEnabled".into(),
            "NewPortMappingDescription".into(),
            "NewLeaseDuration".into(),
        ],
    );
    control_schema.insert(
        "DeletePortMapping".into(),
        vec![
            "NewRemoteHost".into(),
            "NewExternalPort".into(),
            "NewProtocol".into(),
        ],
    );
    Gateway {
        addr,
        root_url: format!("http://{addr}/"),
        control_url: "/control".into(),
        control_schema_url: String::new(),
        control_schema,
        service_type: SERVICE.into(),
        provider: Tokio,
    }
}

fn entry_response_with(
    port: u16,
    description: &str,
    lease: u32,
    enabled: u8,
    client: &str,
) -> String {
    format!(
        r#"<?xml version="1.0"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body><u:GetGenericPortMappingEntryResponse xmlns:u="{SERVICE}"><NewRemoteHost></NewRemoteHost><NewExternalPort>{port}</NewExternalPort><NewProtocol>TCP</NewProtocol><NewInternalPort>{port}</NewInternalPort><NewInternalClient>{client}</NewInternalClient><NewEnabled>{enabled}</NewEnabled><NewPortMappingDescription>{description}</NewPortMappingDescription><NewLeaseDuration>{lease}</NewLeaseDuration></u:GetGenericPortMappingEntryResponse></s:Body></s:Envelope>"#
    )
}

fn entry_response(port: u16, description: &str) -> String {
    entry_response_with(port, description, 30, 1, "192.168.1.20")
}

fn action_response(action: &str) -> String {
    format!(
        r#"<?xml version="1.0"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body><u:{action}Response xmlns:u="{SERVICE}"></u:{action}Response></s:Body></s:Envelope>"#
    )
}

fn external_ip_response() -> String {
    format!(
        r#"<?xml version="1.0"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body><u:GetExternalIPAddressResponse xmlns:u="{SERVICE}"><NewExternalIPAddress>203.0.113.5</NewExternalIPAddress></u:GetExternalIPAddressResponse></s:Body></s:Envelope>"#
    )
}

async fn serve_one(listener: &TcpListener, response_body: &str) -> String {
    let (mut stream, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
        .await
        .unwrap()
        .unwrap();
    let mut request = Vec::new();
    let mut buf = [0; 1024];
    loop {
        let n = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buf))
            .await
            .unwrap()
            .unwrap();
        assert!(n > 0, "client closed before complete SOAP request");
        request.extend_from_slice(&buf[..n]);
        if request.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    let text = String::from_utf8_lossy(&request).into_owned();
    let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", response_body.len(), response_body);
    tokio::time::timeout(
        Duration::from_secs(2),
        stream.write_all(response.as_bytes()),
    )
    .await
    .unwrap()
    .unwrap();
    text
}

async fn scripted_gateway(
    responses: Vec<String>,
) -> (Router, tokio::task::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let mut requests = Vec::new();
        for body in responses {
            requests.push(
                tokio::time::timeout(Duration::from_secs(3), serve_one(&listener, &body))
                    .await
                    .unwrap(),
            );
        }
        requests
    });
    (gateway(addr), server)
}

async fn finish_server(server: tokio::task::JoinHandle<Vec<String>>) -> Vec<String> {
    tokio::time::timeout(Duration::from_secs(4), server)
        .await
        .unwrap()
        .unwrap()
}

fn owned_settings() -> PortMappingSettings {
    PortMappingSettings {
        request_timeout_ms: 500,
        max_mapping_entries: 4,
        lease_duration_secs: 30,
        renewal_margin_secs: 1,
        ..Default::default()
    }
}

#[tokio::test]
async fn finite_mapping_can_be_renewed_and_only_owned_entry_is_removed() {
    let port = 7144;
    let description = "PeerCastStation-owned-session";
    let (gateway, server) = scripted_gateway(vec![
        entry_response(port, description),
        action_response("AddPortMapping"),
        entry_response(port, description),
        entry_response(port, description),
        action_response("AddPortMapping"),
        entry_response(port, description),
        entry_response(port, description),
        action_response("DeletePortMapping"),
    ])
    .await;
    let local = SocketAddr::from(([192, 168, 1, 20], port));
    let settings = owned_settings();
    acquire(&gateway, local, description, &settings)
        .await
        .unwrap();
    acquire(&gateway, local, description, &settings)
        .await
        .unwrap();
    remove_owned_mapping(&gateway, local, description, &settings)
        .await
        .unwrap();
    let requests = finish_server(server).await;
    assert_eq!(requests.len(), 8);
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.contains("AddPortMapping"))
            .count(),
        2
    );
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.contains("DeletePortMapping"))
            .count(),
        1
    );
    assert!(requests
        .iter()
        .filter(|request| request.contains("AddPortMapping"))
        .all(|request| request.contains("<NewLeaseDuration>30</NewLeaseDuration>")));
}

#[tokio::test]
async fn lease_manager_registers_finite_mapping_and_reports_mapping_only() {
    let port = 7144;
    let description = "PeerCastStation-lease-test";
    let mut responses = Vec::new();
    for _ in 0..2 {
        responses.extend([
            entry_response(port, description),
            action_response("AddPortMapping"),
            entry_response_with(port, description, 2, 1, "192.168.1.20"),
            external_ip_response(),
        ]);
    }
    let (gateway, server) = scripted_gateway(responses).await;
    let settings = PortMappingSettings {
        lease_duration_secs: 30,
        renewal_margin_secs: 1,
        ..owned_settings()
    };
    let mut config = AppConfig::default();
    config.runtime.port_mapping = settings.clone();
    let runtime = AppRuntime::new(
        &config,
        crate::TaskTracker::new(),
        Arc::new(crate::BoundedLogBuffer::new(8)),
        Arc::new(RwLock::new(RpcState::default())),
    )
    .unwrap();
    let local = SocketAddr::from(([192, 168, 1, 20], port));
    let worker_runtime = Arc::clone(&runtime);
    let worker_gateway = gateway.clone();
    let worker_settings = settings.clone();
    let worker_description = description.to_owned();
    let worker = tokio::spawn(async move {
        manage_lease(
            &worker_gateway,
            local,
            &worker_description,
            &worker_settings,
            &worker_runtime,
        )
        .await
    });
    let mapped = tokio::time::timeout(Duration::from_millis(500), async {
        loop {
            if let PortMappingStatus::Mapped { external, .. } = runtime.port_mapping_status().await
            {
                break external;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(mapped, SocketAddr::from(([203, 0, 113, 5], port)));
    let requests = finish_server(server).await;
    worker.abort();
    let _ = worker.await;
    assert_eq!(requests.len(), 8);
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.contains("AddPortMapping"))
            .count(),
        2
    );
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.contains("<NewLeaseDuration>30</NewLeaseDuration>"))
            .count(),
        2
    );
}

#[tokio::test]
async fn lease_manager_rejects_unverified_readback() {
    for (lease, enabled, client) in [(0, 1, "192.168.1.20"), (30, 1, "192.168.1.21")] {
        let port = 7144;
        let description = "PeerCastStation-readback-test";
        let (gateway, server) = scripted_gateway(vec![
            entry_response(port, description),
            action_response("AddPortMapping"),
            entry_response_with(port, description, lease, enabled, client),
        ])
        .await;
        let settings = owned_settings();
        let mut config = AppConfig::default();
        config.runtime.port_mapping = settings.clone();
        let runtime = AppRuntime::new(
            &config,
            crate::TaskTracker::new(),
            Arc::new(crate::BoundedLogBuffer::new(8)),
            Arc::new(RwLock::new(RpcState::default())),
        )
        .unwrap();
        let local = SocketAddr::from(([192, 168, 1, 20], port));
        let worker_runtime = Arc::clone(&runtime);
        let description = description.to_owned();
        let mut worker = tokio::spawn(async move {
            manage_lease(&gateway, local, &description, &settings, &worker_runtime).await
        });
        assert!(tokio::time::timeout(Duration::from_secs(2), &mut worker)
            .await
            .unwrap()
            .unwrap()
            .is_err());
        assert!(!matches!(
            runtime.port_mapping_status().await,
            PortMappingStatus::Mapped { .. }
        ));
        assert_eq!(finish_server(server).await.len(), 3);
    }
}

#[test]
fn port_mapping_is_disabled_by_default() {
    assert!(!PortMappingSettings::default().enabled);
}

#[tokio::test]
async fn foreign_mapping_is_neither_replaced_nor_deleted() {
    let port = 7144;
    let (gateway, server) = scripted_gateway(vec![
        entry_response(port, "someone-else"),
        entry_response(port, "someone-else"),
    ])
    .await;
    let local = SocketAddr::from(([192, 168, 1, 20], port));
    let settings = owned_settings();
    assert_eq!(
        acquire(&gateway, local, "ours", &settings)
            .await
            .unwrap_err()
            .kind(),
        io::ErrorKind::AddrInUse
    );
    remove_owned_mapping(&gateway, local, "ours", &settings)
        .await
        .unwrap();
    let requests = finish_server(server).await;
    assert!(requests.iter().all(
        |request| !request.contains("AddPortMapping") && !request.contains("DeletePortMapping")
    ));
}

#[tokio::test]
async fn mapping_scan_and_soap_timeout_are_bounded() {
    let port = 7144;
    let (scan_gateway, server) = scripted_gateway(vec![entry_response(port + 1, "other"); 2]).await;
    let settings = PortMappingSettings {
        max_mapping_entries: 2,
        ..owned_settings()
    };
    assert!(mapping_at(&scan_gateway, port, &settings).await.is_err());
    finish_server(server).await;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hanging = tokio::spawn(async move {
        let (_stream, _) = listener.accept().await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
    });
    let settings = PortMappingSettings {
        request_timeout_ms: 40,
        ..owned_settings()
    };
    let started = tokio::time::Instant::now();
    assert!(mapping_at(&gateway(addr), port, &settings).await.is_err());
    assert!(started.elapsed() < Duration::from_millis(200));
    hanging.abort();
}

fn specific_response(port: u16, description: &str) -> String {
    entry_response(port, description)
        .replace(
            "GetGenericPortMappingEntryResponse",
            "GetSpecificPortMappingEntryResponse",
        )
        .replace(&format!("<NewExternalPort>{port}</NewExternalPort>"), "")
        .replace("<NewProtocol>TCP</NewProtocol>", "")
        .replace("<NewRemoteHost></NewRemoteHost>", "")
}

fn mapping_fault(code: u16) -> String {
    format!(
        r#"<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body><s:Fault><detail><UPnPError xmlns="urn:schemas-upnp-org:control-1-0"><errorCode>{code}</errorCode></UPnPError></detail></s:Fault></s:Body></s:Envelope>"#
    )
}

#[tokio::test]
async fn specific_mapping_does_not_scan_unrelated_entries_and_cleans_up_its_lease() {
    let port = 7144;
    let description = "PeerCastStation-specific-session";
    let (mut router, server) = scripted_gateway(vec![
        mapping_fault(714),
        action_response("AddPortMapping"),
        specific_response(port, description),
        specific_response(port, description),
        action_response("DeletePortMapping"),
    ])
    .await;
    router
        .control_schema
        .insert("GetSpecificPortMappingEntry".into(), Vec::new());
    let settings = PortMappingSettings {
        max_mapping_entries: 1,
        ..owned_settings()
    };
    let local = SocketAddr::from(([192, 168, 1, 20], port));
    assert_eq!(
        acquire(&router, local, description, &settings)
            .await
            .unwrap(),
        30
    );
    remove_owned_mapping(&router, local, description, &settings)
        .await
        .unwrap();
    let requests = finish_server(server).await;
    assert_eq!(requests.len(), 5);
    assert!(requests
        .iter()
        .all(|request| !request.contains("GetGenericPortMappingEntry")));
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.contains("GetSpecificPortMappingEntry")
                && request.contains("<NewExternalPort>7144</NewExternalPort>")
                && request.contains("<NewProtocol>TCP</NewProtocol>"))
            .count(),
        3
    );
}

#[test]
fn specific_mapping_errors_and_incomplete_responses_are_not_treated_as_free_ports() {
    assert!(parse_specific_mapping(&mapping_fault(714), SERVICE, 7144)
        .unwrap()
        .is_none());
    for response in [
        mapping_fault(606),
        mapping_fault(501),
        action_response("GetSpecificPortMappingEntry"),
        specific_response(7144, "ours").replace(
            "<NewEnabled>1</NewEnabled>",
            "<NewEnabled>unknown</NewEnabled>",
        ),
        specific_response(7144, "ours").replace("<NewInternalPort>7144</NewInternalPort>", ""),
        specific_response(7144, "ours").replace(SERVICE, "urn:unexpected-service"),
    ] {
        assert!(parse_specific_mapping(&response, SERVICE, 7144).is_err());
    }
    let entry = parse_specific_mapping(&specific_response(7144, "other-app"), SERVICE, 7144)
        .unwrap()
        .unwrap();
    assert!(!owned(&entry, "192.168.1.20:7144".parse().unwrap(), "ours"));
}

#[derive(Default)]
struct DelayedMappingState {
    mapped: bool,
    adds: usize,
    deletes: usize,
    pending: bool,
}

struct DelayedGateway {
    router: Router,
    state: Arc<Mutex<DelayedMappingState>>,
    add_started: Arc<tokio::sync::Notify>,
    lookup_while_pending: Arc<tokio::sync::Notify>,
    finish_add: Arc<Semaphore>,
    server: tokio::task::JoinHandle<()>,
}

// Read the complete SOAP request, including bodies split across TCP reads.
async fn read_soap_request(stream: &mut tokio::net::TcpStream) -> String {
    let mut bytes = Vec::new();
    loop {
        let mut buffer = [0; 1024];
        let n = stream.read(&mut buffer).await.unwrap();
        assert!(n > 0);
        bytes.extend_from_slice(&buffer[..n]);
        assert!(bytes.len() <= 65536);
        if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&bytes[..end]);
            let length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            if bytes.len() >= end + 4 + length {
                return String::from_utf8(bytes).unwrap();
            }
        }
    }
}

async fn delayed_gateway(hold_add: usize, apply_before_wait: bool) -> DelayedGateway {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut router = gateway(listener.local_addr().unwrap());
    router
        .control_schema
        .insert("GetSpecificPortMappingEntry".into(), Vec::new());
    let state = Arc::new(Mutex::new(DelayedMappingState::default()));
    let add_started = Arc::new(tokio::sync::Notify::new());
    let lookup_while_pending = Arc::new(tokio::sync::Notify::new());
    let finish_add = Arc::new(Semaphore::new(0));
    let shared = (
        state.clone(),
        add_started.clone(),
        lookup_while_pending.clone(),
        finish_add.clone(),
    );
    let server = tokio::spawn(async move {
        let mut handlers = tokio::task::JoinSet::new();
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let (state, add_started, lookup_while_pending, finish_add) = shared.clone();
            handlers.spawn(async move {
                let request = read_soap_request(&mut stream).await;
                let response = if request.contains("#AddPortMapping") {
                    let held = {
                        let mut state = state.lock().await;
                        state.adds += 1;
                        state.pending = state.adds == hold_add;
                        if !state.pending || apply_before_wait { state.mapped = true; }
                        state.pending
                    };
                    if held {
                        add_started.notify_one();
                        finish_add.acquire().await.unwrap().forget();
                    }
                    let mut state = state.lock().await;
                    if !apply_before_wait { state.mapped = true; }
                    state.pending = false;
                    action_response("AddPortMapping")
                } else if request.contains("#GetSpecificPortMappingEntry") {
                    let state = state.lock().await;
                    if state.pending { lookup_while_pending.notify_one(); }
                    if state.mapped {
                        specific_response(7144, "PeerCastStation-delayed-session")
                            .replace("<NewLeaseDuration>30</NewLeaseDuration>", "<NewLeaseDuration>1</NewLeaseDuration>")
                    } else {
                        mapping_fault(714)
                    }
                } else if request.contains("#DeletePortMapping") {
                    let mut state = state.lock().await;
                    state.mapped = false;
                    state.deletes += 1;
                    action_response("DeletePortMapping")
                } else if request.contains("#GetExternalIPAddress") {
                    external_ip_response()
                } else {
                    panic!("unexpected SOAP request: {request}");
                };
                let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len());
                // The client may time out while the gateway finishes its mutation.
                let _ = stream.write_all(response.as_bytes()).await;
            });
        }
    });
    DelayedGateway {
        router,
        state,
        add_started,
        lookup_while_pending,
        finish_add,
        server,
    }
}

#[tokio::test]
async fn shutdown_during_registration_waits_for_commit_before_cleanup() {
    assert_shutdown_during_add(1).await;
}

#[tokio::test]
async fn shutdown_during_renewal_waits_for_commit_before_cleanup() {
    assert_shutdown_during_add(2).await;
}

async fn assert_shutdown_during_add(hold_add: usize) {
    let gateway = delayed_gateway(hold_add, false).await;
    let settings = owned_settings();
    let mut config = AppConfig::default();
    config.runtime.port_mapping = settings.clone();
    let runtime = AppRuntime::new(
        &config,
        crate::TaskTracker::new(),
        Arc::new(crate::BoundedLogBuffer::new(8)),
        Arc::new(RwLock::new(RpcState::default())),
    )
    .unwrap();
    let worker_runtime = runtime.clone();
    let router = gateway.router.clone();
    let worker = tokio::spawn(async move {
        manage_mapping_session(
            &router,
            SocketAddr::from(([192, 168, 1, 20], 7144)),
            "PeerCastStation-delayed-session",
            &settings,
            &worker_runtime,
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(2), gateway.add_started.notified())
        .await
        .unwrap();
    runtime.shutdown().await;
    let premature_lookup = tokio::time::timeout(
        Duration::from_millis(100),
        gateway.lookup_while_pending.notified(),
    )
    .await
    .is_ok();
    gateway.finish_add.add_permits(1);
    let outcome = tokio::time::timeout(Duration::from_secs(2), worker)
        .await
        .unwrap()
        .unwrap();
    // Wait for the gateway mutation even when the client discarded its response.
    tokio::time::timeout(Duration::from_secs(1), async {
        while gateway.state.lock().await.pending {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let state = gateway.state.lock().await;
    let mapped = state.mapped;
    let deletes = state.deletes;
    drop(state);
    gateway.server.abort();
    let _ = gateway.server.await;
    assert!(
        !premature_lookup,
        "cleanup ran before the in-flight registration settled"
    );
    assert!(outcome.is_ok(), "shutdown failed: {outcome:?}");
    assert!(!mapped, "registration survived shutdown");
    assert_eq!(deletes, 1);
}

#[tokio::test]
async fn shutdown_cleans_up_when_applied_registration_response_times_out() {
    let gateway = delayed_gateway(1, true).await;
    let settings = PortMappingSettings {
        request_timeout_ms: 200,
        ..owned_settings()
    };
    let mut config = AppConfig::default();
    config.lifecycle.shutdown_timeout_secs = 1;
    config.runtime.port_mapping = settings.clone();
    let runtime = AppRuntime::new(
        &config,
        crate::TaskTracker::new(),
        Arc::new(crate::BoundedLogBuffer::new(8)),
        Arc::new(RwLock::new(RpcState::default())),
    )
    .unwrap();
    let worker_runtime = runtime.clone();
    let router = gateway.router.clone();
    let worker = tokio::spawn(async move {
        manage_mapping_session(
            &router,
            SocketAddr::from(([192, 168, 1, 20], 7144)),
            "PeerCastStation-delayed-session",
            &settings,
            &worker_runtime,
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(2), gateway.add_started.notified())
        .await
        .unwrap();
    runtime.shutdown().await;
    // The gateway has applied AddPortMapping but never completes its response.
    tokio::time::timeout(Duration::from_millis(800), worker)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let state = gateway.state.lock().await;
    assert!(!state.mapped);
    assert_eq!(state.adds, 1);
    assert_eq!(state.deletes, 1);
    drop(state);
    gateway.server.abort();
    let _ = gateway.server.await;
}

#[tokio::test]
async fn shutdown_before_mapping_session_does_not_contact_gateway() {
    let gateway = delayed_gateway(1, false).await;
    let settings = owned_settings();
    let runtime = AppRuntime::new(
        &AppConfig::default(),
        crate::TaskTracker::new(),
        Arc::new(crate::BoundedLogBuffer::new(8)),
        Arc::new(RwLock::new(RpcState::default())),
    )
    .unwrap();
    runtime.shutdown().await;
    manage_mapping_session(
        &gateway.router,
        SocketAddr::from(([192, 168, 1, 20], 7144)),
        "PeerCastStation-delayed-session",
        &settings,
        &runtime,
    )
    .await
    .unwrap();
    let state = gateway.state.lock().await;
    assert_eq!(state.adds, 0);
    assert_eq!(state.deletes, 0);
    drop(state);
    gateway.server.abort();
    let _ = gateway.server.await;
}

#[tokio::test]
async fn unresponsive_registration_cannot_extend_shutdown_deadline() {
    let gateway = delayed_gateway(1, true).await;
    let settings = PortMappingSettings {
        request_timeout_ms: 5000,
        ..owned_settings()
    };
    let mut config = AppConfig::default();
    config.lifecycle.shutdown_timeout_secs = 1;
    config.runtime.port_mapping = settings.clone();
    let log = Arc::new(crate::BoundedLogBuffer::new(8));
    let runtime = AppRuntime::new(
        &config,
        crate::TaskTracker::new(),
        log.clone(),
        Arc::new(RwLock::new(RpcState::default())),
    )
    .unwrap();
    let worker_runtime = runtime.clone();
    let router = gateway.router.clone();
    let worker = tokio::spawn(async move {
        manage_mapping_session(
            &router,
            SocketAddr::from(([192, 168, 1, 20], 7144)),
            "PeerCastStation-delayed-session",
            &settings,
            &worker_runtime,
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(2), gateway.add_started.notified())
        .await
        .unwrap();
    runtime.shutdown().await;
    tokio::time::timeout(Duration::from_secs(1), worker)
        .await
        .expect("shutdown exceeded its budget waiting for a SOAP response")
        .unwrap()
        .unwrap();
    let state = gateway.state.lock().await;
    assert_eq!(state.adds, 1);
    drop(state);
    assert!(log.snapshot().iter().any(|entry| entry
        .message
        .contains("Mapping cleanup could not be confirmed")));
    assert!(log
        .snapshot()
        .iter()
        .any(|entry| entry.message.contains("finite lease remains the fallback")));
    gateway.server.abort();
    let _ = gateway.server.await;
}
