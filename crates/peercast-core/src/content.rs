use bytes::Bytes;
use std::time::Duration;

/// PCP channel packet continuation attributes (`cont` tag).
///
/// REQ-050: A PCP continuation flag is a packet-level hint, not a universal H.264 IDR detector.
/// Preserved as incoming raw bitflags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ContinuationFlags(pub u8);

impl ContinuationFlags {
    pub const NONE: ContinuationFlags = ContinuationFlags(0x00);
    pub const FRAGMENT: ContinuationFlags = ContinuationFlags(0x01);
    pub const INTER_FRAME: ContinuationFlags = ContinuationFlags(0x02);
    pub const AUDIO_FRAME: ContinuationFlags = ContinuationFlags(0x04);

    #[inline]
    pub fn is_fragment(&self) -> bool {
        (self.0 & Self::FRAGMENT.0) != 0
    }

    #[inline]
    pub fn is_inter_frame(&self) -> bool {
        (self.0 & Self::INTER_FRAME.0) != 0
    }

    #[inline]
    pub fn is_audio_frame(&self) -> bool {
        (self.0 & Self::AUDIO_FRAME.0) != 0
    }

    #[inline]
    pub fn raw(&self) -> u8 {
        self.0
    }
}

/// A media content chunk representing a slice of the broadcast stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Content {
    /// Stream generation/epoch number.
    pub stream_id: i32,
    /// Logical playback timestamp relative to stream start.
    pub timestamp: Duration,
    /// Absolute stream byte offset (position).
    pub pos: i64,
    /// Packet continuation flags.
    pub cont_flags: ContinuationFlags,
    /// Raw media chunk payload.
    pub data: Bytes,
    /// Monotonic serial order assigned upon insertion into the channel buffer.
    pub serial: i64,
}

impl Content {
    pub fn new(
        stream_id: i32,
        timestamp: Duration,
        pos: i64,
        data: impl Into<Bytes>,
        cont_flags: ContinuationFlags,
    ) -> Self {
        Self {
            stream_id,
            timestamp,
            pos,
            cont_flags,
            data: data.into(),
            serial: -1,
        }
    }

    pub fn with_serial(mut self, serial: i64) -> Self {
        self.serial = serial;
        self
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.data.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}
