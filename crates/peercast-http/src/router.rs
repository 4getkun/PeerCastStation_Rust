use crate::playlist::PlaylistFormat;
use thiserror::Error;

/// Routing errors.
#[derive(Error, Debug, PartialEq, Eq, Clone)]
pub enum RoutingError {
    #[error("Malformed HTTP request: {0}")]
    MalformedRequest(&'static str),

    #[error("HTTP header exceeds budget ({0} bytes)")]
    HeaderTooLarge(usize),

    #[error("Method not allowed: {0}")]
    MethodNotAllowed(String),

    #[error("Invalid or missing channel ID: {0}")]
    InvalidChannelId(String),

    #[error("Path not found: {0}")]
    NotFound(String),
}

/// HTTP method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Head,
    Post,
}

/// HTTP version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpVersion {
    Http10,
    Http11,
}

/// Parsed route target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteTarget {
    /// `/stream/{channel_id}[.ext]`
    Stream {
        channel_id_hex: String,
        extension: Option<String>,
    },
    /// `/pls/{channel_id}[.ext]`
    Playlist {
        channel_id_hex: String,
        format: PlaylistFormat,
        extension: Option<String>,
    },
    /// Static / embedded web asset route (e.g. `/`, `/index.html`, `/css/tokens.css`).
    StaticAsset {
        path: String,
    },
    /// JSON-RPC 2.0 API endpoint (`/api/1` or `/api/1/peercaststation.js`).
    RpcApi {
        subpath: String,
    },
}

/// A parsed incoming HTTP request for direct streaming or playlist generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: HttpMethod,
    pub version: HttpVersion,
    pub raw_path: String,
    pub target: RouteTarget,
    pub query_params: Vec<(String, String)>,
    pub host: Option<String>,
    pub user_agent: Option<String>,
    pub pragma: Option<String>,
    pub connection_close: bool,
}

impl HttpRequest {
    /// Parses an HTTP request line and headers from a raw byte slice.
    pub fn parse(input: &[u8]) -> Result<HttpRequest, RoutingError> {
        let text = match std::str::from_utf8(input) {
            Ok(s) => s,
            Err(_) => return Err(RoutingError::MalformedRequest("Non-UTF8 request")),
        };

        let mut lines = text.split("\r\n");
        let request_line = lines
            .next()
            .ok_or(RoutingError::MalformedRequest("Empty request line"))?;

        let mut parts = request_line.split_whitespace();
        let method_str = parts
            .next()
            .ok_or(RoutingError::MalformedRequest("Missing HTTP method"))?;
        let uri_str = parts
            .next()
            .ok_or(RoutingError::MalformedRequest("Missing URI"))?;
        let version_str = parts
            .next()
            .ok_or(RoutingError::MalformedRequest("Missing HTTP version"))?;

        let method = match method_str {
            "GET" => HttpMethod::Get,
            "HEAD" => HttpMethod::Head,
            "POST" => HttpMethod::Post,
            other => return Err(RoutingError::MethodNotAllowed(other.to_string())),
        };

        let version = match version_str {
            "HTTP/1.1" => HttpVersion::Http11,
            "HTTP/1.0" => HttpVersion::Http10,
            _ => HttpVersion::Http10,
        };

        let mut host = None;
        let mut user_agent = None;
        let mut pragma = None;
        let mut connection_close = version == HttpVersion::Http10;

        for line in lines {
            if line.is_empty() {
                break;
            }
            if let Some((k, v)) = line.split_once(':') {
                let name = k.trim().to_ascii_lowercase();
                let val = v.trim();
                match name.as_str() {
                    "host" => host = Some(val.to_string()),
                    "user-agent" => user_agent = Some(val.to_string()),
                    "pragma" => pragma = Some(val.to_string()),
                    "connection" => {
                        if val.eq_ignore_ascii_case("close") {
                            connection_close = true;
                        } else if val.eq_ignore_ascii_case("keep-alive") {
                            connection_close = false;
                        }
                    }
                    _ => {}
                }
            }
        }

        // Parse URI and query params
        let (path_part, query_part) = match uri_str.split_once('?') {
            Some((p, q)) => (p, Some(q)),
            None => (uri_str, None),
        };

        let mut query_params = Vec::new();
        if let Some(q) = query_part {
            for pair in q.split('&') {
                if pair.is_empty() {
                    continue;
                }
                if let Some((k, v)) = pair.split_once('=') {
                    query_params.push((k.to_string(), v.to_string()));
                } else {
                    query_params.push((pair.to_string(), String::new()));
                }
            }
        }

        let target = RouteTarget::parse_path(path_part)?;

        // Enforce method constraints per target route (REQ-060)
        match target {
            RouteTarget::Stream { .. } | RouteTarget::Playlist { .. } | RouteTarget::StaticAsset { .. } => {
                if method != HttpMethod::Get && method != HttpMethod::Head {
                    return Err(RoutingError::MethodNotAllowed(method_str.to_string()));
                }
            }
            RouteTarget::RpcApi { .. } => {
                // RPC API supports GET and POST
            }
        }

        Ok(HttpRequest {
            method,
            version,
            raw_path: uri_str.to_string(),
            target,
            query_params,
            host,
            user_agent,
            pragma,
            connection_close,
        })
    }
}

impl RouteTarget {
    pub fn parse_path(path: &str) -> Result<RouteTarget, RoutingError> {
        let trimmed = path.trim_start_matches('/');

        if let Some(rest) = trimmed.strip_prefix("stream/") {
            let (channel_id_hex, ext) = extract_channel_id_and_ext(rest)?;
            Ok(RouteTarget::Stream {
                channel_id_hex,
                extension: ext,
            })
        } else if let Some(rest) = trimmed.strip_prefix("pls/") {
            let (channel_id_hex, ext) = extract_channel_id_and_ext(rest)?;
            let format = match &ext {
                Some(e) => PlaylistFormat::from_extension(e),
                None => PlaylistFormat::M3U,
            };
            Ok(RouteTarget::Playlist {
                channel_id_hex,
                format,
                extension: ext,
            })
        } else if trimmed == "api/1" || trimmed.starts_with("api/1/") {
            Ok(RouteTarget::RpcApi {
                subpath: trimmed.strip_prefix("api/1").unwrap_or("").to_string(),
            })
        } else if trimmed.is_empty()
            || trimmed.ends_with(".html")
            || trimmed.ends_with(".css")
            || trimmed.ends_with(".js")
            || trimmed.ends_with(".json")
            || trimmed.ends_with(".svg")
            || trimmed.ends_with(".png")
            || trimmed.ends_with(".ico")
            || trimmed.starts_with("html/")
            || trimmed.starts_with("ui/")
            || trimmed.starts_with("css/")
            || trimmed.starts_with("fixtures/")
        {
            Ok(RouteTarget::StaticAsset {
                path: format!("/{}", trimmed),
            })
        } else {
            Err(RoutingError::NotFound(path.to_string()))
        }
    }
}

/// Extracts and validates a 32-character hexadecimal channel ID and optional extension (REQ-060).
fn extract_channel_id_and_ext(
    segment: &str,
) -> Result<(String, Option<String>), RoutingError> {
    if segment.is_empty() {
        return Err(RoutingError::InvalidChannelId(segment.to_string()));
    }

    let (id_candidate, ext) = match segment.split_once('.') {
        Some((id, ext)) => (id, Some(ext.to_string())),
        None => (segment, None),
    };

    // Valid channel ID must be exactly 32 hex chars (128-bit GUID)
    if id_candidate.len() != 32 || !id_candidate.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(RoutingError::InvalidChannelId(segment.to_string()));
    }

    Ok((id_candidate.to_ascii_uppercase(), ext))
}
