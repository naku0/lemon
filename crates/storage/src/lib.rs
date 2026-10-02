//! Lossless wide CSV, streamed replay, and session metadata. Raw values are never filtered here.
use core_types::*;
pub mod external;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metadata {
    pub format_version: u32,
    pub started_unix_ms: u128,
    pub application_version: String,
    pub config: Config,
    pub devices: Vec<String>,
    pub channel_ids: Vec<String>,
    pub timestamp_basis: String,
    pub units: Option<String>,
    pub synthetic_seed: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<external::Provenance>,
}
impl Metadata {
    pub fn new(config: Config) -> Self {
        let synthetic = config.source.mode == "synthetic";
        let devices = if config.source.ports.is_empty() {
            vec![config.source.mode.clone()]
        } else {
            config.source.ports.clone()
        };
        Self { format_version: 1, started_unix_ms: SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |v| v.as_millis()),
            application_version: env!("CARGO_PKG_VERSION").into(),
            channel_ids: (0..config.source.channels).map(|i|format!("ch{}",i+1)).collect(), devices,
            timestamp_basis: "source-relative ns; serial-test: host arrival estimate, not hardware synchronization".into(),
            units: if synthetic { Some("arbitrary units".into()) } else { config.source.units.clone() },
            synthetic_seed: synthetic.then_some(config.source.seed), config, provenance: None,
        }
    }
}
pub struct SessionWriter {
    pub directory: PathBuf,
    raw: csv::Writer<File>,
    events: csv::Writer<File>,
    channels: usize,
    metadata: Metadata,
    initialized: bool,
}
impl SessionWriter {
    pub fn create(parent: &Path, metadata: &Metadata) -> Result<Self> {
        fs::create_dir_all(parent).map_err(|e| format!("storage directory: {e}"))?;
        let safe_name: String = metadata
            .config
            .session
            .name
            .chars()
            .take(48)
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let mut directory = None;
        for suffix in 0..1000 {
            let path = parent.join(format!(
                "{}-{}-{suffix}",
                metadata.started_unix_ms, safe_name
            ));
            match fs::create_dir(&path) {
                Ok(()) => {
                    directory = Some(path);
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(format!("storage create: {e}")),
            }
        }
        let directory = directory.ok_or("Не удалось создать уникальный каталог сеанса")?;
        Self::initialize(directory, metadata)
    }
    /// Create exactly this directory; never overwrite an existing session.
    pub fn create_at(directory: &Path, metadata: &Metadata) -> Result<Self> {
        fs::create_dir(directory).map_err(|e| {
            format!(
                "{}: {e}; choose a new output directory",
                directory.display()
            )
        })?;
        Self::initialize(directory.to_path_buf(), metadata)
    }
    fn initialize(directory: PathBuf, metadata: &Metadata) -> Result<Self> {
        let meta = File::create(directory.join("metadata.json")).map_err(|e| e.to_string())?;
        serde_json::to_writer_pretty(meta, metadata).map_err(|e| e.to_string())?;
        let mut raw =
            csv::Writer::from_path(directory.join("raw.csv")).map_err(|e| e.to_string())?;
        let mut header: Vec<String> = [
            "block_sequence",
            "block_started_ns",
            "sample_rate_hz",
            "source",
            "synchronized_clock",
            "block_flags",
            "sequence",
            "timestamp_ns",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        for ch in 1..=metadata.config.source.channels {
            for field in ["id", "device", "sequence", "timestamp_ns", "value", "flags"] {
                header.push(format!("ch{ch}_{field}"));
            }
        }
        raw.write_record(&header).map_err(|e| e.to_string())?;
        let mut events =
            csv::Writer::from_path(directory.join("events.csv")).map_err(|e| e.to_string())?;
        events
            .write_record(["timestamp_ns", "kind", "text"])
            .map_err(|e| e.to_string())?;
        Ok(Self {
            directory,
            raw,
            events,
            channels: metadata.config.source.channels,
            metadata: metadata.clone(),
            initialized: false,
        })
    }
    pub fn write_block(&mut self, block: &RawSignalBlock) -> Result<()> {
        if block.channels.len() != self.channels {
            return Err("storage: число каналов не совпадает с metadata".into());
        }
        if !self.initialized {
            self.metadata.channel_ids = block.channels.iter().map(|ch| ch.id.clone()).collect();
            self.metadata.devices = block.channels.iter().map(|ch| ch.device.clone()).collect();
            self.metadata.devices.sort();
            self.metadata.devices.dedup();
            let temp = self.directory.join("metadata.json.tmp");
            let mut file = File::create(&temp).map_err(|e| e.to_string())?;
            serde_json::to_writer_pretty(&mut file, &self.metadata).map_err(|e| e.to_string())?;
            file.sync_all().map_err(|e| e.to_string())?;
            fs::rename(temp, self.directory.join("metadata.json")).map_err(|e| e.to_string())?;
            self.initialized = true;
        }
        let count = block
            .channels
            .iter()
            .map(|c| c.samples.len())
            .max()
            .unwrap_or(0);
        let flags = serde_json::to_string(&block.flags).map_err(|e| e.to_string())?;
        for row in 0..count {
            let first = block
                .channels
                .iter()
                .find_map(|ch| ch.samples.get(row))
                .ok_or("storage: пустая строка")?;
            let mut fields = vec![
                block.sequence.to_string(),
                block.started_at.to_string(),
                block.sample_rate_hz.to_string(),
                block.source.clone(),
                block.synchronized_clock.to_string(),
                flags.clone(),
                first.sequence.to_string(),
                first.timestamp_ns.to_string(),
            ];
            for ch in &block.channels {
                fields.push(ch.id.clone());
                fields.push(ch.device.clone());
                if let Some(s) = ch.samples.get(row) {
                    fields.extend([
                        s.sequence.to_string(),
                        s.timestamp_ns.to_string(),
                        s.value.to_string(),
                        serde_json::to_string(&s.flags).map_err(|e| e.to_string())?,
                    ]);
                } else {
                    fields.extend([String::new(), String::new(), String::new(), "[]".into()]);
                }
            }
            self.raw
                .write_record(fields)
                .map_err(|e| format!("storage raw: {e}"))?;
        }
        Ok(())
    }
    pub fn write_event(&mut self, event: &Event) -> Result<()> {
        self.events
            .write_record([
                event.timestamp_ns.to_string(),
                event.kind.clone(),
                event.text.clone(),
            ])
            .map_err(|e| format!("storage event: {e}"))
    }
    pub fn flush(&mut self) -> Result<()> {
        self.raw
            .flush()
            .map_err(|e| format!("storage flush raw: {e}"))?;
        self.events
            .flush()
            .map_err(|e| format!("storage flush events: {e}"))?;
        self.raw.get_ref().sync_data().map_err(|e| e.to_string())?;
        self.events.get_ref().sync_data().map_err(|e| e.to_string())
    }
}
pub fn read_metadata(path: &Path) -> Result<Metadata> {
    let file =
        File::open(path.join("metadata.json")).map_err(|e| format!("replay metadata: {e}"))?;
    // Limit external metadata before deserialization.
    if file.metadata().map_err(|e| e.to_string())?.len() > 1_048_576 {
        return Err("metadata.json превышает 1 МБ".into());
    }
    let meta: Metadata =
        serde_json::from_reader(file).map_err(|e| format!("metadata JSON: {e}"))?;
    if !(1..=2).contains(&meta.format_version) {
        return Err("Неподдерживаемая версия записи".into());
    }
    meta.config.validate()?;
    Ok(meta)
}
pub struct SessionReader {
    pub metadata: Metadata,
    rows: csv::StringRecordsIntoIter<BoundedCsv>,
    pending: Option<csv::StringRecord>,
    last_block: Option<u64>,
}
impl SessionReader {
    pub fn open(path: &Path) -> Result<Self> {
        let metadata = read_metadata(path)?;
        let mut csv = csv::Reader::from_reader(BoundedCsv::open(&path.join("raw.csv"))?);
        let header = csv.headers().map_err(|e| e.to_string())?;
        if header.len() != 8 + metadata.config.source.channels * 6
            || header.get(0) != Some("block_sequence")
            || header.get(7) != Some("timestamp_ns")
        {
            return Err("replay: неподдерживаемый заголовок raw.csv".into());
        }
        let rows = csv.into_records();
        Ok(Self {
            metadata,
            rows,
            pending: None,
            last_block: None,
        })
    }
    pub fn next_block(&mut self) -> Result<Option<RawSignalBlock>> {
        let first = match self.pending.take() {
            Some(r) => Some(r),
            None => self.rows.next().transpose().map_err(|e| e.to_string())?,
        };
        let Some(first) = first else { return Ok(None) };
        let seq: u64 = parse(&first, 0)?;
        if self.last_block.is_some_and(|last| seq <= last) {
            return Err("replay: немонотонный номер блока".into());
        }
        let n = self.metadata.config.source.channels;
        let flags =
            serde_json::from_str(field(&first, 5)?).map_err(|e| format!("replay flags: {e}"))?;
        let mut block = RawSignalBlock {
            sequence: seq,
            started_at: parse(&first, 1)?,
            sample_rate_hz: parse(&first, 2)?,
            source: field(&first, 3)?.into(),
            synchronized_clock: parse(&first, 4)?,
            channels: Vec::new(),
            flags,
        };
        if !block.sample_rate_hz.is_finite()
            || (block.sample_rate_hz - self.metadata.config.source.sample_rate_hz).abs() > 1e-6
        {
            return Err("replay: sample rate не совпадает с metadata".into());
        }
        for ch in 0..n {
            block.channels.push(ChannelSamples {
                id: field(&first, 8 + ch * 6)?.into(),
                device: field(&first, 9 + ch * 6)?.into(),
                samples: Vec::new(),
            });
        }
        self.append(&mut block, &first)?;
        while let Some(row) = self.rows.next() {
            let row = row.map_err(|e| format!("replay CSV: {e}"))?;
            if parse::<u64>(&row, 0)? != seq {
                self.pending = Some(row);
                break;
            }
            if block.channels.iter().any(|c| c.samples.len() >= 4096) {
                return Err("replay: блок превышает 4096 отсчётов".into());
            }
            self.append(&mut block, &row)?;
        }
        self.last_block = Some(seq);
        if block.channels.iter().all(|ch| ch.samples.is_empty()) {
            return Err("replay: пустой блок".into());
        }
        Ok(Some(block))
    }
    fn append(&self, block: &mut RawSignalBlock, row: &csv::StringRecord) -> Result<()> {
        if row.len() != 8 + block.channels.len() * 6 {
            return Err("replay: неверное число полей CSV".into());
        }
        for (ch, c) in block.channels.iter_mut().enumerate() {
            let i = 8 + ch * 6;
            if field(row, i)? != c.id || field(row, i + 1)? != c.device {
                return Err("replay: идентификатор канала изменился внутри блока".into());
            }
            if field(row, i + 2)?.is_empty() {
                if !field(row, i + 3)?.is_empty()
                    || !field(row, i + 4)?.is_empty()
                    || field(row, i + 5)? != "[]"
                {
                    return Err("replay: значение без номера отсчёта".into());
                }
                continue;
            }
            c.samples.push(Sample {
                sequence: parse(row, i + 2)?,
                timestamp_ns: parse(row, i + 3)?,
                value: parse(row, i + 4)?,
                flags: serde_json::from_str(field(row, i + 5)?).map_err(|e| e.to_string())?,
            });
        }
        Ok(())
    }
}
fn field(r: &csv::StringRecord, i: usize) -> Result<&str> {
    r.get(i)
        .ok_or_else(|| format!("replay: отсутствует поле {i}"))
}
fn parse<T: std::str::FromStr>(r: &csv::StringRecord, i: usize) -> Result<T> {
    field(r, i)?
        .parse()
        .map_err(|_| format!("replay: некорректное поле {i}"))
}

/// Stops an unterminated/oversized quoted CSV record before csv can grow its buffer indefinitely.
struct BoundedCsv {
    file: File,
    bytes: usize,
    quoted: bool,
}
impl BoundedCsv {
    fn open(path: &Path) -> Result<Self> {
        Ok(Self {
            file: File::open(path).map_err(|e| format!("{}: {e}", path.display()))?,
            bytes: 0,
            quoted: false,
        })
    }
}
impl std::io::Read for BoundedCsv {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let length = buffer.len().min(4096);
        let n = std::io::Read::read(&mut self.file, &mut buffer[..length])?;
        for byte in &buffer[..n] {
            self.bytes += 1;
            if *byte == b'"' {
                self.quoted = !self.quoted;
            }
            if *byte == b'\n' && !self.quoted {
                self.bytes = 0;
            }
            if self.bytes > 65_536 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "CSV record exceeds 64 KiB",
                ));
            }
        }
        Ok(n)
    }
}
pub struct EventReader {
    rows: csv::DeserializeRecordsIntoIter<BoundedCsv, Event>,
    last_time: Option<u64>,
}
impl EventReader {
    pub fn open(path: &Path) -> Result<Self> {
        let rows = csv::Reader::from_reader(BoundedCsv::open(&path.join("events.csv"))?)
            .into_deserialize();
        Ok(Self {
            rows,
            last_time: None,
        })
    }
    pub fn next_event(&mut self) -> Result<Option<Event>> {
        let next = self
            .rows
            .next()
            .transpose()
            .map_err(|e| format!("replay events: {e}"))?;
        if let Some(event) = &next {
            if self.last_time.is_some_and(|last| event.timestamp_ns < last) {
                return Err("replay events: немонотонное время".into());
            }
            self.last_time = Some(event.timestamp_ns);
        }
        Ok(next)
    }
}
