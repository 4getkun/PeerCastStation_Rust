use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use thiserror::Error;

#[derive(Error, Debug, PartialEq, Eq)]
pub enum ScalarError {
    #[error("Exact width mismatch: expected {expected} bytes, got {actual}")]
    WidthMismatch { expected: usize, actual: usize },
    #[error("Invalid IP address byte length: expected 4 or 16 bytes, got {0}")]
    InvalidIpLength(usize),
    #[error("String is missing required terminating NUL byte")]
    MissingTerminatingNul,
    #[error("Invalid UTF-8 sequence: {0}")]
    InvalidUtf8(#[from] std::str::Utf8Error),
}

/// Helpers for reading and writing fixed-width little-endian scalars from/to byte slices.
pub struct Scalar;

impl Scalar {
    #[inline]
    pub fn read_u8(bytes: &[u8]) -> Result<u8, ScalarError> {
        if bytes.len() != 1 {
            return Err(ScalarError::WidthMismatch { expected: 1, actual: bytes.len() });
        }
        Ok(bytes[0])
    }

    #[inline]
    pub fn write_u8(val: u8) -> [u8; 1] {
        [val]
    }

    #[inline]
    pub fn read_u16_le(bytes: &[u8]) -> Result<u16, ScalarError> {
        if bytes.len() != 2 {
            return Err(ScalarError::WidthMismatch { expected: 2, actual: bytes.len() });
        }
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    #[inline]
    pub fn write_u16_le(val: u16) -> [u8; 2] {
        val.to_le_bytes()
    }

    #[inline]
    pub fn read_u32_le(bytes: &[u8]) -> Result<u32, ScalarError> {
        if bytes.len() != 4 {
            return Err(ScalarError::WidthMismatch { expected: 4, actual: bytes.len() });
        }
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    #[inline]
    pub fn write_u32_le(val: u32) -> [u8; 4] {
        val.to_le_bytes()
    }

    #[inline]
    pub fn read_u64_le(bytes: &[u8]) -> Result<u64, ScalarError> {
        if bytes.len() != 8 {
            return Err(ScalarError::WidthMismatch { expected: 8, actual: bytes.len() });
        }
        Ok(u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3],
            bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }

    #[inline]
    pub fn write_u64_le(val: u64) -> [u8; 8] {
        val.to_le_bytes()
    }

    /// Reads a UTF-8 string that must end with a terminating NUL (`\0`) byte.
    /// Returns the string slice excluding the trailing NUL.
    pub fn read_nul_terminated_utf8(bytes: &[u8]) -> Result<&str, ScalarError> {
        if bytes.is_empty() || bytes[bytes.len() - 1] != 0 {
            return Err(ScalarError::MissingTerminatingNul);
        }
        // Slice excluding trailing NUL
        let s = std::str::from_utf8(&bytes[..bytes.len() - 1])?;
        Ok(s)
    }

    /// Encodes a UTF-8 string with a terminating NUL (`\0`) byte.
    pub fn write_nul_terminated_utf8(s: &str) -> Vec<u8> {
        let mut v = Vec::with_capacity(s.len() + 1);
        v.extend_from_slice(s.as_bytes());
        v.push(0);
        v
    }

    /// Reads an IPv4 or IPv6 address according to C# PeerCastStation reversal convention.
    ///
    /// C# reverses the complete network-order byte array when writing an IP Atom:
    /// IPv4: 4 bytes reversed.
    /// IPv6: 16 bytes reversed.
    pub fn read_ip_reversed(bytes: &[u8]) -> Result<IpAddr, ScalarError> {
        match bytes.len() {
            4 => {
                let mut octets = [0u8; 4];
                octets.copy_from_slice(bytes);
                octets.reverse();
                Ok(IpAddr::V4(Ipv4Addr::from(octets)))
            }
            16 => {
                let mut octets = [0u8; 16];
                octets.copy_from_slice(bytes);
                octets.reverse();
                Ok(IpAddr::V6(Ipv6Addr::from(octets)))
            }
            other => Err(ScalarError::InvalidIpLength(other)),
        }
    }

    /// Writes an IP address reversed according to C# PeerCastStation convention.
    pub fn write_ip_reversed(ip: &IpAddr) -> Vec<u8> {
        match ip {
            IpAddr::V4(v4) => {
                let mut octets = v4.octets();
                octets.reverse();
                octets.to_vec()
            }
            IpAddr::V6(v6) => {
                let mut octets = v6.octets();
                octets.reverse();
                octets.to_vec()
            }
        }
    }
}
