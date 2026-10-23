//! PeerCast HTTP direct playback and playlist serving engine (REQ-060).
//!
//! Enforces:
//! - REQ-060: /stream and /pls routing with validated ChannelIDs, explicit MIME types,
//!   playlist escaping, and on-demand subscriber lifecycle.
//! - Protocol framing parity: strictly forbids applying chunked transfer encoding
//!   blindly to HTTP/1.0 or MMS streams.
//! - Global Rule 3: bounded queue buffers and prompt subscriber resource release upon disconnect.

pub mod playlist;
pub mod router;
pub mod streamer;

pub use playlist::{
    escape_xml, sanitize_single_line, PlaylistBuilder, PlaylistEntry, PlaylistError, PlaylistFormat,
};
pub use router::{
    HttpMethod, HttpRequest, HttpVersion, RouteTarget, RoutingError,
};
pub use streamer::{
    build_http_200_headers, build_http_error, encode_chunked, encode_chunked_terminator,
    resolve_framing, resolve_mime_type, stream_media_to_client, HttpFraming, StreamerError,
};
