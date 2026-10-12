//! Incremental protocol framing. No port access or arrival-time assumptions.
use core_types::{Result, SignalFlag};

/// Parse exactly one finite value per configured channel.
pub fn parse_csv_line(line: &str, channels: usize) -> Result<Vec<f64>> {
    if !(1..=2).contains(&channels) {
        return Err("csv: channels must be 1 or 2".into());
    }
    if line.len() > 256 {
        return Err("csv: строка длиннее 256 байт".into());
    }
    let fields: Vec<_> = line.trim().split(',').collect();
    if fields.len() != channels {
        return Err(format!(
            "csv: ожидалось {channels} значений, получено {}",
            fields.len()
        ));
    }
    fields
        .iter()
        .map(|s| {
            s.trim()
                .parse::<f64>()
                .ok()
                .filter(|v| v.is_finite())
                .ok_or_else(|| "csv: некорректное или неконечное число".into())
        })
        .collect()
}

#[derive(Debug)]
pub struct DecodedFrame {
    pub sequence: Option<u64>,
    /// Minimum missing frames inferred modulo 256, not an absolute loss count.
    pub missing_frames: u64,
    pub duplicate_counter: bool,
    pub device_timestamp_ns: Option<u64>,
    pub channels: Vec<f64>,
    pub flags: Vec<SignalFlag>,
}
pub type DecodeResult = Result<DecodedFrame>;
pub trait SerialDecoder: Send {
    fn push(&mut self, bytes: &[u8]) -> Vec<DecodeResult>;
    fn reset(&mut self);
}
fn frame(channels: Vec<f64>) -> DecodeResult {
    Ok(DecodedFrame {
        sequence: None,
        missing_frames: 0,
        duplicate_counter: false,
        device_timestamp_ns: None,
        channels,
        flags: Vec::new(),
    })
}
pub struct CsvDecoder {
    channels: usize,
    line: Vec<u8>,
    overflow: bool,
}
impl CsvDecoder {
    pub fn new(channels: usize) -> Self {
        Self {
            channels,
            line: Vec::new(),
            overflow: false,
        }
    }
}
impl SerialDecoder for CsvDecoder {
    fn push(&mut self, bytes: &[u8]) -> Vec<DecodeResult> {
        let mut out = Vec::new();
        for &byte in bytes {
            if byte == b'\n' {
                out.push(if self.overflow {
                    Err("csv: line exceeds 256 bytes".into())
                } else {
                    std::str::from_utf8(&self.line)
                        .map_err(|_| "csv: invalid UTF-8".into())
                        .and_then(|s| parse_csv_line(s, self.channels))
                        .and_then(frame)
                });
                self.reset();
            } else if self.line.len() < 256 && !self.overflow {
                self.line.push(byte);
            } else {
                self.line.clear();
                self.overflow = true;
            }
        }
        out
    }
    fn reset(&mut self) {
        self.line.clear();
        self.overflow = false;
    }
}
pub struct TaggedU8Decoder {
    pending: Vec<u8>,
    tags: Vec<[u8; 2]>,
    channels: Vec<usize>,
    counter: Option<usize>,
    previous_counter: Option<u8>,
}
impl Default for TaggedU8Decoder {
    fn default() -> Self {
        Self {
            pending: Vec::new(),
            tags: vec![*b"A0", *b"A1"],
            channels: vec![0, 1],
            counter: None,
            previous_counter: None,
        }
    }
}
impl TaggedU8Decoder {
    pub fn new(config: &core_types::SourceConfig) -> Result<Self> {
        config.validate_serial_tags()?;
        Ok(Self {
            pending: Vec::new(),
            tags: config
                .serial_frame_tags
                .iter()
                .map(|t| [t.as_bytes()[0], t.as_bytes()[1]])
                .collect(),
            channels: config
                .serial_channel_tags
                .iter()
                .filter_map(|t| config.serial_frame_tags.iter().position(|v| v == t))
                .collect(),
            counter: config
                .serial_sequence_tag
                .as_ref()
                .and_then(|t| config.serial_frame_tags.iter().position(|v| v == t)),
            previous_counter: None,
        })
    }
}
impl SerialDecoder for TaggedU8Decoder {
    fn push(&mut self, bytes: &[u8]) -> Vec<DecodeResult> {
        let mut out = Vec::new();
        let mut discarded = 0;
        for &byte in bytes {
            self.pending.push(byte);
            while !self.pending.is_empty() {
                let p = &self.pending;
                let invalid = p
                    .iter()
                    .enumerate()
                    .any(|(i, b)| i % 3 < 2 && *b != self.tags[i / 3][i % 3]);
                if invalid {
                    self.pending.remove(0);
                    discarded += 1;
                } else if p.len() == self.tags.len() * 3 {
                    if discarded > 0 {
                        out.push(Err(format!(
                            "tagged-u8: discarded {discarded} bytes while resynchronizing"
                        )));
                        discarded = 0;
                    }
                    let sequence = self.counter.map(|i| p[3 * i + 2]);
                    let delta = sequence
                        .zip(self.previous_counter)
                        .map(|(next, previous)| next.wrapping_sub(previous));
                    // Modulo evidence only: resets, duplicates and losses of 256 frames are ambiguous.
                    let missing_frames = delta.map_or(0, |d| d.saturating_sub(1) as u64);
                    let duplicate_counter = delta == Some(0);
                    let mut flags = Vec::new();
                    if missing_frames > 0 {
                        flags.push(SignalFlag::LostSample);
                    }
                    if duplicate_counter {
                        flags.push(SignalFlag::InvalidPacket);
                    }
                    out.push(Ok(DecodedFrame {
                        sequence: sequence.map(u64::from),
                        device_timestamp_ns: None,
                        channels: self.channels.iter().map(|i| p[3 * i + 2] as f64).collect(),
                        flags,
                        missing_frames,
                        duplicate_counter,
                    }));
                    self.previous_counter = sequence;
                    self.pending.clear();
                } else {
                    break;
                }
            }
        }
        if discarded > 0 {
            out.push(Err(format!(
                "tagged-u8: discarded {discarded} bytes while resynchronizing"
            )));
        }
        out
    }
    fn reset(&mut self) {
        self.pending.clear();
        self.previous_counter = None;
    }
}
pub(crate) fn decoder(
    config: &core_types::SourceConfig,
    channels: usize,
) -> Result<Box<dyn SerialDecoder>> {
    match config.serial_protocol.as_str() {
        "csv" if (1..=2).contains(&channels) => Ok(Box::new(CsvDecoder::new(channels))),
        "tagged-u8" => Ok(Box::new(TaggedU8Decoder::new(config)?)),
        protocol => Err(format!("Unsupported Serial protocol/channel count {protocol}/{channels}; choose csv or tagged-u8")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config(channels: usize) -> core_types::SourceConfig {
        core_types::SourceConfig {
            channels,
            serial_protocol: "tagged-u8".into(),
            serial_frame_tags: ["A0", "A1", "A2", "A3", "A4", "A5", "A6", "A7", "A9"]
                .map(String::from)
                .to_vec(),
            serial_channel_tags: ["A0", "A1"][..channels]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            serial_sequence_tag: Some("A9".into()),
            ..core_types::SourceConfig::default()
        }
    }
    fn packet(sequence: u8) -> Vec<u8> {
        b"012345679"
            .iter()
            .flat_map(|t| {
                [
                    b'A',
                    *t,
                    if *t == b'9' {
                        sequence
                    } else if *t == b'0' {
                        b'A'
                    } else {
                        *t
                    },
                ]
            })
            .collect()
    }
    #[test]
    fn configurable_channels_counter_wrap_and_gap_at_every_split() {
        let bytes: Vec<_> = [254, 255, 0, 3, 3].into_iter().flat_map(packet).collect();
        for channels in 1..=2 {
            for chunk in 1..=bytes.len() {
                let mut decoder = TaggedU8Decoder::new(&config(channels)).expect("config");
                let frames: Vec<_> = bytes
                    .chunks(chunk)
                    .flat_map(|b| decoder.push(b))
                    .map(|r| r.expect("frame"))
                    .collect();
                assert_eq!(frames.len(), 5);
                assert_eq!(
                    frames.iter().map(|f| f.sequence).collect::<Vec<_>>(),
                    [Some(254), Some(255), Some(0), Some(3), Some(3)]
                );
                assert!(frames[..3].iter().all(|f| f.flags.is_empty()));
                assert_eq!(frames[3].missing_frames, 2);
                assert!(frames[3].flags.contains(&SignalFlag::LostSample));
                assert!(frames[4].duplicate_counter);
                assert_eq!(frames[4].missing_frames, 0);
                for f in frames {
                    assert_eq!(f.channels, [65., 49.][..channels]);
                }
            }
        }
    }
    #[test]
    fn full_frame_resynchronization_and_bounded_garbage() {
        for offset in 0..27 {
            let mut decoder = TaggedU8Decoder::new(&config(1)).expect("config");
            let mut bytes = packet(1)[offset..].to_vec();
            let mut corrupt = packet(2);
            corrupt[15] = b'X';
            bytes.extend(corrupt);
            bytes.extend([0xff; 1000]);
            bytes.extend(packet(3));
            bytes.extend(&packet(4)[..26]);
            let frames: Vec<_> = bytes
                .chunks(1)
                .flat_map(|b| decoder.push(b))
                .filter_map(Result::ok)
                .collect();
            assert_eq!(frames.last().expect("recovered").sequence, Some(3));
            assert_eq!(frames.len(), if offset == 0 { 2 } else { 1 });
            assert!(decoder.pending.len() < 27);
            assert_eq!(
                decoder.push(&[4])[0].as_ref().expect("tail").sequence,
                Some(4)
            );
        }
    }
    #[test]
    fn reject_inconsistent_tag_configuration() {
        let good = config(1);
        let mut variants = Vec::new();
        let mut c = good.clone();
        c.channels = 2;
        variants.push(c);
        let mut c = good.clone();
        c.serial_channel_tags = vec!["A8".into()];
        variants.push(c);
        let mut c = good.clone();
        c.serial_sequence_tag = Some("A0".into());
        variants.push(c);
        let mut c = good.clone();
        c.serial_frame_tags.push("A0".into());
        variants.push(c);
        let mut c = good.clone();
        c.serial_frame_tags[0] = "€".into();
        variants.push(c);
        let mut c = good.clone();
        c.serial_sequence_tag = Some("A8".into());
        variants.push(c);
        for c in variants {
            assert!(TaggedU8Decoder::new(&c).is_err());
        }
        let mut c = good;
        c.serial_sequence_tag = None;
        assert!(
            TaggedU8Decoder::new(&c).expect("optional").push(&packet(2))[0]
                .as_ref()
                .expect("frame")
                .sequence
                .is_none()
        );
    }
    #[test]
    fn csv_fragments_errors_and_recovery() {
        for chunk in 1..=32 {
            let mut d = CsvDecoder::new(2);
            let input = b"1,2\r\nNaN,2\ninf,3\n1,2,3\nx,3\n4,5\n";
            let rows: Vec<_> = input.chunks(chunk).flat_map(|b| d.push(b)).collect();
            assert_eq!(rows.len(), 6);
            assert_eq!(rows[0].as_ref().expect("frame").channels, [1., 2.]);
            assert!(rows[1..5].iter().all(Result::is_err));
            assert_eq!(rows[5].as_ref().expect("frame").channels, [4., 5.]);
        }
        let mut d = CsvDecoder::new(1);
        assert!(d.push(&[b'x'; 10000]).is_empty());
        let rows = d.push(b"\n123.5\n");
        assert!(rows[0].is_err());
        assert_eq!(rows[1].as_ref().expect("frame").channels, [123.5]);
    }
    #[test]
    fn tagged_splits_marker_values_noise_and_tail() {
        let bytes = b"junkA0AA10noiseA0xAX?A01A1AA0\xffA1\0";
        for split in 0..=bytes.len() {
            let mut d = TaggedU8Decoder::default();
            let rows: Vec<_> = d
                .push(&bytes[..split])
                .into_iter()
                .chain(d.push(&bytes[split..]))
                .filter_map(Result::ok)
                .collect();
            assert_eq!(
                rows.iter().map(|r| r.channels.clone()).collect::<Vec<_>>(),
                vec![vec![65., 48.], vec![49., 65.], vec![255., 0.]]
            );
            assert!(rows
                .iter()
                .all(|r| r.sequence.is_none() && r.device_timestamp_ns.is_none()));
        }
        let mut d = TaggedU8Decoder::default();
        let frames: Vec<_> = b"A0\0A1\xff"
            .iter()
            .flat_map(|b| d.push(&[*b]))
            .filter_map(Result::ok)
            .collect();
        assert_eq!(frames.len(), 1);
        assert!(d.push(b"A0\x01A1").is_empty());
        assert_eq!(d.push(&[2])[0].as_ref().expect("tail").channels, [1., 2.]);
    }
    #[test]
    fn arbitrary_bytes_stay_bounded_and_recover() {
        let mut d = TaggedU8Decoder::default();
        let mut state = 42u32;
        for _ in 0..100000 {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            d.push(&[(state >> 24) as u8]);
            assert!(d.pending.len() < 6);
        }
        d.reset();
        assert_eq!(
            d.push(b"A0\x07A1\x09")[0].as_ref().expect("frame").channels,
            [7., 9.]
        );
    }
}
