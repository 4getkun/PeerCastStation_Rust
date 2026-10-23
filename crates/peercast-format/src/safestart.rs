use peercast_core::cache::{BoundedContentCache, CacheClock, HeaderSnapshot, SystemCacheClock};
use peercast_core::content::Content;
use std::sync::Arc;
use std::time::Duration;

/// Default timeout (3 seconds) to wait for a video keyframe before falling back (REQ-055).
pub const DEFAULT_SAFE_START_TIMEOUT: Duration = Duration::from_secs(3);

/// Format-aware safe-start mode (REQ-050, REQ-055).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SafeStartMode {
    /// Format has video with keyframes (e.g. FLV AVC/VP6, MKV, MP4).
    /// Requires an actual keyframe before admitting, distinguishing keyframes
    /// from interframes even when cont == 0.
    VideoKeyframe,

    /// Audio-only format (e.g. MP3, AAC audio-only, Ogg Vorbis).
    /// Safe start point is any unfragmented packet boundary; never waits for video keyframes.
    AudioOnly,

    /// Container without codec keyframe markers (e.g. MPEG-2 TS with PAT/PMT, ASF).
    /// Safe start point is any unfragmented packet boundary (cont == 0).
    AnyUnfragmentedBoundary,

    /// Raw byte stream without container or packet boundaries.
    Raw,
}

/// Configuration for safe-start selection.
#[derive(Debug, Clone)]
pub struct SafeStartConfig {
    pub mode: SafeStartMode,
    pub max_wait: Duration,
}

impl Default for SafeStartConfig {
    fn default() -> Self {
        Self {
            mode: SafeStartMode::VideoKeyframe,
            max_wait: DEFAULT_SAFE_START_TIMEOUT,
        }
    }
}

/// State machine phase for safe-start selection (Global Rule 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SafeStartPhase {
    /// Waiting for active stream header snapshot
    WaitingForHeader,

    /// Header established; actively searching for format-aware safe start point
    SeekingSafePoint {
        epoch: i32,
        started_at: Duration,
    },

    /// Safe start point identified and admitted; streaming actively
    Streaming {
        epoch: i32,
        started_at_pos: i64,
    },
}

/// Decision made by `SafeStartSelector` for an evaluated packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SafeStartDecision {
    /// Skip this packet (e.g. interframe before keyframe, or fragment slice)
    Skip {
        reason: &'static str,
    },

    /// Header snapshot ready to emit before media packets
    EmitHeader(HeaderSnapshot),

    /// Packet admitted as safe-start point or continuous stream data
    Admit,

    /// Bounded timeout expired: fallback admitted this packet despite not being a keyframe
    FallbackAdmit {
        reason: &'static str,
    },
}

/// Inspects whether raw payload starts with an FLV video keyframe tag.
pub fn inspect_flv_keyframe(data: &[u8]) -> bool {
    // FLV tag header is 11 bytes: byte 0 is type (0x09 = Video)
    if data.len() < 12 {
        return false;
    }
    let tag_type = data[0] & 0x1F;
    if tag_type != 9 {
        return false;
    }
    // Tag body byte 0 is at offset 11: upper 4 bits are FrameType
    let frame_type = data[11] >> 4;
    frame_type == 1 || frame_type == 4 // 1 = Keyframe, 4 = Generated keyframe
}

/// Format-aware safe-start selector enforcing REQ-050 and REQ-055.
///
/// Guarantees:
/// - Never treats `cont == 0` as universal proof of H.264 IDR (REQ-050).
/// - Skips packet fragments (`cont & FRAGMENT != 0`) from starting playback.
/// - Does not wait indefinitely for keyframes on audio-only or timed-out video (REQ-055).
/// - Resets safe-start selection upon epoch/format changes (REQ-051, REQ-055).
pub struct SafeStartSelector {
    config: SafeStartConfig,
    phase: SafeStartPhase,
    clock: Arc<dyn CacheClock>,
}

impl SafeStartSelector {
    pub fn new(config: SafeStartConfig) -> Self {
        Self {
            config,
            phase: SafeStartPhase::WaitingForHeader,
            clock: Arc::new(SystemCacheClock::new()),
        }
    }

    pub fn with_clock(config: SafeStartConfig, clock: Arc<dyn CacheClock>) -> Self {
        Self {
            config,
            phase: SafeStartPhase::WaitingForHeader,
            clock,
        }
    }

    pub fn phase(&self) -> SafeStartPhase {
        self.phase
    }

    pub fn mode(&self) -> SafeStartMode {
        self.config.mode
    }

    /// Handles a new or changed stream header snapshot (REQ-051, REQ-055).
    ///
    /// Resets the selector state to `SeekingSafePoint` for the new epoch.
    pub fn on_header(&mut self, header: &HeaderSnapshot) -> SafeStartDecision {
        let now = self.clock.now();
        self.phase = SafeStartPhase::SeekingSafePoint {
            epoch: header.epoch,
            started_at: now,
        };
        SafeStartDecision::EmitHeader(header.clone())
    }

    /// Evaluates an incoming media packet against format-aware safe start rules.
    ///
    /// An explicit `is_keyframe_hint` can be provided by container demuxers (e.g. FLV / MKV).
    pub fn evaluate(
        &mut self,
        content: &Content,
        is_keyframe_hint: Option<bool>,
    ) -> SafeStartDecision {
        match self.phase {
            SafeStartPhase::WaitingForHeader => {
                SafeStartDecision::Skip {
                    reason: "Awaiting channel header snapshot before admitting content",
                }
            }

            SafeStartPhase::Streaming {
                epoch,
                started_at_pos: _,
            } => {
                if content.stream_id != epoch {
                    // Epoch changed without explicit on_header reset; invalidate streaming state
                    self.phase = SafeStartPhase::WaitingForHeader;
                    return SafeStartDecision::Skip {
                        reason: "Epoch changed during streaming; awaiting new header snapshot",
                    };
                }
                SafeStartDecision::Admit
            }

            SafeStartPhase::SeekingSafePoint { epoch, started_at } => {
                if content.stream_id != epoch {
                    return SafeStartDecision::Skip {
                        reason: "Content epoch does not match current seeking epoch",
                    };
                }

                // Rule 1: A continuation fragment slice can NEVER be a safe start point!
                if content.cont_flags.is_fragment() {
                    return SafeStartDecision::Skip {
                        reason: "Cannot start playback on a packet continuation fragment",
                    };
                }

                // Rule 2: Audio-only and raw formats do not wait for video keyframes (REQ-055)
                if matches!(
                    self.config.mode,
                    SafeStartMode::AudioOnly
                        | SafeStartMode::AnyUnfragmentedBoundary
                        | SafeStartMode::Raw
                ) {
                    self.phase = SafeStartPhase::Streaming {
                        epoch,
                        started_at_pos: content.pos,
                    };
                    return SafeStartDecision::Admit;
                }

                // Rule 3: Video formats require a true keyframe, NOT merely cont == 0 (REQ-050)!
                let is_keyframe = is_keyframe_hint
                    .unwrap_or_else(|| inspect_flv_keyframe(&content.data));

                if is_keyframe {
                    self.phase = SafeStartPhase::Streaming {
                        epoch,
                        started_at_pos: content.pos,
                    };
                    return SafeStartDecision::Admit;
                }

                // Rule 4: Check bounded timeout to avoid indefinite waiting (REQ-055)
                let now = self.clock.now();
                let elapsed = now.saturating_sub(started_at);
                if elapsed >= self.config.max_wait {
                    self.phase = SafeStartPhase::Streaming {
                        epoch,
                        started_at_pos: content.pos,
                    };
                    return SafeStartDecision::FallbackAdmit {
                        reason: "Keyframe wait timeout exceeded; falling back to unfragmented packet",
                    };
                }

                // Interframe skipped while within bounded wait period
                SafeStartDecision::Skip {
                    reason: "Interframe skipped waiting for true video keyframe",
                }
            }
        }
    }

    /// Selects the best safe start point from a `BoundedContentCache` (REQ-055).
    ///
    /// For video streams: finds the most recent unfragmented keyframe in the epoch.
    /// If no keyframe is available: falls back to the most recent unfragmented packet.
    /// For audio-only streams: finds the most recent unfragmented packet.
    pub fn select_cache_start_point<'a, C: CacheClock>(
        &self,
        cache: &'a BoundedContentCache<C>,
        epoch: i32,
    ) -> Option<&'a Content> {
        let matching_epoch_items: Vec<&Content> = cache
            .iter_body()
            .filter(|c| c.stream_id == epoch && !c.cont_flags.is_fragment())
            .collect();

        if matching_epoch_items.is_empty() {
            return None;
        }

        if self.config.mode == SafeStartMode::VideoKeyframe {
            // Scan backwards from newest to oldest for a keyframe
            for &item in matching_epoch_items.iter().rev() {
                if inspect_flv_keyframe(&item.data) {
                    return Some(item);
                }
            }
        }

        // Fallback or AudioOnly: newest unfragmented packet
        matching_epoch_items.last().copied()
    }
}
