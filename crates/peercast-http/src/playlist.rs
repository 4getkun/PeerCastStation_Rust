use bytes::Bytes;
use thiserror::Error;

/// Playlist generation error.
#[derive(Error, Debug, PartialEq, Eq, Clone)]
pub enum PlaylistError {
    #[error("Invalid channel ID format: {0}")]
    InvalidChannelId(String),

    #[error("Empty channel list")]
    EmptyChannelList,
}

/// Supported playlist formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaylistFormat {
    M3U,
    PLS,
    ASX,
}

impl PlaylistFormat {
    pub fn mime_type(&self) -> &'static str {
        match self {
            Self::M3U => "audio/x-mpegurl",
            Self::PLS => "audio/x-scpls",
            Self::ASX => "video/x-ms-asf",
        }
    }

    pub fn from_extension(ext: &str) -> Self {
        match ext.to_ascii_lowercase().as_str() {
            "m3u" | "m3u8" => Self::M3U,
            "pls" => Self::PLS,
            "asx" | "wax" | "wvx" => Self::ASX,
            _ => Self::M3U, // Default fallback
        }
    }
}

/// Metadata for a channel entry in a playlist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaylistEntry {
    pub channel_id_hex: String,
    pub title: String,
    pub extension: String,
    pub contact_url: Option<String>,
}

/// Sanitizes a string for single-line playlist text (M3U / PLS) by stripping newlines (CR, LF)
/// to prevent playlist header injection attacks (REQ-060).
pub fn sanitize_single_line(input: &str) -> String {
    input
        .chars()
        .filter(|&c| c != '\r' && c != '\n')
        .collect::<String>()
        .trim()
        .to_string()
}

/// Escapes XML special characters for ASX playlists (REQ-060).
pub fn escape_xml(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 16);
    for c in input.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            other => out.push(other),
        }
    }
    out
}

/// Builds playlist content for M3U, PLS, and ASX formats (REQ-060).
pub struct PlaylistBuilder {
    format: PlaylistFormat,
    scheme: String,
    base_host: String,
    query_params: Vec<(String, String)>,
    entries: Vec<PlaylistEntry>,
}

impl PlaylistBuilder {
    pub fn new(format: PlaylistFormat, base_host: impl Into<String>) -> Self {
        let scheme = match format {
            PlaylistFormat::ASX => "mms".to_string(),
            _ => "http".to_string(),
        };
        Self {
            format,
            scheme,
            base_host: base_host.into(),
            query_params: Vec::new(),
            entries: Vec::new(),
        }
    }

    pub fn with_scheme(mut self, scheme: impl Into<String>) -> Self {
        let s = scheme.into();
        if !s.is_empty() {
            self.scheme = s.to_ascii_lowercase();
        }
        self
    }

    pub fn add_query_param(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.query_params.push((key.into(), value.into()));
        self
    }

    pub fn add_entry(mut self, entry: PlaylistEntry) -> Self {
        self.entries.push(entry);
        self
    }

    /// Builds the stream URL for an entry with escaped query parameters.
    fn make_stream_url(&self, entry: &PlaylistEntry) -> String {
        let ext = if entry.extension.starts_with('.') {
            entry.extension.clone()
        } else if entry.extension.is_empty() {
            String::new()
        } else {
            format!(".{}", entry.extension)
        };

        let mut url = format!(
            "{}://{}/stream/{}{}",
            self.scheme, self.base_host, entry.channel_id_hex, ext
        );

        if !self.query_params.is_empty() {
            let query_str = self
                .query_params
                .iter()
                .map(|(k, v)| format!("{}={}", urlencoding_simple(k), urlencoding_simple(v)))
                .collect::<Vec<_>>()
                .join("&");
            url.push('?');
            url.push_str(&query_str);
        }

        url
    }

    /// Generates the serialized playlist bytes.
    pub fn build(&self) -> Bytes {
        match self.format {
            PlaylistFormat::M3U => self.build_m3u(),
            PlaylistFormat::PLS => self.build_pls(),
            PlaylistFormat::ASX => self.build_asx(),
        }
    }

    fn build_m3u(&self) -> Bytes {
        let mut out = String::new();
        out.push_str("#EXTM3U\r\n");
        for entry in &self.entries {
            let title = sanitize_single_line(&entry.title);
            let url = self.make_stream_url(entry);
            out.push_str(&format!("#EXTINF:-1, {}\r\n", title));
            out.push_str(&format!("{}\r\n", url));
        }
        Bytes::from(out)
    }

    fn build_pls(&self) -> Bytes {
        let mut out = String::new();
        out.push_str("[playlist]\r\n");
        out.push_str(&format!("NumberOfEntries={}\r\n", self.entries.len()));

        for (i, entry) in self.entries.iter().enumerate() {
            let idx = i + 1;
            let title = sanitize_single_line(&entry.title);
            let url = self.make_stream_url(entry);
            out.push_str(&format!("File{}={}\r\n", idx, url));
            out.push_str(&format!("Title{}={}\r\n", idx, title));
            out.push_str(&format!("Length{}=-1\r\n", idx));
        }
        out.push_str("Version=2\r\n");
        Bytes::from(out)
    }

    fn build_asx(&self) -> Bytes {
        let mut out = String::new();
        out.push_str("<ASX version=\"3.0\">\r\n");

        if let Some(first) = self.entries.first() {
            let title_xml = escape_xml(&sanitize_single_line(&first.title));
            out.push_str(&format!("  <Title>{}</Title>\r\n", title_xml));
        }

        for entry in &self.entries {
            let title_xml = escape_xml(&sanitize_single_line(&entry.title));
            let stream_url = escape_xml(&self.make_stream_url(entry));

            out.push_str("  <Entry>\r\n");
            out.push_str(&format!("    <Title>{}</Title>\r\n", title_xml));

            if let Some(contact) = &entry.contact_url {
                if !contact.is_empty() {
                    let contact_xml = escape_xml(contact);
                    out.push_str(&format!("    <MoreInfo href=\"{}\" />\r\n", contact_xml));
                }
            }

            out.push_str(&format!("    <Ref href=\"{}\" />\r\n", stream_url));
            out.push_str("  </Entry>\r\n");
        }

        out.push_str("</ASX>\r\n");
        Bytes::from(out)
    }
}

fn urlencoding_simple(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            other => out.push_str(&format!("%{:02X}", other)),
        }
    }
    out
}
