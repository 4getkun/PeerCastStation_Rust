//! Bounded YellowPage directory listing (`index.txt`) parser and HTML entity decoding (REQ-063).
//!
//! # Architectural Invariants (REQ-063)
//! 1. **Schema Authority**:
//!    The column schema is established from PeerCastStation (`PCPYellowPageClient.cs:1118-1177`)
//!    and PeerCast-YT (`public.cpp:115-136` & `chandir.cpp:26-30`):
//!    - Delimiter: `<>`
//!    - Field 0: Channel Name (HTML decoded)
//!    - Field 1: Channel ID (32 hex characters / GUID)
//!    - Field 2: Tracker (TIP: IP and Port, blank for push broadcast)
//!    - Field 3: Contact URL (HTML decoded)
//!    - Field 4: Genre (HTML decoded)
//!    - Field 5: Description (HTML decoded)
//!    - Field 6: Listeners count (integer; -1 = hidden)
//!    - Field 7: Relays count (integer)
//!    - Field 8: Bitrate in kbps (integer)
//!    - Field 9: Content Type / Media Format (e.g. "FLV", "WMV", "MKV", "OGG", "MP3")
//!    - Field 10: Track Artist (HTML decoded)
//!    - Field 11: Track Album (HTML decoded)
//!    - Field 12: Track Title (HTML decoded)
//!    - Field 13: Track Contact URL (HTML decoded)
//!    - Field 14: Escaped name or reserved
//!    - Field 15: Broadcast Uptime (formatted as `HH:MM`, `HH:MM:SS`, or total seconds)
//!    - Field 16: Click / Stats URL indicator
//!    - Field 17: Comment (HTML decoded)
//!    - Field 18: Direct permission / extra flags
//! 2. **Bounded Resources (Global Rule 3)**:
//!    Response size (`max_total_bytes`), line counts (`max_lines`), and per-field lengths
//!    (`max_field_len`) are strictly enforced. Malformed rows do not crash the parser.
//! 3. **Separation of Concerns**:
//!    Directory listing parsing is a standalone bounded operation completely isolated from
//!    PCP announcement sockets and connection lifecycle.

use std::time::Duration;
use thiserror::Error;

use crate::channel_id::ChannelId;

/// Default maximum total bytes allowed for an `index.txt` response (2 MiB).
pub const DEFAULT_MAX_INDEX_BYTES: usize = 2 * 1024 * 1024;

/// Default maximum number of channel entries parsed from an `index.txt` response (10,000 rows).
pub const DEFAULT_MAX_INDEX_LINES: usize = 10_000;

/// Default maximum allowed byte length for a single field/token (4,096 bytes).
pub const DEFAULT_MAX_FIELD_LEN: usize = 4096;

/// Delimiter separating tokens in standard `index.txt` feeds.
pub const INDEX_DELIMITER: &str = "<>";

/// Errors returned during `index.txt` parsing.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum IndexParseError {
    #[error("Response body size ({size} bytes) exceeds maximum limit ({limit} bytes)")]
    ResponseTooLarge { size: usize, limit: usize },
    #[error(
        "Field {field} at line {line} length ({len} bytes) exceeds maximum limit ({limit} bytes)"
    )]
    FieldTooLong {
        line: usize,
        field: usize,
        len: usize,
        limit: usize,
    },
}

/// Resource limits applied during `index.txt` directory listing parsing (REQ-063, Global Rule 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexParserLimits {
    pub max_total_bytes: usize,
    pub max_lines: usize,
    pub max_field_len: usize,
}

impl Default for IndexParserLimits {
    fn default() -> Self {
        Self {
            max_total_bytes: DEFAULT_MAX_INDEX_BYTES,
            max_lines: DEFAULT_MAX_INDEX_LINES,
            max_field_len: DEFAULT_MAX_FIELD_LEN,
        }
    }
}

/// Represents a parsed channel entry from a YellowPage `index.txt` directory listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryChannelEntry {
    /// Channel name (Field 0, HTML decoded).
    pub name: String,
    /// Channel GUID/ID (Field 1, 32 hex characters).
    pub channel_id: ChannelId,
    /// Tracker IP and port (Field 2, TIP).
    pub tracker: Option<String>,
    /// Contact URL (Field 3, HTML decoded).
    pub contact_url: Option<String>,
    /// Genre (Field 4, HTML decoded).
    pub genre: String,
    /// Description (Field 5, HTML decoded).
    pub description: String,
    /// Listeners count (Field 6). -1 indicates hidden.
    pub listeners: Option<i32>,
    /// Relays count (Field 7).
    pub relays: Option<i32>,
    /// Bitrate in kbps (Field 8).
    pub bitrate_kbps: Option<u32>,
    /// Content type / media format (Field 9, e.g. "FLV", "WMV", "MKV", "OGG", "MP3").
    pub content_type: String,
    /// Track Artist (Field 10, HTML decoded).
    pub artist: Option<String>,
    /// Track Album (Field 11, HTML decoded).
    pub album: Option<String>,
    /// Track Title (Field 12, HTML decoded).
    pub track_title: Option<String>,
    /// Track URL (Field 13, HTML decoded).
    pub track_url: Option<String>,
    /// Broadcast uptime (Field 15).
    pub uptime: Option<Duration>,
    /// Comment (Field 17, HTML decoded).
    pub comment: Option<String>,
    /// Total count of tokens present in the source row.
    pub raw_token_count: usize,
}

/// Decodes standard HTML entities and numeric character references.
///
/// Parity with Station `WebUtility.HtmlDecode`.
pub fn html_decode(input: &str) -> String {
    if !input.contains('&') {
        return input.to_string();
    }

    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '&' {
            let mut entity = String::new();
            let mut terminated = false;
            while let Some(&next_ch) = chars.peek() {
                if next_ch == ';' {
                    chars.next();
                    terminated = true;
                    break;
                }
                if next_ch.is_whitespace() || next_ch == '&' || entity.len() >= 10 {
                    break;
                }
                entity.push(chars.next().unwrap());
            }

            if terminated {
                match entity.as_str() {
                    "amp" => result.push('&'),
                    "lt" => result.push('<'),
                    "gt" => result.push('>'),
                    "quot" => result.push('"'),
                    "apos" => result.push('\''),
                    "nbsp" => result.push(' '),
                    _ if entity.starts_with('#') => {
                        let num_str = &entity[1..];
                        let code_point = if let Some(hex_str) = num_str.strip_prefix('x') {
                            u32::from_str_radix(hex_str, 16).ok()
                        } else {
                            num_str.parse::<u32>().ok()
                        };

                        if let Some(cp) = code_point.and_then(char::from_u32) {
                            result.push(cp);
                        } else {
                            result.push('&');
                            result.push_str(&entity);
                            result.push(';');
                        }
                    }
                    _ => {
                        result.push('&');
                        result.push_str(&entity);
                        result.push(';');
                    }
                }
            } else {
                result.push('&');
                result.push_str(&entity);
            }
        } else {
            result.push(ch);
        }
    }

    result
}

/// Parses an uptime string formatted as `HH:MM`, `HH:MM:SS`, or total seconds into a [`Duration`].
///
/// Parity with Station `PCPYellowPageClient.ParseUptime`:
/// - If `HH:MM`: `(hours * 60 + minutes) * 60`
/// - If `HH:MM:SS`: `hours * 3600 + minutes * 60 + seconds`
/// - If single integer: interpreted as seconds.
pub fn parse_uptime_str(token: &str) -> Option<Duration> {
    let token = token.trim();
    if token.is_empty() {
        return None;
    }

    if token.contains(':') {
        let parts: Vec<&str> = token.split(':').collect();
        if parts.len() == 2 {
            let hours = parts[0].trim().parse::<u64>().ok()?;
            let minutes = parts[1].trim().parse::<u64>().ok()?;
            Some(Duration::from_secs(hours * 3600 + minutes * 60))
        } else if parts.len() == 3 {
            let hours = parts[0].trim().parse::<u64>().ok()?;
            let minutes = parts[1].trim().parse::<u64>().ok()?;
            let seconds = parts[2].trim().parse::<u64>().ok()?;
            Some(Duration::from_secs(hours * 3600 + minutes * 60 + seconds))
        } else {
            None
        }
    } else {
        let secs = token.parse::<u64>().ok()?;
        Some(Duration::from_secs(secs))
    }
}

/// Helper function to parse an optional, HTML-decoded string from a token.
fn parse_opt_str(token: &str) -> Option<String> {
    let trimmed = token.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(html_decode(trimmed))
    }
}

/// Helper function to parse an optional integer.
fn parse_opt_int(token: &str) -> Option<i32> {
    let trimmed = token.trim();
    if trimmed.is_empty() {
        None
    } else {
        trimmed.parse::<i32>().ok()
    }
}

/// Helper function to parse an optional unsigned integer.
fn parse_opt_u32(token: &str) -> Option<u32> {
    let trimmed = token.trim();
    if trimmed.is_empty() {
        None
    } else {
        trimmed.parse::<u32>().ok()
    }
}

/// Parses a YellowPage `index.txt` body into a list of [`DirectoryChannelEntry`] records (REQ-063).
///
/// Enforces `limits` on total response size, maximum lines, and individual field lengths.
/// Malformed or empty rows are gracefully handled and skipped without panicking.
pub fn parse_index_txt(
    text: &str,
    limits: &IndexParserLimits,
) -> Result<Vec<DirectoryChannelEntry>, IndexParseError> {
    if text.len() > limits.max_total_bytes {
        return Err(IndexParseError::ResponseTooLarge {
            size: text.len(),
            limit: limits.max_total_bytes,
        });
    }

    let mut entries = Vec::new();

    for (line_idx, line) in text.lines().enumerate() {
        if entries.len() >= limits.max_lines {
            break;
        }

        let trimmed_line = line.trim();
        if trimmed_line.is_empty() {
            continue;
        }

        // Split by standard "<>" delimiter
        let tokens: Vec<&str> = trimmed_line.split(INDEX_DELIMITER).collect();
        if tokens.is_empty() {
            continue;
        }

        // Enforce per-field length limit
        for (field_idx, &token) in tokens.iter().enumerate() {
            if token.len() > limits.max_field_len {
                return Err(IndexParseError::FieldTooLong {
                    line: line_idx + 1,
                    field: field_idx,
                    len: token.len(),
                    limit: limits.max_field_len,
                });
            }
        }

        // Field 0: Channel Name
        let name = tokens
            .first()
            .map(|&s| html_decode(s.trim()))
            .unwrap_or_default();

        // Field 1: Channel GUID
        let channel_id = if tokens.len() > 1 {
            ChannelId::from_hex(tokens[1].trim()).unwrap_or_default()
        } else {
            ChannelId::default()
        };

        // Field 2: Tracker (TIP)
        let tracker = tokens.get(2).and_then(|&s| parse_opt_str(s));

        // Field 3: Contact URL
        let contact_url = tokens.get(3).and_then(|&s| parse_opt_str(s));

        // Field 4: Genre
        let genre = tokens
            .get(4)
            .map(|&s| html_decode(s.trim()))
            .unwrap_or_default();

        // Field 5: Description
        let description = tokens
            .get(5)
            .map(|&s| html_decode(s.trim()))
            .unwrap_or_default();

        // Field 6: Listeners count
        let listeners = tokens.get(6).and_then(|&s| parse_opt_int(s));

        // Field 7: Relays count
        let relays = tokens.get(7).and_then(|&s| parse_opt_int(s));

        // Field 8: Bitrate in kbps
        let bitrate_kbps = tokens.get(8).and_then(|&s| parse_opt_u32(s));

        // Field 9: Content Type / Media Format
        let content_type = tokens
            .get(9)
            .map(|&s| s.trim().to_uppercase())
            .unwrap_or_default();

        // Field 10: Artist
        let artist = tokens.get(10).and_then(|&s| parse_opt_str(s));

        // Field 11: Album
        let album = tokens.get(11).and_then(|&s| parse_opt_str(s));

        // Field 12: Track Title
        let track_title = tokens.get(12).and_then(|&s| parse_opt_str(s));

        // Field 13: Track URL
        let track_url = tokens.get(13).and_then(|&s| parse_opt_str(s));

        // Field 15: Uptime
        let uptime = tokens.get(15).and_then(|&s| parse_uptime_str(s));

        // Field 17: Comment
        let comment = tokens.get(17).and_then(|&s| parse_opt_str(s));

        entries.push(DirectoryChannelEntry {
            name,
            channel_id,
            tracker,
            contact_url,
            genre,
            description,
            listeners,
            relays,
            bitrate_kbps,
            content_type,
            artist,
            album,
            track_title,
            track_url,
            uptime,
            comment,
            raw_token_count: tokens.len(),
        });
    }

    Ok(entries)
}
