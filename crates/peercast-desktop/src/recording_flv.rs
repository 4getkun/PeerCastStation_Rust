//! Incremental FLV timestamp normalization for recorded streams.

const MAX_HEADER_LEN: usize = 1024 * 1024;
const MAX_TAG_LEN: usize = 11 + 0x00ff_ffff + 4;
const MAX_PRE_ORIGIN_TAGS: usize = 1024 * 1024;

/// Incrementally parses an FLV stream and rebases tag DTS values per recording.
#[derive(Default)]
pub struct FlvTimestampNormalizer {
    input: Vec<u8>,
    header_len: Option<usize>,
    header_written: bool,
    origin: Option<u32>,
    before_origin: Vec<Vec<u8>>,
    before_origin_bytes: usize,
}

impl FlvTimestampNormalizer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Consume bytes and return complete normalized FLV bytes.
    pub fn push(&mut self, mut bytes: &[u8]) -> Result<Vec<u8>, String> {
        let mut output = Vec::new();
        loop {
            self.consume_ready(&mut output)?;
            if bytes.is_empty() {
                break;
            }
            let target = self.next_target()?;
            let needed = target.saturating_sub(self.input.len()).max(1);
            let count = needed.min(bytes.len());
            self.input.extend_from_slice(&bytes[..count]);
            bytes = &bytes[count..];
            if self.input.len() > MAX_HEADER_LEN + MAX_TAG_LEN {
                return Err("FLV parser buffer limit exceeded".into());
            }
        }
        self.consume_ready(&mut output)?;
        Ok(output)
    }

    /// Drop any incomplete final header/tag and return complete buffered pre-origin tags.
    pub fn finish(&mut self) -> Vec<u8> {
        self.input.clear();
        let mut output = Vec::new();
        if self.origin.is_none() {
            for mut tag in self.before_origin.drain(..) {
                write_timestamp(&mut tag, 0);
                output.extend_from_slice(&tag);
            }
        }
        self.before_origin_bytes = 0;
        output
    }

    fn next_target(&mut self) -> Result<usize, String> {
        if self.header_written {
            if self.input.len() < 11 {
                return Ok(self.input.len() + 11);
            }
            let tag_len = (((self.input[1] as usize) << 16)
                | ((self.input[2] as usize) << 8)
                | self.input[3] as usize)
                + 15;
            return Ok(self.input.len() + tag_len.saturating_sub(self.input.len()));
        }
        if self.header_len.is_none() {
            if self.input.len() < 9 {
                return Ok(9);
            }
            if &self.input[..3] != b"FLV" || self.input[3] != 1 {
                return Err("invalid FLV signature or version".into());
            }
            let data_offset = u32::from_be_bytes(self.input[5..9].try_into().unwrap()) as usize;
            if !(9..=MAX_HEADER_LEN - 4).contains(&data_offset) {
                return Err("FLV header length out of bounds".into());
            }
            self.header_len = Some(data_offset + 4);
        }
        if let Some(header_len) = self.header_len {
            if self.input.len() < header_len {
                return Ok(header_len);
            }
            if self.input[header_len - 4..header_len] != [0, 0, 0, 0] {
                return Err("invalid FLV PreviousTagSize0".into());
            }
            if self.input.len() < header_len + 11 {
                return Ok(header_len + 11);
            }
            let tag_len = (((self.input[header_len + 1] as usize) << 16)
                | ((self.input[header_len + 2] as usize) << 8)
                | self.input[header_len + 3] as usize)
                + 15;
            if tag_len > MAX_TAG_LEN {
                return Err("FLV tag length out of bounds".into());
            }
            return Ok(header_len + tag_len);
        }
        Ok(9)
    }

    fn consume_ready(&mut self, output: &mut Vec<u8>) -> Result<(), String> {
        if self.header_len.is_none() && self.input.len() >= 9 {
            self.next_target()?;
        }
        let Some(header_len) = self.header_len else {
            return Ok(());
        };
        if self.input.len() < header_len {
            return Ok(());
        }
        if !self.header_written && self.input[header_len - 4..header_len] != [0, 0, 0, 0] {
            return Err("invalid FLV PreviousTagSize0".into());
        }
        if !self.header_written {
            if self.input.len() < header_len {
                return Ok(());
            }
            output.extend_from_slice(&self.input[..header_len]);
            self.input.drain(..header_len);
            self.header_written = true;
        }
        loop {
            if self.input.len() < 11 {
                break;
            }
            let size = ((self.input[1] as usize) << 16)
                | ((self.input[2] as usize) << 8)
                | self.input[3] as usize;
            if self.input[0] & 0xc0 != 0 || self.input[8..11] != [0, 0, 0] {
                return Err("invalid FLV tag header".into());
            }
            let tag_len = 11 + size + 4;
            if self.input.len() < tag_len {
                break;
            }
            let expected = (11 + size) as u32;
            let actual = u32::from_be_bytes(self.input[11 + size..tag_len].try_into().unwrap());
            if actual != expected {
                return Err("invalid FLV PreviousTagSize".into());
            }
            let mut tag = self.input[..tag_len].to_vec();
            self.input.drain(..tag_len);
            let kind = tag[0] & 0x1f;
            let body = &tag[11..11 + size];
            if kind == 18 && is_metadata(body) {
                continue;
            }
            let is_media = matches!(kind, 8 | 9);
            let sequence_header = is_sequence_header(kind, body);
            let timestamp = read_timestamp(&tag);
            if is_media && !sequence_header && self.origin.is_none() {
                self.origin = Some(timestamp);
                output.extend_from_slice(&self.flush_before_origin());
            }
            if sequence_header {
                write_timestamp(&mut tag, 0);
            } else if let Some(origin) = self.origin {
                let delta = timestamp.wrapping_sub(origin);
                write_timestamp(&mut tag, if delta > i32::MAX as u32 { 0 } else { delta });
            } else {
                self.before_origin_bytes += tag.len();
                if self.before_origin_bytes > MAX_PRE_ORIGIN_TAGS {
                    return Err("FLV pre-origin tag budget exceeded".into());
                }
                self.before_origin.push(tag);
                continue;
            }
            output.extend_from_slice(&tag);
        }
        Ok(())
    }

    fn flush_before_origin(&mut self) -> Vec<u8> {
        let origin = self.origin.unwrap_or(0);
        let mut output = Vec::with_capacity(self.before_origin_bytes);
        for mut tag in self.before_origin.drain(..) {
            let ts = read_timestamp(&tag);
            let delta = ts.wrapping_sub(origin);
            write_timestamp(&mut tag, if delta > i32::MAX as u32 { 0 } else { delta });
            output.extend_from_slice(&tag);
        }
        self.before_origin_bytes = 0;
        output
    }
}

fn read_timestamp(tag: &[u8]) -> u32 {
    ((tag[7] as u32) << 24) | ((tag[4] as u32) << 16) | ((tag[5] as u32) << 8) | tag[6] as u32
}

fn write_timestamp(tag: &mut [u8], timestamp: u32) {
    tag[4] = (timestamp >> 16) as u8;
    tag[5] = (timestamp >> 8) as u8;
    tag[6] = timestamp as u8;
    tag[7] = (timestamp >> 24) as u8;
}

fn is_sequence_header(kind: u8, body: &[u8]) -> bool {
    match kind {
        8 => body.len() >= 2 && body[0] >> 4 == 10 && body[1] == 0,
        9 => body.len() >= 2 && body[0] & 0x0f == 7 && body[1] == 0,
        _ => false,
    }
}

fn is_metadata(body: &[u8]) -> bool {
    fn amf_string(bytes: &[u8], at: usize) -> Option<(&[u8], usize)> {
        if bytes.get(at)? != &2 {
            return None;
        }
        let len = u16::from_be_bytes([*bytes.get(at + 1)?, *bytes.get(at + 2)?]) as usize;
        let end = at.checked_add(3)?.checked_add(len)?;
        Some((bytes.get(at + 3..end)?, end))
    }
    let Some((first, end)) = amf_string(body, 0) else {
        return false;
    };
    if first == b"onMetaData" {
        return true;
    }
    first == b"@setDataFrame"
        && amf_string(body, end).is_some_and(|(second, _)| second == b"onMetaData")
}

#[cfg(test)]
mod tests {
    use super::FlvTimestampNormalizer;

    #[test]
    fn rebases_late_start_and_drops_old_metadata_duration() {
        let input = fixture(4 * 60 * 60 * 1000, true);
        let mut normalizer = FlvTimestampNormalizer::new();
        let mut output = Vec::new();
        for chunk in input.chunks(1) {
            output.extend(normalizer.push(chunk).unwrap());
        }
        output.extend(normalizer.finish());
        let timestamps = tag_timestamps(&output);
        assert_eq!(timestamps, vec![0, 0, 0, 7]);
    }

    #[test]
    fn handles_every_single_split_and_preserves_video_composition_bytes() {
        let input = fixture(15_666_827, false);
        for split in 0..=input.len() {
            let mut normalizer = FlvTimestampNormalizer::new();
            let mut output = normalizer.push(&input[..split]).unwrap();
            output.extend(normalizer.push(&input[split..]).unwrap());
            output.extend(normalizer.finish());
            assert_eq!(tag_timestamps(&output), vec![0, 0, 0, 7]);
            assert!(output.windows(4).any(|w| w == [0x17, 1, 0xff, 0xff]));
        }
    }

    #[test]
    fn wraps_u32_clock_and_discards_truncated_final_tag() {
        let mut input = fixture(u32::MAX - 4, false);
        input.extend(tag(9, 3, &[0x27, 1, 0, 0, 0]));
        let mut normalizer = FlvTimestampNormalizer::new();
        let mut output = normalizer.push(&input).unwrap();
        output.extend(normalizer.finish());
        assert_eq!(tag_timestamps(&output), vec![0, 0, 0, 7, 8]);

        let mut truncated = fixture(100, false);
        truncated.extend_from_slice(&tag(8, 110, &[1])[..12]);
        let mut normalizer = FlvTimestampNormalizer::new();
        let mut output = normalizer.push(&truncated).unwrap();
        output.extend(normalizer.finish());
        assert_eq!(tag_timestamps(&output), vec![0, 0, 0, 7]);
    }

    #[test]
    fn strips_both_metadata_forms_but_keeps_other_script_tags_and_rejects_bad_size() {
        let mut input = b"FLV\x01\x05\0\0\0\x09\0\0\0\0".to_vec();
        let mut body = vec![2, 0, 13];
        body.extend_from_slice(b"@setDataFrame");
        body.extend_from_slice(&[2, 0, 10]);
        body.extend_from_slice(b"onMetaData");
        input.extend(tag(18, 55, &body));
        input.extend(tag(
            18,
            56,
            &[
                2, 0, 11, b'o', b'n', b'T', b'e', b'x', b't', b'D', b'a', b't', b'a',
            ],
        ));
        input.extend(tag(8, 100, &[0xaf, 1]));
        let mut normalizer = FlvTimestampNormalizer::new();
        let mut output = normalizer.push(&input).unwrap();
        output.extend(normalizer.finish());
        assert_eq!(tag_types(&output), vec![18, 8]);
        assert_eq!(tag_timestamps(&output), vec![0, 0]);

        let mut invalid_header = b"FLV\x01\x05\0\0\0\x09\0\0\0\x01".to_vec();
        let mut normalizer = FlvTimestampNormalizer::new();
        assert!(normalizer.push(&invalid_header).is_err());
        invalid_header[12] = 0;
        let mut bad_tag = tag(8, 10, &[0xaf, 1]);
        let footer = bad_tag.len() - 1;
        bad_tag[footer] ^= 1;
        let mut normalizer = FlvTimestampNormalizer::new();
        assert_eq!(normalizer.push(&invalid_header).unwrap().len(), 13);
        assert!(normalizer.push(&bad_tag).is_err());
    }

    #[test]
    fn clamps_small_reordering_before_the_origin_to_zero() {
        let mut input = b"FLV\x01\x05\0\0\0\x09\0\0\0\0".to_vec();
        input.extend(tag(8, 100, &[0xaf, 1]));
        input.extend(tag(9, 99, &[0x17, 1, 0, 0, 0]));
        let mut normalizer = FlvTimestampNormalizer::new();
        let mut output = normalizer.push(&input).unwrap();
        output.extend(normalizer.finish());
        assert_eq!(tag_timestamps(&output), vec![0, 0]);
    }

    fn fixture(base: u32, metadata: bool) -> Vec<u8> {
        let mut out = b"FLV\x01\x05\0\0\0\x09\0\0\0\0".to_vec();
        if metadata {
            let body = [
                2, 0, 10, b'o', b'n', b'M', b'e', b't', b'a', b'D', b'a', b't', b'a',
            ];
            out.extend(tag(18, 0, &body));
        }
        out.extend(tag(9, 0, &[0x17, 0, 0, 0, 0]));
        out.extend(tag(8, base, &[0xaf, 0]));
        out.extend(tag(8, base, &[0xaf, 1]));
        out.extend(tag(9, base.wrapping_add(7), &[0x17, 1, 0xff, 0xff]));
        out
    }

    fn tag(kind: u8, timestamp: u32, body: &[u8]) -> Vec<u8> {
        let n = body.len() as u32;
        let mut out = vec![
            kind,
            (n >> 16) as u8,
            (n >> 8) as u8,
            n as u8,
            (timestamp >> 16) as u8,
            (timestamp >> 8) as u8,
            timestamp as u8,
            (timestamp >> 24) as u8,
            0,
            0,
            0,
        ];
        out.extend_from_slice(body);
        out.extend_from_slice(&(11 + n).to_be_bytes());
        out
    }

    fn tag_timestamps(data: &[u8]) -> Vec<u32> {
        let mut pos = 13;
        let mut values = Vec::new();
        while pos + 15 <= data.len() {
            let len = ((data[pos + 1] as usize) << 16)
                | ((data[pos + 2] as usize) << 8)
                | data[pos + 3] as usize;
            if pos + 15 + len > data.len() {
                break;
            }
            values.push(
                ((data[pos + 7] as u32) << 24)
                    | ((data[pos + 4] as u32) << 16)
                    | ((data[pos + 5] as u32) << 8)
                    | data[pos + 6] as u32,
            );
            pos += 15 + len;
        }
        values
    }

    fn tag_types(data: &[u8]) -> Vec<u8> {
        let mut pos = 13;
        let mut values = Vec::new();
        while pos + 15 <= data.len() {
            let len = ((data[pos + 1] as usize) << 16)
                | ((data[pos + 2] as usize) << 8)
                | data[pos + 3] as usize;
            if pos + 15 + len > data.len() {
                break;
            }
            values.push(data[pos] & 0x1f);
            pos += 15 + len;
        }
        values
    }
}
