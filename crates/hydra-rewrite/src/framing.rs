use anyhow::{bail, ensure};
use hydra_domain::ForwardFraming;
use std::ops::Range;
pub const MAX_FRAME: usize = 65536;
pub struct Frame {
    pub wire: Vec<u8>,
    pub regions: Vec<Vec<Range<usize>>>,
    pub method: Option<String>,
    pub informational: bool,
}
impl Frame {
    pub fn payload(bytes: Vec<u8>, start: usize) -> Self {
        let end = bytes.len();
        Self {
            wire: bytes,
            regions: vec![vec![start..end]],
            method: None,
            informational: false,
        }
    }
    pub fn rewrite(
        &mut self,
        mut replacements: impl FnMut(&[u8]) -> anyhow::Result<Vec<(Range<usize>, Vec<u8>)>>,
    ) -> anyhow::Result<usize> {
        let mut count = 0;
        for spans in &self.regions {
            let mut logical: Vec<u8> = spans
                .iter()
                .flat_map(|r| self.wire[r.clone()].iter().copied())
                .collect();
            let mut previous = 0;
            for (range, value) in replacements(&logical)? {
                ensure!(
                    range.start >= previous
                        && range.end <= logical.len()
                        && range.len() == value.len(),
                    "invalid replacement span"
                );
                previous = range.end;
                logical[range].copy_from_slice(&value);
                count += 1;
            }
            let mut at = 0;
            for span in spans {
                self.wire[span.clone()].copy_from_slice(&logical[at..at + span.len()]);
                at += span.len();
            }
        }
        Ok(count)
    }
}
pub struct Framer {
    pub(crate) policy: ForwardFraming,
    pub(crate) buffer: Vec<u8>,
    pub(crate) response: bool,
}
impl Framer {
    pub fn new(policy: ForwardFraming, response: bool) -> anyhow::Result<Self> {
        match &policy {
            ForwardFraming::Delimited { delimiter } => ensure!(
                !delimiter.is_empty() && delimiter.len() <= 16,
                "delimiter must contain 1..16 bytes"
            ),
            ForwardFraming::LengthPrefixed { offset, width, .. } => ensure!(
                *offset <= 16 && [1, 2, 4].contains(width),
                "length prefix width must be 1, 2 or 4 and offset at most16"
            ),
            _ => {}
        }
        Ok(Self {
            policy,
            buffer: Vec::new(),
            response,
        })
    }
    pub fn pending(&self) -> bool {
        !self.buffer.is_empty()
    }
    pub fn feed(
        &mut self,
        bytes: &[u8],
        eof: bool,
        method: Option<&str>,
    ) -> anyhow::Result<Vec<Frame>> {
        ensure!(
            self.buffer.len() + bytes.len() <= MAX_FRAME + 16384,
            "forwarding frame buffer exceeded"
        );
        self.buffer.extend_from_slice(bytes);
        let mut frames = Vec::new();
        loop {
            let frame = match &self.policy {
                ForwardFraming::Http1 => {
                    crate::http::frame(&self.buffer, self.response, eof, method)?
                }
                ForwardFraming::Datagram => {
                    ensure!(
                        !self.pending() || self.buffer.len() <= 65535,
                        "UDP datagram too large"
                    );
                    Some(Frame::payload(std::mem::take(&mut self.buffer), 0))
                }
                ForwardFraming::Delimited { delimiter } => self
                    .buffer
                    .windows(delimiter.len())
                    .position(|w| w == delimiter.as_bytes())
                    .map(|p| {
                        let mut frame =
                            Frame::payload(self.buffer[..p + delimiter.len()].to_vec(), 0);
                        frame.regions = vec![vec![0..p]];
                        frame
                    }),
                ForwardFraming::LengthPrefixed {
                    offset,
                    width,
                    little_endian,
                    length_includes_header,
                } => {
                    let header = *offset as usize + *width as usize;
                    if self.buffer.len() < header {
                        None
                    } else {
                        let mut length = 0usize;
                        let digits = &self.buffer[*offset as usize..header];
                        if *little_endian {
                            for b in digits.iter().rev() {
                                length = (length << 8) | usize::from(*b);
                            }
                        } else {
                            for b in digits {
                                length = (length << 8) | usize::from(*b);
                            }
                        }
                        let total = if *length_includes_header {
                            length
                        } else {
                            length
                                .checked_add(header)
                                .ok_or_else(|| anyhow::anyhow!("frame size overflow"))?
                        };
                        ensure!(
                            (header..=MAX_FRAME).contains(&total),
                            "invalid forwarding frame length"
                        );
                        if self.buffer.len() < total {
                            None
                        } else {
                            Some(Frame::payload(self.buffer[..total].to_vec(), header))
                        }
                    }
                }
            };
            let Some(frame) = frame else { break };
            let length = frame.wire.len();
            ensure!(length <= MAX_FRAME, "forwarding frame exceeds64KiB");
            if !matches!(self.policy, ForwardFraming::Datagram) {
                self.buffer.drain(..length);
            }
            frames.push(frame);
            // HTTP response framing depends on the next queued request method.
            if frames.len() >= 128
                || self.buffer.is_empty()
                || self.response && matches!(self.policy, ForwardFraming::Http1)
            {
                break;
            }
        }
        if eof && self.pending() && frames.is_empty() {
            bail!("incomplete forwarding frame at EOF");
        }
        ensure!(
            self.buffer.len() <= MAX_FRAME,
            "forwarding frame exceeds64KiB"
        );
        Ok(frames)
    }
}
