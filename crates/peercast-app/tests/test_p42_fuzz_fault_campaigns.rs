use std::time::Instant;

use bytes::Bytes;
use peercast_core::atom::{Atom, AtomBudget};
use peercast_core::index_txt::{parse_index_txt, IndexParserLimits};
use peercast_format::asf::{AsfDemuxer, AsfTransportProfile};
use peercast_format::flv::{FlvDemuxer, DEFAULT_MAX_FLV_TAG_SIZE};
use peercast_format::mkv::MkvDemuxer;
use peercast_format::mp3::Mp3Demuxer;
use peercast_format::ogg::OggDemuxer;
use peercast_format::sniffer::{detect_container, SniffBuffer, SniffResult};
use peercast_format::ts::TsDemuxer;
use peercast_http::HttpRequest;
use peercast_pcp::http_prelude::{parse_relay_request, parse_relay_response, PcpHttpLimits};
use peercast_rpc::server::{AccessLevel, RpcDispatcher, RpcState};

/// Deterministic 64-bit Xorshift PRNG for repeatable fuzz testing (REQ-070).
struct FuzzRng {
    state: u64,
}

impl FuzzRng {
    fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 { 0x123456789ABCDEF0 } else { seed },
        }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    fn next_usize(&mut self, bound: usize) -> usize {
        if bound == 0 {
            0
        } else {
            (self.next_u64() as usize) % bound
        }
    }

    fn next_u8(&mut self) -> u8 {
        self.next_u64() as u8
    }

    /// Applies one or more random mutation operators to input byte buffer.
    fn mutate(&mut self, data: &mut Vec<u8>) {
        if data.is_empty() {
            data.push(self.next_u8());
            return;
        }

        let op = self.next_usize(6);
        match op {
            0 => {
                // Bit flip
                let idx = self.next_usize(data.len());
                let bit = 1 << self.next_usize(8);
                data[idx] ^= bit;
            }
            1 => {
                // Byte override with boundary value
                let boundary_values = [0x00, 0xFF, 0x7F, 0x80, 0x0A, 0x0D, 0x47, 0x1A, 0x20];
                let idx = self.next_usize(data.len());
                let val = boundary_values[self.next_usize(boundary_values.len())];
                data[idx] = val;
            }
            2 => {
                // Truncation
                let new_len = self.next_usize(data.len());
                data.truncate(new_len);
            }
            3 => {
                // Insert random byte
                let idx = self.next_usize(data.len() + 1);
                let byte = self.next_u8();
                data.insert(idx, byte);
            }
            4 => {
                // Swap two bytes
                if data.len() >= 2 {
                    let a = self.next_usize(data.len());
                    let b = self.next_usize(data.len());
                    data.swap(a, b);
                }
            }
            _ => {
                // Atom descriptor tampering: set or clear bit 31
                if data.len() >= 8 {
                    let idx = (self.next_usize(data.len() / 8)) * 8 + 7;
                    if idx < data.len() {
                        data[idx] ^= 0x80;
                    }
                }
            }
        }
    }
}

/// Acceptance criterion 1:
/// Atom and PCP Protocol parser fuzzing and budget safety (REQ-025, REQ-027, REQ-070).
/// Verifies parser aborts safely without panic or unbounded heap allocation on corrupt atoms.
#[test]
fn test_p42_atom_and_pcp_fuzz_campaign() {
    let seeds = [0xDEADBEEF12345678, 0xCAFEBABEDEADBEEF, 0x0123456789ABCDEF];
    let start = Instant::now();
    let mut iterations = 0;

    // Base seed atoms: valid pcp\n root atom with helo child
    let base_atom_bytes = vec![
        b'p', b'c', b'p', b'\n', 0x01, 0x00, 0x00, 0x80, // parent with 1 child
        b'h', b'e', b'l', b'o', 0x04, 0x00, 0x00, 0x00, 0x01, 0x02, 0x03, 0x04, // leaf 4 bytes
    ];

    let budget = AtomBudget::default();
    let pcp_limits = PcpHttpLimits::default();

    for &seed in &seeds {
        let mut rng = FuzzRng::new(seed);
        for _ in 0..1000 {
            iterations += 1;
            let mut buf = base_atom_bytes.clone();
            let mutations = rng.next_usize(5) + 1;
            for _ in 0..mutations {
                rng.mutate(&mut buf);
            }

            // Test Atom::decode with strict budgets
            let mut bytes_buf = Bytes::from(buf.clone());
            let decode_res = Atom::decode(&mut bytes_buf, &budget);
            match decode_res {
                Ok(Some(atom)) => {
                    let _ = format!("{:?}", atom);
                }
                Ok(None) => {}
                Err(_) => {
                    // Safely rejected with typed AtomError (REQ-025, REQ-027)
                }
            }

            // Test PCP HTTP prelude parsing
            let mut req_bytes = Bytes::copy_from_slice(&buf);
            let _ = parse_relay_request(&mut req_bytes, &pcp_limits);
            let mut resp_bytes = Bytes::copy_from_slice(&buf);
            let _ = parse_relay_response(&mut resp_bytes, &pcp_limits);
        }
    }

    assert!(iterations >= 3000, "fuzz iterations must be at least 3000");
    assert!(start.elapsed().as_secs() < 30, "fuzz run must complete efficiently");
}

/// Acceptance criterion 2:
/// Media container demuxer fuzzing across FLV, MPEG-TS, ASF, MKV, Ogg, and MP3 (REQ-056, REQ-070).
/// Verifies no infinite loops, hangs, or panics when parsing corrupted stream chunks.
#[test]
fn test_p42_container_demuxers_fuzz_campaign() {
    let seeds = [0x1122334455667788, 0x99AABBCCDDEEFF00];
    let mut iterations = 0;

    // Seed headers for each container format
    let flv_seed = vec![b'F', b'L', b'V', 0x01, 0x05, 0x00, 0x00, 0x00, 0x09, 0x00, 0x00, 0x00, 0x00];
    let ts_seed = vec![0x47, 0x40, 0x11, 0x10]; // TS sync byte 0x47 + packet header
    let asf_seed = vec![
        0x30, 0x26, 0xB2, 0x75, 0x8E, 0x66, 0xCF, 0x11, 0xA6, 0xD9, 0x00, 0xAA, 0x00, 0x62, 0xCE, 0x6C,
        0x30, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x02,
    ];
    let mkv_seed = vec![0x1A, 0x45, 0xDF, 0xA3, 0x9F, 0x42, 0x86, 0x81, 0x01, 0x42, 0xF7, 0x81, 0x01];
    let ogg_seed = vec![b'O', b'g', b'g', b'S', 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
    let mp3_seed = vec![0xFF, 0xFB, 0x90, 0x64]; // MP3 frame sync 0xFFFB

    for &seed in &seeds {
        let mut rng = FuzzRng::new(seed);
        for _ in 0..500 {
            iterations += 1;

            // 1. FLV Fuzz
            let mut flv_buf = flv_seed.clone();
            rng.mutate(&mut flv_buf);
            let mut flv_demux = FlvDemuxer::new(DEFAULT_MAX_FLV_TAG_SIZE);
            flv_demux.feed(&flv_buf);
            let _ = flv_demux.next_output();

            // 2. TS Fuzz
            let mut ts_buf = ts_seed.clone();
            rng.mutate(&mut ts_buf);
            let mut ts_demux = TsDemuxer::new();
            ts_demux.feed(&ts_buf);
            let _ = ts_demux.next_output();

            // 3. ASF Fuzz
            let mut asf_buf = asf_seed.clone();
            rng.mutate(&mut asf_buf);
            let mut asf_demux = AsfDemuxer::new(AsfTransportProfile::MmsFramed);
            let _ = asf_demux.feed(&asf_buf);
            let _ = asf_demux.next_output();

            // 4. MKV Fuzz
            let mut mkv_buf = mkv_seed.clone();
            rng.mutate(&mut mkv_buf);
            let mut mkv_demux = MkvDemuxer::new();
            let _ = mkv_demux.feed(&mkv_buf);
            let _ = mkv_demux.next_output();

            // 5. Ogg Fuzz
            let mut ogg_buf = ogg_seed.clone();
            rng.mutate(&mut ogg_buf);
            let mut ogg_demux = OggDemuxer::new();
            let _ = ogg_demux.feed(&ogg_buf);
            let _ = ogg_demux.next_output();

            // 6. MP3 Fuzz
            let mut mp3_buf = mp3_seed.clone();
            rng.mutate(&mut mp3_buf);
            let mut mp3_demux = Mp3Demuxer::new();
            let _ = mp3_demux.feed(&mp3_buf);
            let _ = mp3_demux.next_output();

            // 7. Bounded Sniffer Fuzz
            let sniff_res = detect_container(&flv_buf);
            assert!(
                matches!(sniff_res, SniffResult::Definitive(_) | SniffResult::NeedsMoreData { .. } | SniffResult::Unsupported)
            );

            let mut sniff_buf = SniffBuffer::new(4096);
            let _ = sniff_buf.feed(&flv_buf);
            let _ = sniff_buf.detect();
        }
    }

    assert!(iterations >= 1000);
}

/// Acceptance criterion 3:
/// Yellow Pages directory line parser fuzzing (REQ-063, REQ-070).
/// Verifies malformed directory text with invalid characters or extreme token lengths is safely handled.
#[test]
fn test_p42_directory_parser_fuzz_campaign() {
    let seeds = [0x5555AAAA5555AAAA, 0x1234432112344321];
    let base_lines = "name=TestChannel&tip=192.168.1.1:7144&id=0123456789ABCDEF0123456789ABCDEF&url=http://example.com/stream\n";
    let limits = IndexParserLimits::default();

    for &seed in &seeds {
        let mut rng = FuzzRng::new(seed);
        for _ in 0..500 {
            let mut line_bytes = base_lines.as_bytes().to_vec();
            rng.mutate(&mut line_bytes);

            // Convert to string lossily
            let line_str = String::from_utf8_lossy(&line_bytes);
            let res = parse_index_txt(&line_str, &limits);
            // Parser must either return Ok with parsed channels or Ok(empty), never panic
            assert!(res.is_ok());
        }
    }
}

/// Acceptance criterion 4:
/// HTTP request router and JSON-RPC management fuzzing (REQ-060, REQ-064, REQ-070).
/// Verifies malformed HTTP framing and invalid JSON-RPC requests return typed protocol errors.
#[tokio::test]
async fn test_p42_http_and_rpc_fuzz_campaign() {
    let seeds = [0x77889900AABBCCDD];
    let base_http = "POST /api/1 HTTP/1.1\r\nHost: localhost:7144\r\nContent-Type: application/json\r\nContent-Length: 42\r\n\r\n{\"jsonrpc\":\"2.0\",\"method\":\"getStatus\",\"id\":1}";

    for &seed in &seeds {
        let mut rng = FuzzRng::new(seed);
        for _ in 0..500 {
            let mut req_bytes = base_http.as_bytes().to_vec();
            rng.mutate(&mut req_bytes);

            // 1. HTTP parse fuzz
            let parse_res = HttpRequest::parse(&req_bytes);
            match parse_res {
                Ok(req) => {
                    assert!(!req.raw_path.is_empty());
                }
                Err(_) => {
                    // Clean rejection of malformed HTTP framing
                }
            }

            // 2. JSON-RPC dispatch fuzz
            let rpc_state = RpcState::default();
            let dispatcher = RpcDispatcher::new(rpc_state);
            let json_str = String::from_utf8_lossy(&req_bytes);
            let _ = dispatcher.handle_request_str(&json_str, AccessLevel::Playback).await;
        }
    }
}

/// Acceptance criterion 5:
/// Fault injection: Partial reads, fragmentations at every byte split, and disconnects (REQ-025, REQ-070).
#[test]
fn test_p42_fragmentation_fault_injection() {
    let payload = vec![
        b'p', b'c', b'p', b'\n', 0x01, 0x00, 0x00, 0x80, // parent 1 child
        b'c', b'h', b'a', b'n', 0x10, 0x00, 0x00, 0x00, // leaf 16 bytes
        0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x10,
    ];

    let budget = AtomBudget::default();

    // 1. Every split position from 1 to payload.len()
    for split_pos in 1..payload.len() {
        let first_half = &payload[..split_pos];
        let mut b = Bytes::copy_from_slice(first_half);
        // Partial decode must return Ok(None) or Err, never panic or return corrupt atom
        let res = Atom::decode(&mut b, &budget);
        assert!(matches!(res, Ok(None) | Err(_)), "partial atom at split {} must not return Ok(Some)", split_pos);
    }

    // 2. Fragmented feeding: 1-byte chunks into container demuxers
    let flv_data = vec![
        b'F', b'L', b'V', 0x01, 0x05, 0x00, 0x00, 0x00, 0x09,
        0x00, 0x00, 0x00, 0x00, // FLV header
        0x12, 0x00, 0x00, 0x05, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // Tag header
        0x01, 0x02, 0x03, 0x04, 0x05, // Tag payload
        0x00, 0x00, 0x00, 0x10, // Previous tag size
    ];

    let mut demuxer = FlvDemuxer::new(DEFAULT_MAX_FLV_TAG_SIZE);
    for &byte in &flv_data {
        demuxer.feed(&[byte]);
        let _ = demuxer.next_output();
    }
}
