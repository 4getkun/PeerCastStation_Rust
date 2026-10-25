use super::*;
use crate::runtime_config::{DirectorySettings, YellowPageSettings};
use peercast_rpc::server::YellowPageDirectory;
use serde_json::{json, Value};
use std::future::Future;
use std::pin::Pin;
use tokio::task::JoinSet;

const MAX_CONCURRENT_DIRECTORY_REQUESTS: usize = 8;

/// Directory HTTP requests are independent of PCP announcement sessions.
pub(crate) struct DirectoryService {
    client: reqwest::Client,
    yellow_pages: Vec<YellowPageSettings>,
    limits: DirectorySettings,
    cache: Mutex<Option<(Instant, Value)>>,
    log: Arc<BoundedLogBuffer>,
    shutdown: watch::Receiver<bool>,
}

impl std::fmt::Debug for DirectoryService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DirectoryService").finish_non_exhaustive()
    }
}

impl DirectoryService {
    pub(super) fn new(
        config: &AppConfig,
        log: Arc<BoundedLogBuffer>,
        shutdown: watch::Receiver<bool>,
    ) -> io::Result<Arc<Self>> {
        let client = reqwest::Client::builder()
            .user_agent(concat!("PeerCastStation/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_millis(
                config.runtime.directory.request_timeout_ms,
            ))
            .build()
            .map_err(io::Error::other)?;
        Ok(Arc::new(Self {
            client,
            yellow_pages: config.runtime.yellow_pages.clone(),
            limits: config.runtime.directory.clone(),
            cache: Mutex::new(None),
            log,
            shutdown,
        }))
    }

    async fn load(&self, refresh: bool) -> Result<Value, String> {
        let mut shutdown = self.shutdown.clone();
        if *shutdown.borrow() {
            return Err("runtime has stopped".into());
        }
        let operation = async {
            // Serialize refreshes so a late request cannot overwrite a newer snapshot.
            let mut cache = self.cache.lock().await;
            if !refresh {
                if let Some((updated, value)) = &*cache {
                    if updated.elapsed() < Duration::from_secs(self.limits.cache_ttl_secs) {
                        return Ok(value.clone());
                    }
                }
            }
            let mut pending = self
                .yellow_pages
                .iter()
                .filter(|yp| yp.channels_uri.is_some());
            let mut requests = JoinSet::new();
            let mut results = Vec::new();
            loop {
                while requests.len() < MAX_CONCURRENT_DIRECTORY_REQUESTS {
                    let Some(yp) = pending.next() else {
                        break;
                    };
                    let client = self.client.clone();
                    let yp = yp.clone();
                    let limits = self.limits.clone();
                    requests.spawn(async move { fetch(&client, &yp, &limits).await });
                }
                let Some(result) = requests.join_next().await else {
                    break;
                };
                match result {
                    Ok(Ok(entries)) => results.extend(entries),
                    Ok(Err(error)) => self.log.log(LogLevel::Warn, "directory", &error),
                    Err(error) => self
                        .log
                        .log(LogLevel::Warn, "directory", &error.to_string()),
                }
            }
            results.sort_by(|a, b| a["channelId"].as_str().cmp(&b["channelId"].as_str()));
            let value = Value::Array(results);
            *cache = Some((Instant::now(), value.clone()));
            Ok(value)
        };
        tokio::select! {
            result = tokio::time::timeout(Duration::from_millis(self.limits.request_timeout_ms), operation) => result.map_err(|_| "directory request timed out".to_string())?,
            _ = shutdown.changed() => Err("runtime has stopped".into()),
        }
    }
}

impl YellowPageDirectory for DirectoryService {
    fn channels(
        &self,
        refresh: bool,
    ) -> Pin<Box<dyn Future<Output = Result<Value, String>> + Send + '_>> {
        Box::pin(self.load(refresh))
    }
}

async fn fetch(
    client: &reqwest::Client,
    yp: &YellowPageSettings,
    limits: &DirectorySettings,
) -> Result<Vec<Value>, String> {
    let mut response = client
        .get(yp.channels_uri.as_deref().expect("filtered URI"))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;
    let mut body = Vec::new();
    if response
        .content_length()
        .is_some_and(|len| len > limits.max_total_bytes as u64)
    {
        return Err("directory body exceeds configured limit".into());
    }
    while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
        if chunk.len() > limits.max_total_bytes.saturating_sub(body.len()) {
            return Err("directory body exceeds configured limit".into());
        }
        body.extend_from_slice(&chunk);
    }
    let entries = parse_index_txt(
        &String::from_utf8_lossy(&body),
        &IndexParserLimits {
            max_total_bytes: limits.max_total_bytes,
            max_lines: limits.max_lines,
            max_field_len: limits.max_field_len,
        },
    )
    .map_err(|e| e.to_string())?;
    Ok(entries.into_iter().map(|entry| json!({
        "yellowPage": if yp.name.is_empty() { yp.endpoint.to_string() } else { yp.name.clone() },
        "name": entry.name, "channelId": entry.channel_id.to_hex_upper(), "tracker": entry.tracker,
        "contactUrl": entry.contact_url, "genre": entry.genre, "description": entry.description,
        "comment": entry.comment, "bitrate": entry.bitrate_kbps, "contentType": entry.content_type,
        "trackTitle": entry.track_title, "album": entry.album, "creator": entry.artist,
        "trackUrl": entry.track_url, "listeners": entry.listeners, "relays": entry.relays,
        "uptime": entry.uptime.map(|duration| duration.as_secs() as f64),
    })).collect())
}
