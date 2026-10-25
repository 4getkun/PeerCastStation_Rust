use md5::Digest;
use sha2::Sha512;
use std::fmt;
use thiserror::Error;

#[derive(Error, Debug, PartialEq, Eq, Clone)]
pub enum ChannelIdError {
    #[error("Hex string must be exactly 32 characters (got {0})")]
    InvalidHexLength(usize),
    #[error("Invalid hex digit: {0}")]
    InvalidHexDigit(char),
}

/// A 16-byte PeerCast Channel identifier or Session/Broadcast identifier.
///
/// Internally stored in canonical big-endian (network / hex) byte order:
/// e.g. "0123456789abcdef0123456789abcdef" maps directly to `bytes[0..16]`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct ChannelId(pub [u8; 16]);

impl ChannelId {
    /// Creates a ChannelId from canonical 16 bytes.
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        ChannelId(bytes)
    }

    /// Returns canonical 16 bytes.
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    /// Converts canonical 16 bytes to .NET Guid.ToByteArray() byte layout.
    ///
    /// .NET Guid layout reverses:
    /// - bytes 0..4 (little-endian u32)
    /// - bytes 4..6 (little-endian u16)
    /// - bytes 6..8 (little-endian u16)
    /// - bytes 8..16 remain in identical order.
    pub fn to_dotnet_guid_bytes(&self) -> [u8; 16] {
        let mut b = self.0;
        b[0..4].reverse();
        b[4..6].reverse();
        b[6..8].reverse();
        b
    }

    /// Creates a ChannelId from .NET Guid.ToByteArray() bytes.
    pub fn from_dotnet_guid_bytes(b: [u8; 16]) -> Self {
        let mut canonical = b;
        canonical[0..4].reverse();
        canonical[4..6].reverse();
        canonical[6..8].reverse();
        ChannelId(canonical)
    }

    /// Parses a 32-character hexadecimal string into a ChannelId.
    pub fn from_hex(s: &str) -> Result<Self, ChannelIdError> {
        if s.len() != 32 {
            return Err(ChannelIdError::InvalidHexLength(s.len()));
        }
        let mut bytes = [0u8; 16];
        for (i, byte) in bytes.iter_mut().enumerate() {
            let high = s.as_bytes()[i * 2] as char;
            let low = s.as_bytes()[i * 2 + 1] as char;
            let h = high
                .to_digit(16)
                .ok_or(ChannelIdError::InvalidHexDigit(high))?;
            let l = low
                .to_digit(16)
                .ok_or(ChannelIdError::InvalidHexDigit(low))?;
            *byte = ((h << 4) | l) as u8;
        }
        Ok(ChannelId(bytes))
    }

    /// Formats as 32-character uppercase hex string (e.g. for display and query URLs).
    pub fn to_hex_upper(&self) -> String {
        let mut s = String::with_capacity(32);
        for &b in &self.0 {
            use std::fmt::Write;
            write!(&mut s, "{:02X}", b).unwrap();
        }
        s
    }

    /// Formats as 32-character lowercase hex string.
    pub fn to_hex_lower(&self) -> String {
        let mut s = String::with_capacity(32);
        for &b in &self.0 {
            use std::fmt::Write;
            write!(&mut s, "{:02x}", b).unwrap();
        }
        s
    }

    /// Returns true if all 16 bytes are zero.
    #[inline]
    pub fn is_zero(&self) -> bool {
        self.0 == [0u8; 16]
    }
}

impl fmt::Debug for ChannelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ChannelId({})", self.to_hex_upper())
    }
}

impl fmt::Display for ChannelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_hex_upper())
    }
}

/// Network type selector for Station-compatible ChannelID derivation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelNetworkType {
    IPv4 = 0,
    IPv6 = 1,
}

/// Helper to write C# BinaryWriter 7-bit encoded length integer.
fn write_7bit_encoded_int(buf: &mut Vec<u8>, mut val: usize) {
    while val >= 0x80 {
        buf.push((val | 0x80) as u8);
        val >>= 7;
    }
    buf.push(val as u8);
}

/// Writes a string using C# BinaryWriter string format:
/// 7-bit encoded UTF-8 byte count, followed by UTF-8 bytes without NUL terminator.
fn write_binary_writer_string(buf: &mut Vec<u8>, s: &str) {
    let b = s.as_bytes();
    write_7bit_encoded_int(buf, b.len());
    buf.extend_from_slice(b);
}

/// Derives a Station-compatible deterministic ChannelID for a new broadcast channel.
///
/// Follows REQ-040 & REQ-041:
/// 1. Hash .NET-layout BCID bytes using SHA-512.
/// 2. If network is IPv4, append NO network prefix. If IPv6, prefix little-endian i32 value 1.
/// 3. Append the 64 hash bytes without length prefix.
/// 4. Append name, genre, source using C# BinaryWriter string encoding.
/// 5. Compute MD5 digest. Interpret digest as .NET Guid(byte[]) and convert to canonical ChannelId.
pub fn derive_broadcast_channel_id(
    broadcast_id: &ChannelId,
    network: ChannelNetworkType,
    name: &str,
    genre: &str,
    source: &str,
) -> ChannelId {
    // Step 1: SHA512 of .NET-layout BCID bytes
    let dotnet_bcid = broadcast_id.to_dotnet_guid_bytes();
    let mut hasher = Sha512::new();
    hasher.update(dotnet_bcid);
    let bcid_hash: [u8; 64] = hasher.finalize().into();

    // Step 2-4: Concatenate buffer
    let mut buf = Vec::new();
    if network == ChannelNetworkType::IPv6 {
        buf.extend_from_slice(&1i32.to_le_bytes());
    }
    buf.extend_from_slice(&bcid_hash);
    write_binary_writer_string(&mut buf, name);
    write_binary_writer_string(&mut buf, genre);
    write_binary_writer_string(&mut buf, source);

    // Step 5: MD5 digest interpreted as .NET Guid(byte[])
    let mut md5_hasher = md5::Md5::new();
    md5_hasher.update(&buf);
    let md5_digest: [u8; 16] = md5_hasher.finalize().into();

    ChannelId::from_dotnet_guid_bytes(md5_digest)
}
