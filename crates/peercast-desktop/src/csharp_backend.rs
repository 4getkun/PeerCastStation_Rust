use crate::backend::ChannelSnapshot;
use futures_util::{stream, StreamExt};
use peercast_app::runtime::{ConnectionSnapshot, PortSnapshot};
use peercast_app::runtime_config::ChannelMetadata;
use reqwest::{Client, Url};
use serde_json::{json, Value};
use std::{net::SocketAddr, sync::Arc, time::Duration};

pub struct CSharpBackend {
    origin: String,
    agent_name: String,
    client: Client,
}

impl CSharpBackend {
    pub async fn connect(origin: &str) -> Result<Arc<Self>, String> {
        let mut url = Url::parse(origin).map_err(|_| "C#版の接続先が正しくありません")?;
        if url.scheme() != "http"
            || url.host_str() != Some("127.0.0.1")
            || !url.port().is_some_and(|port| port > 0)
            || url.path() != "/"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err("C#版はこのPCの明示したローカルポートだけに接続できます".into());
        }
        url.set_path("");
        let client = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_millis(300))
            .timeout(Duration::from_millis(1200))
            .build()
            .map_err(|error| error.to_string())?;
        let mut backend = Self {
            origin: url.as_str().trim_end_matches('/').to_owned(),
            agent_name: String::new(),
            client,
        };
        let version = backend.rpc("getVersionInfo").await?;
        let agent = version["agentName"].as_str().unwrap_or_default();
        if !agent.starts_with("PeerCastStation/") || version["apiVersion"] != "1.0.0" {
            return Err("対応するC#版PeerCastStation APIではありません".into());
        }
        backend.agent_name = agent.to_owned();
        Ok(Arc::new(backend))
    }

    pub async fn detect() -> Option<Arc<Self>> {
        if let Ok(backend) = Self::connect("http://127.0.0.1:7144").await {
            return Some(backend);
        }
        let ports = discover_process_ports().await;
        let mut pending = tokio::task::JoinSet::new();
        let mut ports = ports.into_iter().filter(|port| *port != 7144);
        for port in ports.by_ref().take(4) {
            pending.spawn(async move {
                Self::connect(&format!("http://127.0.0.1:{port}"))
                    .await
                    .ok()
            });
        }
        while let Some(result) = pending.join_next().await {
            if let Ok(Some(backend)) = result {
                pending.abort_all();
                return Some(backend);
            }
            if let Some(port) = ports.next() {
                pending.spawn(async move {
                    Self::connect(&format!("http://127.0.0.1:{port}"))
                        .await
                        .ok()
                });
            }
        }
        None
    }

    pub fn origin(&self) -> &str {
        &self.origin
    }
    pub fn agent_name(&self) -> &str {
        &self.agent_name
    }

    async fn rpc(&self, method: &str) -> Result<Value, String> {
        self.rpc_params(method, json!({})).await
    }

    async fn rpc_params(&self, method: &str, params: Value) -> Result<Value, String> {
        // Expose only the fixed discovery and explicit broadcast operations.
        if !matches!(
            method,
            "getVersionInfo"
                | "getChannels"
                | "getYellowPages"
                | "addYellowPage"
                | "removeYellowPage"
                | "broadcastChannel"
                | "stopChannel"
                | "getChannelInfo"
                | "getStatus"
                | "getListeners"
                | "getChannelConnections"
        ) {
            return Err("未対応のC# APIです".into());
        }
        let body =
            serde_json::to_vec(&json!({"jsonrpc":"2.0", "id":1, "method":method, "params":params}))
                .map_err(|error| error.to_string())?;
        let mut response = self
            .client
            .post(format!("{}/api/1", self.origin))
            .header("X-Requested-With", "XMLHttpRequest")
            .header("Content-Type", "application/json")
            .timeout(
                if matches!(
                    method,
                    "addYellowPage" | "broadcastChannel" | "stopChannel" | "removeYellowPage"
                ) {
                    Duration::from_secs(15)
                } else {
                    Duration::from_secs(3)
                },
            )
            .body(body)
            .send()
            .await
            .map_err(|error| format!("C#版へ接続できません: {error}"))?;
        if !response.status().is_success() {
            return Err(format!(
                "C#版API: HTTP {}（認証設定は変更しません）",
                response.status()
            ));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|error| error.to_string())? {
            if bytes.len() + chunk.len() > 2 * 1024 * 1024 {
                return Err("C#版APIの応答が大きすぎます".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        let value: Value = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        if value["jsonrpc"] != "2.0" || value["id"] != 1 || value.get("error").is_some() {
            return Err("C#版APIから正常な応答が返りませんでした".into());
        }
        value
            .get("result")
            .cloned()
            .ok_or_else(|| "C#版APIに結果がありません".into())
    }

    pub(crate) async fn ensure_sp(&self) -> Result<(i64, bool), String> {
        let pages = self.rpc("getYellowPages").await?;
        if let Some(page) = pages
            .as_array()
            .ok_or("C#版のYP一覧が正しくありません")?
            .iter()
            .find(|page| {
                is_sp_uri(
                    page["announceUri"]
                        .as_str()
                        .or_else(|| page["uri"].as_str())
                        .unwrap_or_default(),
                )
            })
        {
            return Ok((
                page["yellowPageId"]
                    .as_i64()
                    .ok_or("YP IDが正しくありません")?,
                false,
            ));
        }
        let page=self.rpc_params("addYellowPage",json!({"protocol":"pcp","name":"SP (Peercast RT)","announceUri":crate::broadcast::SP_ANNOUNCE_URI,"uri":crate::broadcast::SP_ANNOUNCE_URI,"channelsUri":"http://bayonet.ddo.jp/sp/index.txt"})).await?;
        Ok((
            page["yellowPageId"]
                .as_i64()
                .ok_or("SPの登録結果にIDがありません")?,
            true,
        ))
    }

    pub(crate) async fn remove_sp(&self, id: i64) -> Result<(), String> {
        let pages = self.rpc("getYellowPages").await?;
        if pages
            .as_array()
            .ok_or("C#版のYP一覧が正しくありません")?
            .iter()
            .any(|page| {
                page["yellowPageId"] == id
                    && page["name"] == "SP (Peercast RT)"
                    && is_sp_uri(
                        page["announceUri"]
                            .as_str()
                            .or_else(|| page["uri"].as_str())
                            .unwrap_or_default(),
                    )
            })
        {
            self.rpc_params("removeYellowPage", json!({"yellowPageId":id}))
                .await?;
        }
        Ok(())
    }

    pub(crate) async fn start_broadcast(
        &self,
        request: &crate::broadcast::BroadcastRequest,
        yp: Option<i64>,
    ) -> Result<String, String> {
        let source = if request.source_mode == "push" {
            format!("http://127.0.0.1:{}/", request.ingest_port)
        } else {
            request.source_uri.clone()
        };
        let result=self.rpc_params("broadcastChannel",json!({"yellowPageId":yp,"networkType":"ipv4","sourceUri":source,"sourceStream":if request.source_mode=="push"{"HTTP Push Source"}else{"http"},"contentReader":if request.content_type=="WMV"{"ASF(WMV or WMA)"}else{"Flash Video (FLV)"},"info":{"name":request.name,"genre":request.genre,"desc":request.description,"comment":request.comment,"url":request.contact_url,"bitrate":request.bitrate},"track":{}})).await?;
        let id = result
            .as_str()
            .ok_or("配信開始結果にチャンネルIDがありません")?;
        validate_channel_id(id)?;
        Ok(id.to_owned())
    }

    pub(crate) async fn stop_broadcast(&self, id: &str) -> Result<(), String> {
        validate_channel_id(id)?;
        if let Some(channel) = self
            .channels()
            .await?
            .into_iter()
            .find(|channel| channel.id.eq_ignore_ascii_case(id))
        {
            if !channel.is_broadcasting {
                return Err("受信リレーはこの操作では停止できません".into());
            }
            self.rpc_params("stopChannel", json!({"channelId":id}))
                .await?;
        }
        Ok(())
    }

    pub(crate) async fn announcement_status(&self, id: &str) -> Result<String, String> {
        let result = self
            .rpc_params("getChannelInfo", json!({"channelId":id}))
            .await?;
        Ok(result["yellowPages"]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .map(|row| row["status"].as_str().unwrap_or("接続中"))
                    .collect::<Vec<_>>()
                    .join(" / ")
            })
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "SPへ掲載要求中".into()))
    }

    pub async fn channels(&self) -> Result<Vec<ChannelSnapshot>, String> {
        let rows = self.rpc("getChannels").await?;
        let rows = rows
            .as_array()
            .ok_or("C#版のチャンネル一覧が正しくありません")?
            .clone();
        let api_status = self.rpc("getStatus").await.unwrap_or(Value::Null);
        let listeners = self.rpc("getListeners").await.unwrap_or(Value::Null);
        let mut tasks = Vec::with_capacity(rows.len());
        for row in rows {
            let id = row["channelId"].as_str().unwrap_or_default().to_owned();
            validate_channel_id(&id)?;
            tasks.push((id, row));
        }
        let backend = self;
        Ok(stream::iter(tasks.into_iter().map(|(id, row)| {
            let api_status = api_status.clone();
            let listeners = listeners.clone();
            async move {
                let status = &row["status"];
                let info_result = backend
                    .rpc_params("getChannelInfo", json!({"channelId":id}))
                    .await;
                let connections_result = backend
                    .rpc_params("getChannelConnections", json!({"channelId":id}))
                    .await;
                let info = info_result
                    .as_ref()
                    .ok()
                    .map(|v| &v["info"])
                    .cloned()
                    .unwrap_or_else(|| row["info"].clone());
                let connection_rows = connections_result.as_ref().ok().and_then(Value::as_array);
                let connections_available = connection_rows.is_some();
                let connections = connection_rows
                    .into_iter()
                    .flatten()
                    .filter_map(connection_snapshot)
                    .collect();
                let network = row_port_snapshot(status, &api_status, &listeners);
                let metadata = ChannelMetadata {
                    genre: info["genre"].as_str().unwrap_or_default().to_owned(),
                    description: info["desc"].as_str().unwrap_or_default().to_owned(),
                    comment: info["comment"].as_str().unwrap_or_default().to_owned(),
                    contact_url: info["url"].as_str().unwrap_or_default().to_owned(),
                    bitrate: info["bitrate"]
                        .as_u64()
                        .unwrap_or_default()
                        .min(u32::MAX as u64) as u32,
                };
                ChannelSnapshot {
                    id,
                    name: info["name"].as_str().unwrap_or("名称未設定").to_owned(),
                    status: if status["isReceiving"].as_bool() == Some(true) {
                        "受信中"
                    } else {
                        status["status"].as_str().unwrap_or("不明")
                    }
                    .into(),
                    buffered_bytes: 0,
                    buffered_packets: 0,
                    connection_attempts: 0,
                    is_broadcasting: status["isBroadcasting"].as_bool().unwrap_or(false),
                    listeners: status["localDirects"]
                        .as_u64()
                        .or_else(|| status["totalDirects"].as_u64()),
                    relays: status["localRelays"]
                        .as_u64()
                        .or_else(|| status["totalRelays"].as_u64()),
                    diagnostics: crate::backend::ChannelDiagnostics {
                        available: connections_available,
                        content_type: info["contentType"].as_str().unwrap_or_default().to_owned(),
                        purpose: String::new(),
                        metadata,
                        uptime_secs: status["uptime"]
                            .as_u64()
                            .or_else(|| api_status["uptime"].as_u64())
                            .unwrap_or_default(),
                        session_id: None,
                        connections,
                        network,
                    },
                }
            }
        }))
        .buffered(8)
        .collect()
        .await)
    }

    pub fn stream_url(&self, channel_id: &str, tracker: SocketAddr) -> Result<String, String> {
        validate_channel_id(channel_id)?;
        if tracker.port() == 0 || tracker.ip().is_unspecified() || tracker.ip().is_multicast() {
            return Err("配信元のアドレスが正しくありません".into());
        }
        let mut url = Url::parse(&format!(
            "{}/stream/{}",
            self.origin,
            channel_id.to_ascii_uppercase()
        ))
        .map_err(|error| error.to_string())?;
        url.query_pairs_mut()
            .append_pair("tip", &tracker.to_string());
        Ok(url.to_string())
    }

    pub async fn wait_for_media(&self, stream_url: &str) -> Result<(), String> {
        let url = Url::parse(stream_url).map_err(|error| error.to_string())?;
        if url.origin().ascii_serialization() != self.origin || !url.path().starts_with("/stream/")
        {
            return Err("接続中のC#版の配信URLではありません".into());
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while tokio::time::Instant::now() < deadline {
            if let Ok(response) = self.client.get(url.clone()).send().await {
                if response.status().is_success() {
                    return Ok(());
                }
                if response.status().as_u16() != 503 {
                    return Err(format!(
                        "C#版から配信を受信できません: HTTP {}",
                        response.status()
                    ));
                }
            }
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
        Err("C#版が配信へ接続中です。少し待ってから再度お試しください".into())
    }
}

fn connection_snapshot(row: &Value) -> Option<ConnectionSnapshot> {
    let id = row["connectionId"].as_u64()?;
    let kind = row["type"]
        .as_str()
        .unwrap_or("unknown")
        .to_ascii_lowercase();
    Some(ConnectionSnapshot {
        id,
        kind,
        endpoint: row["remoteEndPoint"]
            .as_str()
            .or_else(|| row["remoteName"].as_str())
            .unwrap_or_default()
            .to_owned(),
        status: row["status"]
            .as_str()
            .unwrap_or("unknown")
            .to_ascii_lowercase(),
        session_id: row["remoteSessionId"].as_str().map(str::to_owned),
        agent: row["agentName"].as_str().map(str::to_owned),
        port_open: row["remoteHostStatus"].as_array().and_then(|items| {
            items
                .iter()
                .any(|v| v.as_str() == Some("firewalled"))
                .then_some(false)
        }),
        measurements_available: false,
        received_bytes: 0,
        sent_bytes: 0,
        uptime_secs: 0,
    })
}

fn row_port_snapshot(status: &Value, global: &Value, listeners: &Value) -> Option<PortSnapshot> {
    let network = status["network"]
        .as_str()
        .unwrap_or("ipv4")
        .to_ascii_lowercase();
    let listener_for = |family: u8| {
        listeners.as_array().into_iter().flatten().find(|listener| {
            let accepts_relay = listener["localAccepts"]
                .as_i64()
                .or_else(|| listener["globalAccepts"].as_i64())
                .is_none_or(|bits| bits & 2 != 0);
            accepts_relay
                && listener["address"]
                    .as_str()
                    .and_then(|address| address.parse::<std::net::IpAddr>().ok())
                    .is_some_and(|ip| {
                        if family == 6 {
                            ip.is_ipv6()
                        } else {
                            ip.is_ipv4()
                        }
                    })
        })
    };
    let state_for = |family: u8| {
        let listener_status = listener_for(family)
            .and_then(|item| item["portStatus"].as_i64())
            .map(|n| match n {
                0 => "unavailable",
                1 => "unknown",
                2 => "closed",
                3 => "open",
                _ => "unknown",
            });
        if let Some(value) = listener_status {
            return value;
        }
        let matches_network =
            (family == 6 && network == "ipv6") || (family == 4 && network != "ipv6");
        if matches_network {
            if let Some(firewalled) = status["isFirewalled"].as_bool() {
                return if firewalled { "closed" } else { "open" };
            }
        }
        if listeners.is_array() {
            "unavailable"
        } else {
            "unknown"
        }
    };
    let family = if network == "ipv6" { 6 } else { 4 };
    let matching = listener_for(family);
    let endpoint = if family == 4 {
        global["localRelayEndPoint"].as_array()
    } else {
        None
    };
    let relay_port = endpoint
        .and_then(|v| v.get(1))
        .and_then(Value::as_u64)
        .or_else(|| matching.and_then(|v| v["port"].as_u64()))
        .unwrap_or_default()
        .min(u16::MAX as u64) as u16;
    let observed = global["globalRelayEndPoint"]
        .as_array()
        .and_then(|v| v.first())
        .and_then(Value::as_str)
        .and_then(|address| address.parse::<std::net::IpAddr>().ok());
    let ipv4 = state_for(4);
    let ipv6 = state_for(6);
    Some(PortSnapshot {
        relay_port,
        ipv4: ipv4.into(),
        ipv6: ipv6.into(),
        observed_ipv4: observed
            .filter(std::net::IpAddr::is_ipv4)
            .map(|ip| ip.to_string()),
        observed_ipv6: observed
            .filter(std::net::IpAddr::is_ipv6)
            .map(|ip| ip.to_string()),
    })
}

fn is_sp_uri(value: &str) -> bool {
    Url::parse(value).is_ok_and(|url| {
        url.scheme() == "pcp"
            && url.host_str() == Some("bayonet.ddo.jp")
            && url.port() == Some(7146)
            && url.username().is_empty()
            && url.password().is_none()
    })
}

fn validate_channel_id(id: &str) -> Result<(), String> {
    if id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err("チャンネルIDが正しくありません".into())
    }
}

#[cfg(windows)]
async fn discover_process_ports() -> Vec<u16> {
    use std::collections::HashSet;
    async fn command_output(name: &str, args: &[&str]) -> Option<String> {
        let executable = std::path::PathBuf::from(
            std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into()),
        )
        .join("System32")
        .join(name);
        let mut command = tokio::process::Command::new(executable);
        command
            .args(args)
            .creation_flags(0x08000000)
            .kill_on_drop(true);
        let output = tokio::time::timeout(Duration::from_secs(2), command.output())
            .await
            .ok()?
            .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
    }
    let Some(tasks) = command_output(
        "tasklist.exe",
        &[
            "/fi",
            "imagename eq PeerCastStation.exe",
            "/fo",
            "csv",
            "/nh",
        ],
    )
    .await
    else {
        return Vec::new();
    };
    let pids: HashSet<u32> = tasks
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix('"')?
                .split("\",\"")
                .nth(1)?
                .parse()
                .ok()
        })
        .collect();
    if pids.is_empty() {
        return Vec::new();
    }
    let Some(connections) = command_output("netstat.exe", &["-ano", "-p", "tcp"]).await else {
        return Vec::new();
    };
    let mut ports: Vec<u16> = connections
        .lines()
        .filter_map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if fields.len() != 5 || !pids.contains(&fields[4].parse::<u32>().ok()?) {
                return None;
            }
            let local: SocketAddr = fields[1].parse().ok()?;
            let remote: SocketAddr = fields[2].parse().ok()?;
            (remote.port() == 0 && local.port() > 0).then_some(local.port())
        })
        .collect();
    ports.sort_unstable();
    ports.dedup();
    ports.truncate(16);
    ports
}

#[cfg(not(windows))]
async fn discover_process_ports() -> Vec<u16> {
    Vec::new()
}
