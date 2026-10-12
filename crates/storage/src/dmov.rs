//! Verified Lab2 profile: BE u32 length, 'T', LE u32 ticks, fragmented A<digit>/u8 stream.
//! A packet timestamp is a batch timestamp, not proof of simultaneous acquisition.
use crate::{
    external::{Options, Provenance},
    Metadata, SessionWriter,
};
use core_types::*;
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs::File,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
};

const MAX_RECORD: usize = 4096;
const TICK_NS: u64 = 8192;
const VERSION: &str = "lab2-t-u32le-v1";

pub(crate) fn recognized(path: &Path) -> Result<bool> {
    let mut f = File::open(path).map_err(|e| format!("DMOV {} byte 0: {e}", path.display()))?;
    let mut b = [0; 5];
    Ok(f.read(&mut b).map_err(|e| e.to_string())? == 5 && b[4] == b'T')
}
struct Packet {
    offset: u64,
    tick: u32,
    payload: Vec<u8>,
}
struct Reader {
    file: BufReader<File>,
    path: PathBuf,
    offset: u64,
    previous: Option<u32>,
}
impl Reader {
    fn open(path: &Path) -> Result<Self> {
        Ok(Self {
            file: BufReader::new(
                File::open(path).map_err(|e| format!("DMOV {} byte 0: {e}", path.display()))?,
            ),
            path: path.into(),
            offset: 0,
            previous: None,
        })
    }
    fn error(&self, offset: u64, why: &str) -> String {
        format!("DMOV {} byte {offset}: {why}", self.path.display())
    }
    fn next(&mut self) -> Result<Option<Packet>> {
        let offset = self.offset;
        let mut length = [0; 4];
        let n = self
            .file
            .read(&mut length[..1])
            .map_err(|e| self.error(offset, &e.to_string()))?;
        if n == 0 {
            return Ok(None);
        }
        self.file
            .read_exact(&mut length[1..])
            .map_err(|_| self.error(offset, "truncated record length"))?;
        let length = u32::from_be_bytes(length) as usize;
        if !(6..=MAX_RECORD).contains(&length) {
            return Err(self.error(offset, "record length must be 6..4096 bytes"));
        }
        let mut body = vec![0; length];
        self.file
            .read_exact(&mut body)
            .map_err(|_| self.error(offset + 4, "truncated record body"))?;
        if body[0] != b'T' {
            return Err(self.error(
                offset + 4,
                "expected T timestamp marker; unsupported DMOV profile",
            ));
        }
        let tick = u32::from_le_bytes([body[1], body[2], body[3], body[4]]);
        if self.previous.is_some_and(|p| tick <= p) {
            return Err(self.error(
                offset + 5,
                "non-increasing packet timestamp; wrap/reset is not supported",
            ));
        }
        self.previous = Some(tick);
        self.offset += length as u64 + 4;
        Ok(Some(Packet {
            offset,
            tick,
            payload: body[5..].to_vec(),
        }))
    }
}
struct Field {
    tag: String,
    value: u8,
    split: bool,
}
#[derive(Default)]
struct Fields {
    pending: Vec<u8>,
    start_offset: u64,
}
impl Fields {
    fn push(&mut self, p: &Packet, path: &Path) -> Result<Vec<Field>> {
        let mut out = Vec::new();
        let mut split = !self.pending.is_empty();
        for (i, &b) in p.payload.iter().enumerate() {
            let offset = p.offset + 9 + i as u64;
            if self.pending.is_empty() {
                self.start_offset = offset;
            }
            if (self.pending.is_empty() && b != b'A')
                || (self.pending.len() == 1 && !b.is_ascii_digit())
            {
                return Err(format!("DMOV {} byte {offset}: invalid tagged field; refusing speculative resynchronization",path.display()));
            }
            self.pending.push(b);
            if self.pending.len() == 3 {
                out.push(Field {
                    tag: format!("A{}", self.pending[1] as char),
                    value: self.pending[2],
                    split,
                });
                self.pending.clear();
                split = false;
            }
        }
        Ok(out)
    }
    fn finish(&self, path: &Path) -> Result<()> {
        if self.pending.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "DMOV {} byte {}: truncated tagged field",
                path.display(),
                self.start_offset
            ))
        }
    }
}
struct Dat {
    path: PathBuf,
    reader: BufReader<File>,
    line: u64,
    pending: Option<(f64, f64)>,
    skipped: u64,
    matched: u64,
}
impl Dat {
    fn open(path: PathBuf, first_time: f64) -> Result<Option<Self>> {
        if !path.exists() {
            return Ok(None);
        }
        let reader = BufReader::new(File::open(&path).map_err(|e| e.to_string())?);
        let mut d = Self {
            path,
            reader,
            line: 0,
            pending: None,
            skipped: 0,
            matched: 0,
        };
        d.pending = d.next()?;
        while d.pending.is_some_and(|(t, _)| t < first_time - 1e-7) && d.skipped < 2 {
            d.skipped += 1;
            d.pending = d.next()?;
        }
        Ok(Some(d))
    }
    fn next(&mut self) -> Result<Option<(f64, f64)>> {
        let mut bytes = Vec::new();
        let n = self
            .reader
            .by_ref()
            .take(4097)
            .read_until(b'\n', &mut bytes)
            .map_err(|e| e.to_string())?;
        if n == 0 {
            return Ok(None);
        }
        self.line += 1;
        if n > 4096 {
            return Err(format!(
                "DMOV validation {} line {}: DAT line too long",
                self.path.display(),
                self.line
            ));
        }
        let s = std::str::from_utf8(&bytes).map_err(|e| e.to_string())?;
        let values: Vec<f64> = s
            .split_whitespace()
            .map(|s| s.parse::<f64>())
            .collect::<std::result::Result<_, _>>()
            .map_err(|e| e.to_string())?;
        if values.len() != 2 || values.iter().any(|v| !v.is_finite()) {
            return Err(format!(
                "DMOV validation {} line {}: expected finite time/value",
                self.path.display(),
                self.line
            ));
        }
        Ok(Some((values[0], values[1])))
    }
    fn compare(&mut self, tick: u32, value: u8, offset: u64) -> Result<()> {
        let expected_time = (tick as f64 * TICK_NS as f64 / 1e9) as f32 as f64;
        // Exporter's numeric encoding, used ONLY to validate DAT, never to scale imported Raw.
        let expected_value = (value as f64 / 255. * 5.) as f32 as f64;
        let good = self.pending.is_some_and(|(t, v)| {
            (t - expected_time).abs() <= 1e-7 && (v - expected_value).abs() <= 1e-7
        });
        if !good {
            return Err(format!("DMOV byte {offset}: validation against {} line {} failed (time/value mismatch); verify the companion export",self.path.display(),self.line));
        }
        self.matched += 1;
        self.pending = self.next()?;
        Ok(())
    }
}

pub(crate) fn import(
    input: &Path,
    output: Option<&Path>,
    parent: &Path,
    options: &Options,
    config: &Config,
) -> Result<PathBuf> {
    if !options.channel_columns.is_empty() || options.time_column.is_some() {
        return Err("DMOV: this verified profile extracts A0/A2 only; text column selection is not applicable".into());
    }
    let names = if options.channel_names.is_empty() {
        vec!["A0".into(), "A2".into()]
    } else {
        options.channel_names.clone()
    };
    if names.len() != 2 || names[0] == names[1] {
        return Err(
            "DMOV: provide exactly two distinct --channel-name values (A0,A2 order)".into(),
        );
    }
    let mut reader = Reader::open(input)?;
    let first = reader
        .next()?
        .ok_or_else(|| format!("DMOV {} byte 0: empty file", input.display()))?;
    let origin = first.tick as u64 * TICK_NS;
    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or("DMOV: invalid file name")?;
    let first_export_time = (origin as f64 / 1e9) as f32 as f64;
    let mut dat = [
        Dat::open(
            input.with_file_name(format!("{stem}_EMG.dat")),
            first_export_time,
        )?,
        Dat::open(
            input.with_file_name(format!("{stem}_EEG.dat")),
            first_export_time,
        )?,
    ];
    let mut fields = Fields::default();
    let mut counts = [0u64; 2];
    let mut split_counts = [0u64; 2];
    let mut first_channel = [None; 2];
    let mut last_channel = [None; 2];
    let mut unknown = BTreeMap::<String, u64>::new();
    let mut records = 0u64;
    let mut last_tick = first.tick;
    let mut packet = Some(first);
    while let Some(p) = packet {
        records += 1;
        last_tick = p.tick;
        for f in fields.push(&p, input)? {
            if let Some(ch) = ["A0", "A2"].iter().position(|t| *t == f.tag) {
                counts[ch] += 1;
                split_counts[ch] += u64::from(f.split);
                first_channel[ch].get_or_insert(p.tick);
                last_channel[ch] = Some(p.tick);
                if !f.split {
                    if let Some(d) = &mut dat[ch] {
                        d.compare(p.tick, f.value, p.offset)?;
                    }
                }
            } else {
                *unknown.entry(f.tag).or_default() += 1;
            }
        }
        packet = reader.next()?;
    }
    fields.finish(input)?;
    for d in dat.iter().flatten() {
        if d.pending.is_some() {
            return Err(format!(
                "DMOV validation {} line {}: unconsumed DAT rows",
                d.path.display(),
                d.line
            ));
        }
    }
    if counts.iter().sum::<u64>() == 0 {
        return Err(format!("DMOV {} byte 0: no A0/A2 samples", input.display()));
    }
    let rates: Vec<f64> = (0..2)
        .filter_map(|ch| {
            let span = last_channel[ch]?.checked_sub(first_channel[ch]?)?;
            (span > 0 && counts[ch] > 2)
                .then(|| (counts[ch] - 1) as f64 / (span as f64 * TICK_NS as f64 / 1e9))
        })
        .collect();
    let rate = options
        .sample_rate
        .or_else(|| (!rates.is_empty()).then(|| rates.iter().sum::<f64>() / rates.len() as f64))
        .ok_or("DMOV: insufficient timing span to estimate rate; pass --sample-rate HZ")?;
    let mut cfg = config.clone();
    cfg.source.mode = "replay".into();
    cfg.source.channels = 2;
    cfg.source.sample_rate_hz = rate;
    cfg.source.units = Some(
        options
            .units
            .clone()
            .unwrap_or_else(|| "adc_counts_u8".into()),
    );
    cfg.source.replay_path = input.display().to_string();
    cfg.validate()
        .map_err(|e| format!("DMOV {}: {e}", input.display()))?;
    let mut warnings=vec!["A0 and A2 electrode locations are unconfirmed; no automatic C3/C4 mapping.".into(),
        "Packet ticks are shared batch timestamps, not synchronized per-sample clocks; asymmetry disabled. Individual times reconstructed; original ticks and sample sequence mapping preserved in DmovPacket events.".into(),
        "DMOV has no confirmed checksum/sequence: whole missing records cannot be counted. Malformed files are rejected before session creation, without speculative recovery.".into()];
    if options.sample_rate.is_none() {
        warnings.push(format!("Sample rate {rate:.6} Hz estimated from decoded sample counts and packet span; not a device specification."));
    }
    if counts.contains(&0) {
        warnings.push(format!(
            "Selected channel has no data: counts={counts:?}; no zero fill."
        ));
    }
    if !unknown.is_empty() {
        warnings.push(format!("Nonselected transport tags ignored: {unknown:?}"));
    }
    let mut validations = serde_json::Map::new();
    let mut files = vec![input.display().to_string()];
    for (ch, tag) in ["A0", "A2"].iter().enumerate() {
        if let Some(d) = &dat[ch] {
            files.push(d.path.display().to_string());
            validations.insert((*tag).into(),json!({"file":d.path,"status":"matched","matched_rows":d.matched,"leading_rows_before_dmov":d.skipped,"cross_record_fields_recovered":split_counts[ch],"comparison":"DAT time=f32(tick*8192ns)/seconds; DAT value=f32(u8/255*5); absolute tolerance 1e-7; imported values remain u8 counts"}));
        } else {
            validations.insert((*tag).into(), json!({"status":"not_available"}));
            warnings.push(format!(
                "{tag}: companion DAT unavailable; this input has no independent DAT validation."
            ));
        }
    }
    let mut meta = Metadata::new(cfg);
    meta.format_version = 2;
    meta.channel_ids = names.clone();
    let normalization=format!("origin_tick={}; packet ticks*8192 ns retained exactly in DmovPacket events (absolute); native per-channel index/rate grid starts at first completion packet minus origin, forward gaps >10 periods retained; no interpolation/zero fill; synchronized_clock=false",origin/TICK_NS);
    meta.timestamp_basis = normalization.clone();
    meta.provenance = Some(Provenance {
        details: BTreeMap::from([
            ("source_tags".into(), json!(["A0", "A2"])),
            ("source_channel_map".into(), json!({"A0":null,"A2":null})),
            (
                "synchronization_status".into(),
                json!("shared packet clock; per-sample simultaneity unproven"),
            ),
            (
                "timestamp_source".into(),
                json!("T + LE u32 ticks, 8192 ns/tick empirically validated against DAT"),
            ),
            ("parser_version".into(), json!(VERSION)),
            ("validation_against_dat".into(), json!(validations)),
            ("packet_count".into(), json!(records)),
            ("last_tick".into(), json!(last_tick)),
        ]),
        original_format: "dmov".into(),
        original_path: input.display().to_string(),
        importer_version: env!("CARGO_PKG_VERSION").into(),
        detected_channels: names.clone(),
        detected_sample_rate_hz: rate,
        sample_rate_source: if options.sample_rate.is_some() {
            "user"
        } else {
            "estimated"
        }
        .into(),
        original_units: meta.units.clone(),
        import_warnings: warnings.clone(),
        duplicate_files_ignored: vec![],
        time_normalization: normalization,
        files_used: files,
    });
    let mut writer = if let Some(p) = output {
        SessionWriter::create_at(p, &meta)?
    } else {
        SessionWriter::create(parent, &meta)?
    };
    for text in warnings {
        writer.write_event(&Event {
            timestamp_ns: 0,
            kind: "ImportWarning".into(),
            text,
        })?;
    }
    let mut reader = Reader::open(input)?;
    let mut fields = Fields::default();
    let mut index = [0u64; 2];
    let mut sequence_gap = [0u64; 2];
    let mut extra = [0u64; 2];
    let mut previous = [None; 2];
    let mut channels: Vec<ChannelSamples> = names
        .iter()
        .map(|id| ChannelSamples {
            id: id.clone(),
            device: "dmov".into(),
            samples: vec![],
        })
        .collect();
    let mut block_sequence = 0;
    while let Some(p) = reader.next()? {
        let start = index;
        let mut completed = [0u64; 2];
        for f in fields.push(&p, input)? {
            let Some(ch) = ["A0", "A2"].iter().position(|t| *t == f.tag) else {
                continue;
            };
            let original = p.tick as u64 * TICK_NS - origin;
            let anchor = first_channel[ch].unwrap_or(p.tick) as u64 * TICK_NS - origin;
            let base = anchor + (index[ch] as f64 * 1e9 / rate).round() as u64;
            let gap =
                previous[ch].is_some_and(|old| original.saturating_sub(old) as f64 > 10e9 / rate);
            if gap {
                extra[ch] = extra[ch].max(original.saturating_sub(base));
                sequence_gap[ch] += 1;
                writer.write_event(&Event{timestamp_ns:original,kind:"ImportGap".into(),text:format!("DMOV byte {} tag {}: packet-time gap; missing count unknown; one virtual sequence gap resets DSP",p.offset,f.tag)})?;
            }
            channels[ch].samples.push(Sample {
                sequence: index[ch] + sequence_gap[ch],
                timestamp_ns: base + extra[ch],
                value: f.value as f64,
                flags: if gap {
                    vec![SignalFlag::LostSample]
                } else {
                    vec![]
                },
            });
            index[ch] += 1;
            completed[ch] += 1;
            previous[ch] = Some(original);
        }
        writer.write_event(&Event{timestamp_ns:p.tick as u64*TICK_NS-origin,kind:"DmovPacket".into(),text:json!({"byte_offset":p.offset,"tick":p.tick,"absolute_timestamp_ns":p.tick as u64*TICK_NS,"payload_bytes":p.payload.len(),"channel_order":["A0","A2"],"sample_index_start":start,"completed_samples":completed}).to_string()})?;
        if channels.iter().any(|c| c.samples.len() >= 25) {
            write_block(&mut writer, &mut channels, block_sequence, rate)?;
            block_sequence += 1;
        }
    }
    if channels.iter().any(|c| !c.samples.is_empty()) {
        write_block(&mut writer, &mut channels, block_sequence, rate)?;
    }
    writer.flush()?;
    Ok(writer.directory)
}
fn write_block(
    writer: &mut SessionWriter,
    channels: &mut [ChannelSamples],
    sequence: u64,
    rate: f64,
) -> Result<()> {
    let start = channels
        .iter()
        .filter_map(|c| c.samples.first())
        .map(|s| s.timestamp_ns)
        .min()
        .unwrap_or(0);
    writer.write_block(&RawSignalBlock {
        sequence,
        started_at: start,
        sample_rate_hz: rate,
        source: "dmov".into(),
        synchronized_clock: false,
        channels: channels
            .iter_mut()
            .map(|c| ChannelSamples {
                id: c.id.clone(),
                device: c.device.clone(),
                samples: std::mem::take(&mut c.samples),
            })
            .collect(),
        flags: vec![],
    })
}
