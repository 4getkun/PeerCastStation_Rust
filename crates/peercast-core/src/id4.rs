use std::fmt;
use thiserror::Error;

#[derive(Error, Debug, PartialEq, Eq)]
pub enum Id4Error {
    #[error("ID4 string length cannot exceed 4 bytes (got {0} bytes)")]
    TooLong(usize),
}

/// A 4-byte FourCC identifier representing an Atom tag or protocol message type.
///
/// In PeerCast wire protocol, an ID4 is strictly 4 raw bytes.
/// Identifiers shorter than 4 characters are NUL-padded (`\0`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ID4(pub [u8; 4]);

impl ID4 {
    /// Creates an ID4 from a 4-byte array.
    pub const fn from_bytes(bytes: [u8; 4]) -> Self {
        ID4(bytes)
    }

    /// Creates an ID4 from an ASCII / UTF-8 string slice, right-padding with `\0` up to 4 bytes.
    ///
    /// Fails if `s.as_bytes().len() > 4`.
    pub fn from_str_padded(s: &str) -> Result<Self, Id4Error> {
        let b = s.as_bytes();
        if b.len() > 4 {
            return Err(Id4Error::TooLong(b.len()));
        }
        let mut buf = [0u8; 4];
        buf[..b.len()].copy_from_slice(b);
        Ok(ID4(buf))
    }

    /// Returns the underlying 4 bytes.
    pub const fn as_bytes(&self) -> &[u8; 4] {
        &self.0
    }

    /// Converts to byte array.
    pub const fn to_bytes(&self) -> [u8; 4] {
        self.0
    }
}

// Standard PCP Wire Identifiers
impl ID4 {
    /// Preamble for raw PCP connection (4 bytes: 'p', 'c', 'p', '\n')
    pub const PCP_CONNECT: ID4 = ID4(*b"pcp\n");

    pub const PCP_OK: ID4 = ID4(*b"ok\0\0");
    pub const PCP_HELO: ID4 = ID4(*b"helo");
    pub const PCP_OLEH: ID4 = ID4(*b"oleh");
    pub const PCP_QUIT: ID4 = ID4(*b"quit");
    pub const PCP_CHAN: ID4 = ID4(*b"chan");
    pub const PCP_BCST: ID4 = ID4(*b"bcst");
    pub const PCP_HOST: ID4 = ID4(*b"host");
    pub const PCP_ROOT: ID4 = ID4(*b"root");

    // Helo / Oleh field tags
    pub const HELO_AGENT: ID4 = ID4(*b"agnt");
    pub const HELO_OSTYPE: ID4 = ID4(*b"ostp");
    pub const HELO_SESSIONID: ID4 = ID4(*b"sid\0");
    pub const HELO_PORT: ID4 = ID4(*b"port");
    pub const HELO_PING: ID4 = ID4(*b"ping");
    pub const HELO_PONG: ID4 = ID4(*b"pong");
    pub const HELO_REMOTEIP: ID4 = ID4(*b"rip\0");
    pub const HELO_VERSION: ID4 = ID4(*b"ver\0");
    pub const HELO_BCID: ID4 = ID4(*b"bcid");
    pub const HELO_DISABLE: ID4 = ID4(*b"dis\0");

    // Channel tags
    pub const CHAN_ID: ID4 = ID4(*b"id\0\0");
    pub const CHAN_BCID: ID4 = ID4(*b"bcid");
    pub const CHAN_PKT: ID4 = ID4(*b"pkt\0");
    pub const CHAN_INFO: ID4 = ID4(*b"info");
    pub const CHAN_TRCK: ID4 = ID4(*b"trck");

    // Channel Info tags
    pub const INFO_NAME: ID4 = ID4(*b"name");
    pub const INFO_GENRE: ID4 = ID4(*b"gnre");
    pub const INFO_DESC: ID4 = ID4(*b"desc");
    pub const INFO_URL: ID4 = ID4(*b"url\0");
    pub const INFO_BITR: ID4 = ID4(*b"bitr");
    pub const INFO_COMMENT: ID4 = ID4(*b"cmnt");
    pub const INFO_TYPE: ID4 = ID4(*b"type");

    // Channel Track tags
    pub const TRCK_NAME: ID4 = ID4(*b"name");
    pub const TRCK_CREATOR: ID4 = ID4(*b"auth");
    pub const TRCK_URL: ID4 = ID4(*b"url\0");

    // Packet tags
    pub const PKT_TYPE: ID4 = ID4(*b"type");
    pub const PKT_HEAD: ID4 = ID4(*b"head");
    pub const PKT_META: ID4 = ID4(*b"meta");
    pub const PKT_DATA: ID4 = ID4(*b"data");
    pub const PKT_POS: ID4 = ID4(*b"pos\0");
    pub const PKT_CONT: ID4 = ID4(*b"cont");

    // Packet type values (payload of PKT_TYPE leaf)
    pub const PKT_TYPE_HEAD: ID4 = ID4(*b"head");
    pub const PKT_TYPE_DATA: ID4 = ID4(*b"data");
    pub const PKT_TYPE_META: ID4 = ID4(*b"meta");

    // Host tags
    pub const HOST_ID: ID4 = ID4(*b"id\0\0");
    pub const HOST_IP: ID4 = ID4(*b"ip\0\0");
    pub const HOST_PORT: ID4 = ID4(*b"port");
    pub const HOST_CHANID: ID4 = ID4(*b"cid\0");
    pub const HOST_NUML: ID4 = ID4(*b"numl");
    pub const HOST_NUMR: ID4 = ID4(*b"numr");
    pub const HOST_UPTIME: ID4 = ID4(*b"uptm");
    pub const HOST_TRACKER: ID4 = ID4(*b"trkr");
    pub const HOST_VERSION: ID4 = ID4(*b"ver\0");
    pub const HOST_VER_VP: ID4 = ID4(*b"vevp");
    pub const HOST_VER_EXP: ID4 = ID4(*b"vexp");
    pub const HOST_VER_EXN: ID4 = ID4(*b"vexn");
    pub const HOST_FLAGS1: ID4 = ID4(*b"flg1");
    pub const HOST_OLDPOS: ID4 = ID4(*b"oldp");
    pub const HOST_NEWPOS: ID4 = ID4(*b"newp");
    pub const HOST_UPIP: ID4 = ID4(*b"upip");
    pub const HOST_UPPT: ID4 = ID4(*b"uppt");
    pub const HOST_UPHP: ID4 = ID4(*b"uphp");

    // Broadcast tags
    pub const BCST_TTL: ID4 = ID4(*b"ttl\0");
    pub const BCST_HOPS: ID4 = ID4(*b"hops");
    pub const BCST_FROM: ID4 = ID4(*b"from");
    pub const BCST_DEST: ID4 = ID4(*b"dest");
    pub const BCST_GRP: ID4 = ID4(*b"grp\0");
    pub const BCST_CID: ID4 = ID4(*b"cid\0");
    pub const BCST_VERS: ID4 = ID4(*b"vers");
    pub const BCST_VRVP: ID4 = ID4(*b"vrvp");
    pub const BCST_VEXP: ID4 = ID4(*b"vexp");
    pub const BCST_VEXN: ID4 = ID4(*b"vexn");

    // Root tags
    pub const ROOT_UPDINT: ID4 = ID4(*b"uint");
    pub const ROOT_CHECKVER: ID4 = ID4(*b"chkv");
    pub const ROOT_URL: ID4 = ID4(*b"url\0");
    pub const ROOT_UPDATE: ID4 = ID4(*b"upd\0");
    pub const ROOT_NEXT: ID4 = ID4(*b"next");
}

impl fmt::Debug for ID4 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ID4({:?} [0x{:02x}, 0x{:02x}, 0x{:02x}, 0x{:02x}])",
            String::from_utf8_lossy(&self.0),
            self.0[0],
            self.0[1],
            self.0[2],
            self.0[3]
        )
    }
}

impl fmt::Display for ID4 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Display as ASCII if printable, else hex
        for &b in &self.0 {
            if b.is_ascii_graphic() {
                write!(f, "{}", b as char)?;
            } else if b == b'\n' {
                write!(f, "\\n")?;
            } else if b == 0 {
                write!(f, "\\0")?;
            } else {
                write!(f, "\\x{:02x}", b)?;
            }
        }
        Ok(())
    }
}
