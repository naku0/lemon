//! Bounded, two-pass external import. All replay still uses the native SessionReader.
use crate::{EventReader, Metadata, SessionReader, SessionWriter};
use core_types::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::hash_map::DefaultHasher,
    fs::{self, File},
    hash::Hasher,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

#[derive(Debug, Clone, Default)]
pub struct Options {
    pub sample_rate: Option<f64>,
    pub time_column: Option<String>,
    pub channel_columns: Vec<String>,
    pub delimiter: String,
    pub units: Option<String>,
    pub channel_names: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provenance {
    pub original_format: String,
    pub original_path: String,
    pub importer_version: String,
    pub detected_channels: Vec<String>,
    pub detected_sample_rate_hz: f64,
    pub sample_rate_source: String,
    pub original_units: Option<String>,
    pub import_warnings: Vec<String>,
    pub duplicate_files_ignored: Vec<String>,
    pub time_normalization: String,
    pub files_used: Vec<String>,
}

/// Only owns a uniquely created temporary directory, never a user input directory.
pub struct Prepared {
    pub path: PathBuf,
    temporary: Option<PathBuf>,
}
impl Drop for Prepared {
    fn drop(&mut self) {
        if let Some(p) = &self.temporary {
            let _ = fs::remove_dir_all(p);
        }
    }
}
pub fn native_path(input: &Path) -> Option<PathBuf> {
    let p = if input.file_name().is_some_and(|n| n == "raw.csv") {
        input.parent()?
    } else {
        input
    };
    (p.join("metadata.json").is_file()
        && p.join("raw.csv").is_file()
        && p.join("events.csv").is_file())
    .then(|| p.to_path_buf())
}
pub fn prepare(input: &Path, options: &Options, config: &Config) -> Result<Prepared> {
    if let Some(path) = native_path(input) {
        crate::read_metadata(&path)?;
        return Ok(Prepared {
            path,
            temporary: None,
        });
    }
    static ID: AtomicU64 = AtomicU64::new(0);
    for _ in 0..100 {
        let dir = std::env::temp_dir().join(format!(
            "lemon-import-{}-{}",
            std::process::id(),
            ID.fetch_add(1, Ordering::Relaxed)
        ));
        match fs::create_dir(&dir) {
            Ok(()) => {
                let prepared = Prepared {
                    path: dir.join("session"),
                    temporary: Some(dir),
                };
                import(input, Some(&prepared.path), Path::new("."), options, config)?;
                return Ok(prepared);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("temporary import: {e}")),
        }
    }
    Err("Cannot create temporary import directory".into())
}

struct Lines {
    path: PathBuf,
    reader: BufReader<File>,
    line: u64,
}
impl Lines {
    fn open(path: &Path) -> Result<Self> {
        Ok(Self {
            path: path.into(),
            reader: BufReader::new(
                File::open(path).map_err(|e| format!("{}: {e}", path.display()))?,
            ),
            line: 0,
        })
    }
    fn next(&mut self) -> Result<Option<String>> {
        loop {
            let mut bytes = Vec::new();
            let n = self
                .reader
                .by_ref()
                .take(65_537)
                .read_until(b'\n', &mut bytes)
                .map_err(|e| self.error(&e.to_string()))?;
            if n == 0 {
                return Ok(None);
            }
            self.line += 1;
            if n > 65_536 {
                return Err(self.error("line exceeds 64 KiB; split or fix the input"));
            }
            let s = String::from_utf8(bytes).map_err(|_| self.error("expected UTF-8 text"))?;
            let s = s.trim().trim_start_matches('\u{feff}').to_string();
            if !s.is_empty() {
                return Ok(Some(s));
            }
        }
    }
    fn error(&self, reason: &str) -> String {
        format!("{} line {}: {reason}", self.path.display(), self.line)
    }
}
#[derive(Clone)]
struct Layout {
    delimiter: Option<u8>, // None = whitespace
    columns: usize,
    header: bool,
    time: Option<usize>,
    time_scale: u64,
    channels: Vec<usize>,
    names: Vec<String>,
    dat: bool,
}
fn fields(line: &str, delimiter: Option<u8>) -> Result<Vec<String>> {
    let v: Vec<String> = if let Some(d) = delimiter {
        let mut r = csv::ReaderBuilder::new()
            .has_headers(false)
            .delimiter(d)
            .from_reader(line.as_bytes());
        r.records()
            .next()
            .transpose()
            .map_err(|e| format!("CSV: {e}"))?
            .ok_or("CSV: empty record")?
            .iter()
            .map(|s| s.trim().to_string())
            .collect()
    } else {
        line.split_whitespace().map(str::to_string).collect()
    };
    if v.len() > 32 || v.iter().any(|s| s.len() > 1024) {
        return Err("maximum 32 fields, 1024 bytes per field".into());
    }
    Ok(v)
}
fn channel_id(path: &Path) -> Option<&'static str> {
    let name = path.file_stem()?.to_str()?;
    let base = name.strip_suffix("_1").unwrap_or(name);
    if base.ends_with("A0") || base.ends_with("А0") {
        Some("A0")
    } else if base.ends_with("A1") || base.ends_with("А1") {
        Some("A1")
    } else {
        None
    }
}
fn layout(path: &Path, options: &Options, dat: bool) -> Result<Layout> {
    let mut reader = Lines::open(path)?;
    let first = reader
        .next()?
        .ok_or_else(|| format!("{}: empty input", path.display()))?;
    let delimiter = if dat {
        None
    } else {
        match options.delimiter.as_str() {
            "" | "auto" => {
                let found: Vec<_> = [b',', b';', b'\t']
                    .into_iter()
                    .filter(|d| first.contains(*d as char))
                    .collect();
                if found.len() > 1 {
                    return Err(reader.error(
                        "CSV: ambiguous delimiter; use --delimiter comma|semicolon|tab|whitespace",
                    ));
                }
                found.first().copied()
            }
            "comma" => Some(b','),
            "semicolon" => Some(b';'),
            "tab" => Some(b'\t'),
            "whitespace" => None,
            _ => return Err("CSV: unknown --delimiter".into()),
        }
    };
    let f = fields(&first, delimiter).map_err(|e| reader.error(&e))?;
    if dat && (f.len() != 2 || f.iter().any(|s| s.parse::<f64>().is_err())) {
        return Err(reader.error("DAT: expected time and value (no header); fix this line"));
    }
    let header = !dat && f.iter().all(|s| !s.is_empty() && s.parse::<f64>().is_err());
    let lookup = |s: &str| -> Result<usize> {
        let i = s
            .parse::<usize>()
            .ok()
            .or_else(|| header.then(|| f.iter().position(|n| n == s)).flatten())
            .ok_or_else(|| {
                reader.error(&format!(
                    "CSV: column {s:?} not found; use a header name or zero-based index"
                ))
            })?;
        if i >= f.len() {
            return Err(reader.error("column index out of bounds"));
        }
        Ok(i)
    };
    let known_time = |s: &str| {
        matches!(
            s,
            "time" | "timestamp" | "timestamp_s" | "timestamp_ms" | "timestamp_ns"
        )
    };
    let time = if let Some(s) = &options.time_column {
        Some(lookup(s)?)
    } else if dat || (!header && matches!(f.len(), 2 | 3) && options.channel_columns.is_empty()) {
        Some(0)
    } else if header {
        let found: Vec<_> = f
            .iter()
            .enumerate()
            .filter(|(_, s)| known_time(s))
            .map(|(i, _)| i)
            .collect();
        if found.len() > 1 {
            return Err(reader.error("CSV: multiple timestamp columns; use --time-column"));
        }
        found.first().copied()
    } else {
        None
    };
    let channels = if options.channel_columns.is_empty() {
        let candidates: Vec<_> = (0..f.len()).filter(|i| Some(*i) != time).collect();
        if !(1..=2).contains(&candidates.len()) || (!header && time.is_none()) {
            return Err(reader.error("CSV: ambiguous columns; example: lemon import input.csv --sample-rate 250 --channel-column 0"));
        }
        candidates
    } else {
        options
            .channel_columns
            .iter()
            .map(|s| lookup(s))
            .collect::<Result<Vec<_>>>()?
    };
    if !(1..=2).contains(&channels.len())
        || channels.iter().any(|i| Some(*i) == time)
        || (channels.len() == 2 && channels[0] == channels[1])
    {
        return Err(reader.error("choose one or two distinct signal columns, separate from time"));
    }
    if time.is_none() && options.sample_rate.is_none() {
        return Err(
            reader.error("CSV: timestamp column not found; use --time-column or --sample-rate 250")
        );
    }
    if !options.channel_names.is_empty() && options.channel_names.len() != channels.len() {
        return Err(reader.error("--channel-name count must match selected channels"));
    }
    let names = if !options.channel_names.is_empty() {
        options.channel_names.clone()
    } else if dat {
        vec![channel_id(path).unwrap_or("ch1").into()]
    } else {
        channels
            .iter()
            .enumerate()
            .map(|(n, i)| {
                if header {
                    f[*i].clone()
                } else {
                    format!("ch{}", n + 1)
                }
            })
            .collect()
    };
    let time_scale = match time.filter(|_| header).map(|i| f[i].as_str()) {
        Some("timestamp_ms") => 1_000_000,
        Some("timestamp_ns") => 1,
        _ => 1_000_000_000,
    };
    Ok(Layout {
        delimiter,
        columns: f.len(),
        header,
        time,
        time_scale,
        channels,
        names,
        dat,
    })
}
// Decimal conversion avoids rounding epoch nanoseconds through f64.
fn timestamp(text: &str, scale: u64) -> Result<u64> {
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    let w = whole
        .parse::<u64>()
        .map_err(|_| "time must be a nonnegative decimal number")?;
    if !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return Err("invalid decimal timestamp".into());
    }
    let digits = scale.ilog10() as usize;
    if fraction.len() > digits && fraction[digits..].bytes().any(|b| b != b'0') {
        return Err("timestamp precision exceeds nanoseconds".into());
    }
    let fraction = &fraction[..fraction.len().min(digits)];
    let frac = if fraction.is_empty() {
        0
    } else {
        fraction.parse::<u64>().map_err(|_| "invalid fraction")?
    };
    let part = frac
        .checked_mul(10u64.pow((digits - fraction.len()) as u32))
        .ok_or("timestamp overflow")?;
    w.checked_mul(scale)
        .and_then(|v| v.checked_add(part))
        .ok_or_else(|| "timestamp overflow".into())
}
struct Row {
    time: Option<u64>,
    values: Vec<Option<f64>>,
}
struct Rows {
    lines: Lines,
    layout: Layout,
    last_time: Option<u64>,
}
impl Rows {
    fn open(path: &Path, layout: &Layout) -> Result<Self> {
        let mut lines = Lines::open(path)?;
        if layout.header {
            lines.next()?;
        }
        Ok(Self {
            lines,
            layout: layout.clone(),
            last_time: None,
        })
    }
    fn next(&mut self) -> Result<Option<Row>> {
        let Some(line) = self.lines.next()? else {
            return Ok(None);
        };
        let f = fields(&line, self.layout.delimiter).map_err(|e| self.lines.error(&e))?;
        let result = (|| {
            if f.len() != self.layout.columns {
                return Err(format!(
                    "{}: expected {} fields, got {}; fix the row",
                    if self.layout.dat { "DAT" } else { "CSV" },
                    self.layout.columns,
                    f.len()
                ));
            }
            let time = self
                .layout
                .time
                .map(|i| timestamp(&f[i], self.layout.time_scale))
                .transpose()?;
            if let Some(t) = time {
                if self
                    .last_time
                    .is_some_and(|last| t < last || (!self.layout.dat && t == last))
                {
                    return Err(
                        "time must increase (DAT allows repeated timestamps); fix the row order"
                            .into(),
                    );
                }
                self.last_time = Some(t);
            }
            let values = self
                .layout
                .channels
                .iter()
                .map(|i| {
                    if f[*i].is_empty() && !self.layout.dat {
                        return Ok(None);
                    }
                    let v = f[*i]
                        .parse::<f64>()
                        .map_err(|_| format!("invalid numeric value in column {i}"))?;
                    if !v.is_finite() {
                        return Err(format!("non-finite value in column {i}; fix the input"));
                    }
                    Ok(Some(v))
                })
                .collect::<Result<_>>()?;
            Ok(Some(Row { time, values }))
        })();
        result.map_err(|e: String| self.lines.error(&e))
    }
}
#[derive(Default)]
struct Stats {
    count: u64,
    first: Option<u64>,
    last: Option<u64>,
    repeats: u64,
    missing: u64,
    rates: Vec<f64>,
    anchor: Option<u64>,
}
fn scan(path: &Path, layout: &Layout) -> Result<Stats> {
    let mut r = Rows::open(path, layout)?;
    let mut s = Stats::default();
    while let Some(row) = r.next()? {
        if let Some(t) = row.time {
            if s.last
                .is_some_and(|last| t < last || (!layout.dat && t == last))
            {
                return Err(r.lines.error("time is not strictly increasing (DAT permits equal times); fix ordering, do not sort amplitudes silently"));
            }
            s.repeats += u64::from(s.last == Some(t));
            s.first.get_or_insert(t);
            if layout.dat {
                if s.count % 256 == 0 {
                    if let Some(a) = s.anchor {
                        if t > a && s.rates.len() < 4096 {
                            s.rates.push(256e9 / (t - a) as f64);
                        }
                    }
                    s.anchor = Some(t);
                }
            } else if let Some(last) = s.last {
                if s.rates.len() < 4096 {
                    s.rates.push(1e9 / (t - last) as f64);
                }
            }
            s.last = Some(t);
        }
        s.missing += row.values.iter().filter(|v| v.is_none()).count() as u64;
        s.count += 1;
    }
    if s.count == 0 || s.missing == s.count * layout.channels.len() as u64 {
        return Err(format!("{}: no data rows", path.display()));
    }
    Ok(s)
}
fn estimate(s: &Stats, dat: bool, path: &Path) -> Result<f64> {
    let mut rates = s.rates.clone();
    rates.sort_by(f64::total_cmp);
    let reliable = if dat {
        rates.len() >= 3
    } else {
        !rates.is_empty()
    };
    if !reliable {
        return Err(format!(
            "{} {}: sample rate cannot be determined reliably; pass --sample-rate HZ",
            path.display(),
            if dat { "DAT" } else { "CSV" }
        ));
    }
    let median = rates[rates.len() / 2];
    if (rates[rates.len() * 3 / 4] - rates[rates.len() / 4]) / median > 0.15 {
        return Err(format!(
            "{}: unstable sampling estimate; pass --sample-rate HZ",
            path.display()
        ));
    }
    Ok(median)
}
fn fingerprint(path: &Path) -> Result<u64> {
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut hash = DefaultHasher::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = file.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hash.write(&buf[..n]);
    }
    Ok(hash.finish())
}
fn identical(a: &Path, b: &Path) -> Result<bool> {
    if fs::metadata(a).map_err(|e| e.to_string())?.len()
        != fs::metadata(b).map_err(|e| e.to_string())?.len()
        || fingerprint(a)? != fingerprint(b)?
    {
        return Ok(false);
    }
    // Hash is only a prefilter: byte comparison also excludes collisions.
    let mut a = BufReader::new(File::open(a).map_err(|e| e.to_string())?);
    let mut b = BufReader::new(File::open(b).map_err(|e| e.to_string())?);
    loop {
        let aa = a.fill_buf().map_err(|e| e.to_string())?;
        let bb = b.fill_buf().map_err(|e| e.to_string())?;
        if aa != bb {
            return Ok(false);
        }
        let n = aa.len();
        if n == 0 {
            return Ok(true);
        }
        a.consume(n);
        b.consume(n);
    }
}
fn discover(input: &Path) -> Result<(Vec<PathBuf>, Vec<String>, String)> {
    let ext = input
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !input.is_dir() && ext != "dmov" {
        if ext != "dat" && ext != "csv" {
            return Err(format!(
                "{}: supported inputs: LEMON session, CSV, DAT, DMOV, BiTronics directory",
                input.display()
            ));
        }
        return Ok((vec![input.into()], vec![], ext));
    }
    if ext == "dmov" && !input.is_file() {
        return Err(format!("DMOV {}: file not found", input.display()));
    }
    let dir = if input.is_dir() {
        input
    } else {
        input.parent().unwrap_or(Path::new("."))
    };
    let mut dat = Vec::new();
    let mut dmov = 0;
    for entry in fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let p = entry.map_err(|e| e.to_string())?.path();
        if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("dat")) {
            dat.push(p);
        } else if p
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("dmov"))
        {
            dmov += 1;
        }
        if dat.len() > 32 {
            return Err(format!(
                "{}: too many DAT candidates; select one file explicitly",
                dir.display()
            ));
        }
    }
    if dat.is_empty() {
        return Err(format!("DMOV {}: распознан как запись BiTronics, но прямое декодирование этой версии пока не поддерживается. Связанные DAT-файлы не найдены.",input.display()));
    }
    if dmov > 1 {
        return Err(format!("{}: multiple DMOV sessions; select DAT explicitly or place one session in its own directory",dir.display()));
    }
    dat.sort();
    let mut files: Vec<PathBuf> = Vec::new();
    let mut ignored = Vec::new();
    for p in &dat {
        if let Some(stem) = p
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| s.strip_suffix("_1"))
        {
            let base = p.with_file_name(format!("{stem}.dat"));
            if base.is_file() {
                if identical(p, &base)? {
                    ignored.push(p.display().to_string());
                    continue;
                }
                return Err(format!(
                    "DAT {} differs from {}; not ignored. Select the desired DAT explicitly",
                    p.display(),
                    base.display()
                ));
            }
        }
        if channel_id(p).is_none() {
            return Err(format!(
                "DAT {}: channel identity ambiguous; select file with --channel-name",
                p.display()
            ));
        }
        if files.iter().any(|f| channel_id(f) == channel_id(p)) {
            return Err(format!(
                "{}: multiple files for a channel; select a DAT explicitly",
                dir.display()
            ));
        }
        files.push(p.clone());
    }
    files.sort_by_key(|p| channel_id(p));
    if files.len() > 2 {
        return Err("BiTronics: at most two channels".into());
    }
    Ok((
        files,
        ignored,
        if ext == "dmov" {
            "dmov-via-dat"
        } else {
            "bitronics-directory"
        }
        .into(),
    ))
}

struct Input {
    rows: Rows,
    stats: Stats,
    pending: Option<(u64, Vec<Option<f64>>, u64, bool)>,
    index: u64,
    first_channel: usize,
    last_original: Option<u64>,
    extra_ns: u64,
    sequence_offset: u64,
}
impl Input {
    fn advance(&mut self, origin: u64, rate: f64) -> Result<()> {
        self.pending = None;
        while let Some(row) = self.rows.next()? {
            let original = row.time;
            let regular = (self.index as f64 * 1e9 / rate).round();
            if regular >= u64::MAX as f64 {
                return Err(self
                    .rows
                    .lines
                    .error("recording duration exceeds timestamp range"));
            }
            let mut gap = false;
            let time = if self.rows.layout.dat {
                // Independent uniform grids anchored at each channel's first recorded timestamp.
                let offset = self.stats.first.unwrap_or(origin).saturating_sub(origin);
                let base = offset
                    .checked_add(regular as u64)
                    .and_then(|v| v.checked_add(self.extra_ns))
                    .ok_or("time overflow")?;
                if let (Some(last), Some(t)) = (self.last_original, original) {
                    if (t - last) as f64 > 10e9 / rate && t.saturating_sub(origin) > base {
                        self.extra_ns = self
                            .extra_ns
                            .checked_add(t - origin - base)
                            .ok_or("time overflow")?;
                        gap = true;
                    }
                }
                offset
                    .checked_add(regular as u64)
                    .and_then(|v| v.checked_add(self.extra_ns))
                    .ok_or("time overflow")?
            } else {
                if let (Some(last), Some(t)) = (self.last_original, original) {
                    gap = (t - last) as f64 > 2.5e9 / rate;
                }
                match original {
                    Some(t) => t
                        .checked_sub(origin)
                        .ok_or("input changed between import passes: time before origin")?,
                    None => regular as u64,
                }
            };
            self.last_original = original;
            if gap {
                self.sequence_offset = self
                    .sequence_offset
                    .checked_add(1)
                    .ok_or("sequence overflow")?;
            }
            let index = self
                .index
                .checked_add(self.sequence_offset)
                .ok_or("sequence overflow")?;
            self.index += 1;
            if row.values.iter().any(Option::is_some) {
                self.pending = Some((time, row.values, index, gap));
                break;
            }
        }
        Ok(())
    }
}

pub fn import(
    input: &Path,
    output: Option<&Path>,
    parent: &Path,
    options: &Options,
    config: &Config,
) -> Result<PathBuf> {
    if options
        .channel_names
        .iter()
        .any(|s| s.is_empty() || s.len() > 128)
        || options.units.as_ref().is_some_and(|s| s.len() > 128)
    {
        return Err("import: names/units must be at most 128 bytes, names nonempty".into());
    }
    if let Some(native) = native_path(input) {
        let mut reader = SessionReader::open(&native)?;
        let mut metadata = reader.metadata.clone();
        metadata.format_version = 2;
        if metadata.provenance.is_none() {
            metadata.provenance = Some(Provenance {
                original_format: "lemon-session".into(),
                original_path: input.display().to_string(),
                importer_version: env!("CARGO_PKG_VERSION").into(),
                detected_channels: metadata.channel_ids.clone(),
                detected_sample_rate_hz: metadata.config.source.sample_rate_hz,
                sample_rate_source: "native-metadata".into(),
                original_units: metadata.units.clone(),
                import_warnings: vec![],
                duplicate_files_ignored: vec![],
                time_normalization: "None; native per-channel timestamps and sequences preserved"
                    .into(),
                files_used: ["metadata.json", "raw.csv", "events.csv"]
                    .iter()
                    .map(|s| native.join(s).display().to_string())
                    .collect(),
            });
        }
        let mut writer = if let Some(p) = output {
            SessionWriter::create_at(p, &metadata)?
        } else {
            SessionWriter::create(parent, &metadata)?
        };
        let mut events = EventReader::open(&native)?;
        while let Some(b) = reader.next_block()? {
            writer.write_block(&b)?;
        }
        while let Some(e) = events.next_event()? {
            writer.write_event(&e)?;
        }
        writer.flush()?;
        return Ok(writer.directory);
    }
    let (files, duplicates, format) = discover(input)?;
    if !options.channel_names.is_empty()
        && files.len() > 1
        && options.channel_names.len() != files.len()
    {
        return Err("--channel-name count must match DAT files".into());
    }
    let mut inputs = Vec::new();
    let mut names = Vec::new();
    let mut rates = Vec::new();
    let mut warnings = Vec::new();
    for (i, path) in files.iter().enumerate() {
        let dat = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("dat"));
        let mut per_file = options.clone();
        if files.len() > 1 && !options.channel_names.is_empty() {
            per_file.channel_names = vec![options.channel_names[i].clone()];
        }
        let l = layout(path, &per_file, dat)?;
        let stats = scan(path, &l)?;
        let rate = options
            .sample_rate
            .map(Ok)
            .unwrap_or_else(|| estimate(&stats, dat, path))?;
        if !rate.is_finite() || !(2.0..=100_000.0).contains(&rate) {
            return Err(format!(
                "{}: sample rate must be 2..100000 Hz",
                path.display()
            ));
        }
        rates.push(rate);
        if stats.repeats > 0 {
            warnings.push(format!(
                "{}: {} repeated timestamps; individual acquisition times are unknown",
                path.display(),
                stats.repeats
            ));
        }
        if stats.missing > 0 {
            warnings.push(format!(
                "{}: {} empty cells retained as missing samples (no zero fill)",
                path.display(),
                stats.missing
            ));
        }
        let first_channel = names.len();
        names.extend(l.names.clone());
        inputs.push(Input {
            rows: Rows::open(path, &l)?,
            stats,
            pending: None,
            index: 0,
            first_channel,
            last_original: None,
            extra_ns: 0,
            sequence_offset: 0,
        });
    }
    if names.len() > 2 || names.is_empty() || (names.len() == 2 && names[0] == names[1]) {
        return Err("import: expected 1..2 distinct channel names".into());
    }
    let rate = rates.iter().sum::<f64>() / rates.len() as f64;
    if rates.iter().any(|r| (r - rate).abs() / rate > 0.10) {
        return Err(
            "DAT: channel rate estimates differ by >10%; specify --sample-rate only if known"
                .into(),
        );
    }
    let dat = inputs.iter().all(|i| i.rows.layout.dat);
    if options.sample_rate.is_none() {
        warnings.push(format!("Sampling rate {rate:.6} Hz is ESTIMATED assuming input time is in seconds (CSV timestamp_ms/ns use named units); not a hardware specification. Per-file estimates: {rates:?}"));
    }
    if dat {
        warnings.push("DAT time is reconstructed per channel; amplitudes are unchanged. Channels are NOT hardware-synchronized; asymmetry is disabled.".into());
    }
    if format.contains("dmov") || format == "bitronics-directory" {
        warnings.push("DMOV binary decoding is not supported; selected the sole DAT channel set in this directory.".into());
    }
    let origin = inputs
        .iter()
        .filter_map(|i| i.stats.first)
        .min()
        .unwrap_or(0);
    let mut cfg = config.clone();
    cfg.source.mode = "replay".into();
    cfg.source.channels = names.len();
    cfg.source.sample_rate_hz = rate;
    cfg.source.units = options.units.clone();
    cfg.source.replay_path = input.display().to_string();
    cfg.validate().map_err(|e|format!("import {} at {rate:.6} Hz: {e}; provide a compatible experiment with --config FILE (do not change the sample rate to bypass Nyquist)",input.display()))?;
    let mut meta = Metadata::new(cfg);
    meta.format_version = 2;
    meta.channel_ids = names.clone();
    meta.units = options.units.clone();
    meta.provenance = Some(Provenance {
        original_format: format.clone(),
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
        original_units: options.units.clone(),
        import_warnings: warnings.clone(),
        duplicate_files_ignored: duplicates.clone(),
        time_normalization: if dat {
            format!("origin={origin} ns; independent index/rate grids anchored to each file's first time, forward gaps >10 nominal periods retained when beyond reconstructed time; no interpolation; sequence=row index plus one virtual gap per time discontinuity; actual lost count unknown; cross-device synchronization unknown")
        } else {
            format!("origin={origin} ns subtracted from exact decimal timestamps; absent time=index/rate; empty values omitted; sequence=row index plus one virtual gap for each interval >2.5 nominal periods; actual lost count unknown")
        },
        files_used: files.iter().map(|p| p.display().to_string()).collect(),
    });
    meta.timestamp_basis = meta
        .provenance
        .as_ref()
        .map_or(String::new(), |p| p.time_normalization.clone());
    // Validate all input before creating permanent output. A second bounded pass writes blocks.
    let mut writer = if let Some(p) = output {
        SessionWriter::create_at(p, &meta)?
    } else {
        SessionWriter::create(parent, &meta)?
    };
    for text in warnings.iter().chain(duplicates.iter()) {
        writer.write_event(&Event {
            timestamp_ns: 0,
            kind: "ImportWarning".into(),
            text: text.clone(),
        })?;
    }
    for path in &files {
        writer.write_event(&Event {
            timestamp_ns: 0,
            kind: "ImportFile".into(),
            text: path.display().to_string(),
        })?;
    }
    for item in &mut inputs {
        item.advance(origin, rate)?;
    }
    let mut sequence = 0;
    while let Some(start) = inputs
        .iter()
        .filter_map(|i| i.pending.as_ref().map(|p| p.0))
        .min()
    {
        let end = start.saturating_add((25e9 / rate).round() as u64);
        let mut channels: Vec<_> = names
            .iter()
            .map(|id| ChannelSamples {
                id: id.clone(),
                device: format.clone(),
                samples: Vec::new(),
            })
            .collect();
        let mut flags = Vec::new();
        let mut rows = 0;
        while let Some(i) = inputs
            .iter()
            .enumerate()
            .filter_map(|(i, item)| item.pending.as_ref().map(|p| (i, p.0)))
            .filter(|(_, t)| *t < end)
            .min_by_key(|(_, t)| *t)
            .map(|(i, _)| i)
        {
            if rows >= 256 {
                break;
            }
            rows += 1;
            let item = &mut inputs[i];
            let (time, values, index, gap) = item.pending.take().ok_or("import: missing row")?;
            if gap {
                flags.push(SignalFlag::LostSample);
                writer.write_event(&Event {timestamp_ns:time,kind:"ImportGap".into(),text:format!("{}: time discontinuity; inserted one virtual sequence gap to reset DSP. Actual missing sample count is unknown.",item.rows.lines.path.display())})?;
            }
            for (j, value) in values.into_iter().enumerate() {
                if let Some(value) = value {
                    channels[item.first_channel + j].samples.push(Sample {
                        sequence: index,
                        timestamp_ns: time,
                        value,
                        flags: if gap {
                            vec![SignalFlag::LostSample]
                        } else {
                            vec![]
                        },
                    });
                }
            }
            item.advance(origin, rate)?;
        }
        let block = RawSignalBlock {
            sequence,
            started_at: start,
            sample_rate_hz: rate,
            source: format.clone(),
            synchronized_clock: !dat,
            channels,
            flags,
        };
        writer.write_block(&block)?;
        sequence += 1;
    }
    writer.flush()?;
    Ok(writer.directory)
}

/// Long form preserves each channel's exact timestamp. No alignment or interpolation.
pub fn export_csv(session: &Path, output: &Path) -> Result<()> {
    let mut reader = SessionReader::open(&native_path(session).unwrap_or_else(|| session.into()))?;
    let file = File::options()
        .write(true)
        .create_new(true)
        .open(output)
        .map_err(|e| {
            format!(
                "CSV export {}: {e}; choose a new output file",
                output.display()
            )
        })?;
    let mut csv = csv::Writer::from_writer(file);
    csv.write_record(["timestamp", "channel", "value"])
        .map_err(|e| e.to_string())?;
    while let Some(block) = reader.next_block()? {
        let mut samples: Vec<_> = block
            .channels
            .iter()
            .flat_map(|c| c.samples.iter().map(move |s| (s, &c.id)))
            .collect();
        samples.sort_by_key(|(s, _)| s.timestamp_ns);
        for (s, id) in samples {
            csv.write_record([
                format!(
                    "{}.{:09}",
                    s.timestamp_ns / 1_000_000_000,
                    s.timestamp_ns % 1_000_000_000
                ),
                id.clone(),
                s.value.to_string(),
            ])
            .map_err(|e| e.to_string())?;
        }
    }
    csv.flush().map_err(|e| e.to_string())
}
