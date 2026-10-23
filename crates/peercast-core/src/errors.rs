use std::fmt;

/// PeerCast PCP wire Quit Reason codes.
///
/// REQ-038: The wire quit code preserves the complete raw integer.
/// The standard quit reason codes are offset by `BASE_QUIT` (1000) with reasons 0..11:
/// normal (1000), skip (1001), already connected (1002), unavailable (1003),
/// loopback (1004), not identified (1005), bad response (1006), bad agent (1007),
/// off air (1008), shutdown (1009), no root (1010), banned (1011).
///
/// Other error bases observed on the wire:
/// - BCST: 2000
/// - READ: 3000
/// - WRITE: 4000
/// - GENERAL: 5000
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct QuitReason(pub u32);

impl QuitReason {
    // Error code category bases
    pub const BASE_QUIT: u32 = 1000;
    pub const BASE_BCST: u32 = 2000;
    pub const BASE_READ: u32 = 3000;
    pub const BASE_WRITE: u32 = 4000;
    pub const BASE_GENERAL: u32 = 5000;

    // Standard quit reason codes (1000 + 0..11)
    pub const NORMAL: QuitReason = QuitReason(1000);
    pub const SKIP: QuitReason = QuitReason(1001);
    pub const ALREADY_CONNECTED: QuitReason = QuitReason(1002);
    pub const UNAVAILABLE: QuitReason = QuitReason(1003);
    pub const LOOPBACK: QuitReason = QuitReason(1004);
    pub const NOT_IDENTIFIED: QuitReason = QuitReason(1005);
    pub const BAD_RESPONSE: QuitReason = QuitReason(1006);
    pub const BAD_AGENT: QuitReason = QuitReason(1007);
    pub const OFF_AIR: QuitReason = QuitReason(1008);
    pub const SHUTDOWN: QuitReason = QuitReason(1009);
    pub const NO_ROOT: QuitReason = QuitReason(1010);
    pub const BANNED: QuitReason = QuitReason(1011);

    // Read / Write / General errors
    pub const READ_ERROR: QuitReason = QuitReason(3000);
    pub const WRITE_ERROR: QuitReason = QuitReason(4000);

    /// Returns the raw integer value as seen on the wire.
    #[inline]
    pub const fn raw(&self) -> u32 {
        self.0
    }

    /// Constructs a QuitReason from a raw wire integer value.
    #[inline]
    pub const fn from_raw(val: u32) -> Self {
        QuitReason(val)
    }

    /// Returns a human-readable description for known quit codes.
    pub fn description(&self) -> &'static str {
        match self.0 {
            1000 => "Normal disconnect",
            1001 => "Channel skip / bandwidth exceeded",
            1002 => "Already connected",
            1003 => "Relay unavailable / node full",
            1004 => "Loopback detected",
            1005 => "Not identified / auth failed",
            1006 => "Bad response",
            1007 => "Bad agent / incompatible version",
            1008 => "Channel off-air",
            1009 => "Servent shutting down",
            1010 => "No root connection",
            1011 => "Banned by host",
            2000 => "Broadcast error (2000)",
            3000 => "Socket read error (3000)",
            4000 => "Socket write error (4000)",
            5000 => "General error (5000)",
            _ => "Unknown quit code",
        }
    }
}

impl fmt::Debug for QuitReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "QuitReason({}, \"{}\")", self.0, self.description())
    }
}

impl fmt::Display for QuitReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.description())
    }
}
