//! Direction-specific USB channel framing recovered from the runtime (0x1402d3490, 0x1402d3740).
//!
//! Outgoing records carry `[channel, payload length]`; incoming records carry
//! `[recipient bitmask, total length including the header]`. See docs/usb-transport-static.md.
//! Validation limits are our design, not vendor limits.

use crate::error::FramingError;

pub const HEADER_LEN: usize = 8;
pub const CHANNEL_COUNT: u32 = 12;
pub const MAX_FRAME: usize = 1024 * 1024;
pub const MAX_OUT_PAYLOAD: usize = 4087;

type Result<T> = std::result::Result<T, FramingError>;

fn check_channel(channel: u32) -> Result<()> {
    if channel < CHANNEL_COUNT { Ok(()) } else { Err(FramingError::new("Channel must be between 0 and 11")) }
}

fn header(first: u32, second: u32) -> [u8; HEADER_LEN] {
    let mut bytes = [0; HEADER_LEN];
    bytes[..4].copy_from_slice(&first.to_le_bytes());
    bytes[4..].copy_from_slice(&second.to_le_bytes());
    bytes
}

fn read_header(bytes: &[u8]) -> (u32, usize) {
    let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes"));
    (word(0), word(4) as usize)
}

/// Bulk OUT records for one payload, with the original's chunk sizes: at most 4087
/// payload bytes, one fewer when the chunk would be a multiple of 512.
pub fn outgoing_frames(channel: u32, payload: &[u8]) -> Result<Vec<Vec<u8>>> {
    check_channel(channel)?;
    if payload.len() > MAX_FRAME {
        return Err(FramingError::new("Payload exceeds local frame limit"));
    }
    let mut frames = Vec::new();
    let mut offset = 0;
    while offset < payload.len() {
        let mut count = (payload.len() - offset).min(MAX_OUT_PAYLOAD);
        if count.is_multiple_of(512) {
            count -= 1;
        }
        let mut frame = Vec::with_capacity(HEADER_LEN + count);
        frame.extend_from_slice(&header(channel, count as u32));
        frame.extend_from_slice(&payload[offset..offset + count]);
        frames.push(frame);
        offset += count;
    }
    Ok(frames)
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IncomingFrame {
    pub recipients: u32,
    pub payload: Vec<u8>,
}

impl IncomingFrame {
    pub fn for_channel(&self, channel: u32) -> Result<bool> {
        check_channel(channel)?;
        Ok(self.recipients & (1 << channel) != 0)
    }
}

/// Incremental bulk IN parser.
///
/// Fragmented headers and payloads and several frames per USB read are supported.
/// Invalid framing poisons the parser until the owner creates a new one; recipient
/// bits are preserved although the original dispatches only channels 0 to 11.
#[derive(Debug, Clone)]
pub struct IncomingFrames {
    max_frame: usize,
    buffer: Vec<u8>,
    failed: bool,
}

impl Default for IncomingFrames {
    fn default() -> Self {
        Self { max_frame: MAX_FRAME, buffer: Vec::new(), failed: false }
    }
}

impl IncomingFrames {
    pub fn new(max_frame: usize) -> Result<Self> {
        if !(HEADER_LEN..=MAX_FRAME).contains(&max_frame) {
            return Err(FramingError::new("Invalid frame limit"));
        }
        Ok(Self { max_frame, ..Self::default() })
    }

    pub fn max_frame(&self) -> usize {
        self.max_frame
    }

    pub fn buffered(&self) -> &[u8] {
        &self.buffer
    }

    pub fn failed(&self) -> bool {
        self.failed
    }

    fn poison(&mut self, message: &str) -> FramingError {
        self.failed = true;
        self.buffer.clear();
        FramingError::new(message)
    }

    fn checked_target(&mut self) -> Result<(u32, usize)> {
        let (recipients, target) = read_header(&self.buffer);
        if !(HEADER_LEN..=self.max_frame).contains(&target) {
            return Err(self.poison("Invalid incoming total frame length"));
        }
        Ok((recipients, target))
    }

    pub fn feed(&mut self, data: &[u8]) -> Result<Vec<IncomingFrame>> {
        if self.failed {
            return Err(FramingError::new("Parser unusable after invalid framing"));
        }
        if data.len() > self.max_frame {
            return Err(self.poison("Input exceeds local frame limit"));
        }
        let mut frames = Vec::new();
        let mut offset = 0;
        while offset < data.len() || self.buffer.len() >= HEADER_LEN {
            let mut target = HEADER_LEN;
            if self.buffer.len() >= HEADER_LEN {
                target = self.checked_target()?.1;
            }
            let needed = target - self.buffer.len();
            if needed > 0 {
                let take = needed.min(data.len() - offset);
                self.buffer.extend_from_slice(&data[offset..offset + take]);
                offset += take;
                if self.buffer.len() < target {
                    break;
                }
                if target == HEADER_LEN && self.checked_target()?.1 > HEADER_LEN {
                    continue;
                }
            }
            let (recipients, target) = read_header(&self.buffer);
            frames.push(IncomingFrame { recipients, payload: self.buffer[HEADER_LEN..target].to_vec() });
            self.buffer.clear();
        }
        Ok(frames)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inbound(mask: u32, payload: &[u8]) -> Vec<u8> {
        let mut data = header(mask, (payload.len() + HEADER_LEN) as u32).to_vec();
        data.extend_from_slice(payload);
        data
    }

    #[test]
    fn original_emulated_tx_vectors() {
        let frames = outgoing_frames(7, b"x").unwrap();
        assert_eq!(frames, vec![[&[7, 0, 0, 0, 1, 0, 0, 0][..], b"x"].concat()]);
        let frames = outgoing_frames(3, &[b'x'; 512]).unwrap();
        assert_eq!(frames.iter().map(Vec::len).collect::<Vec<_>>(), [519, 9]);
        assert_eq!(read_header(&frames[0]), (3, 511));
        let frames = outgoing_frames(11, &[b'x'; 8192]).unwrap();
        assert_eq!(frames.iter().map(Vec::len).collect::<Vec<_>>(), [4095, 4095, 26]);
        assert!(outgoing_frames(0, b"").unwrap().is_empty());
        assert!(outgoing_frames(12, b"x").is_err());
    }

    #[test]
    fn every_two_way_fragmentation_and_coalesced_frames() {
        let wire = [inbound(0x805, b"hello"), inbound(2, b"world"), inbound(0, b"")].concat();
        for split in 0..=wire.len() {
            let mut parser = IncomingFrames::default();
            let mut frames = parser.feed(&wire[..split]).unwrap();
            frames.extend(parser.feed(&wire[split..]).unwrap());
            let got: Vec<_> = frames.iter().map(|f| (f.recipients, f.payload.as_slice())).collect();
            assert_eq!(got, [(0x805, &b"hello"[..]), (2, b"world"), (0, b"")]);
            assert!(frames[0].for_channel(11).unwrap());
            assert!(!frames[0].for_channel(1).unwrap());
        }
    }

    #[test]
    fn bytewise_and_length_validation_fail_closed() {
        let mut parser = IncomingFrames::default();
        let mut frames = Vec::new();
        for byte in inbound(1, b"abc") {
            frames.extend(parser.feed(&[byte]).unwrap());
        }
        assert_eq!(frames[0].payload, b"abc");
        for length in [0u32, 7, 1024 * 1024 + 1, u32::MAX] {
            let mut parser = IncomingFrames::default();
            assert!(parser.feed(&header(1, length)).is_err());
            assert!(parser.buffered().is_empty());
            assert!(parser.feed(&inbound(1, b"abc")).is_err());
        }
    }

    #[test]
    fn rx_length_includes_header_unlike_tx() {
        let mut parser = IncomingFrames::default();
        assert!(parser.feed(&outgoing_frames(1, b"abc").unwrap()[0]).is_err());
    }
}
