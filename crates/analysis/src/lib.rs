//! Streaming, TUI-independent analysis of a native or externally imported session.
//! The processor and all spectral calculations are the same implementation used by live/replay.
use core_types::{ChannelSamples, Config, RawSignalBlock, SignalFlag, SignalQuality};
use processing::Processor;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    fs::{self, File},
    hash::{Hash, Hasher},
    io::Read,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use storage::{external, Metadata, SessionReader};

pub type AnalysisError = String;
#[derive(Debug, Clone)]
pub struct AnalysisInput {
    pub path: PathBuf,
}
#[derive(Debug, Clone, Default)]
pub struct AnalysisOptions {
    pub output: Option<PathBuf>,
    pub config: Option<Config>,
    pub sample_rate: Option<f64>,
    pub units: Option<String>,
    pub overwrite: bool,
    pub import: external::Options,
}
#[derive(Debug, Clone)]
pub struct AnalysisBundle {
    pub directory: PathBuf,
    pub summary: Summary,
}
impl AnalysisBundle {
    pub fn open(directory: impl Into<PathBuf>) -> Result<Self, AnalysisError> {
        let directory = directory.into();
        for name in ["manifest.json", "summary.json"] {
            let path = directory.join(name);
            if fs::metadata(&path)
                .map_err(|e| format!("analysis/open {}: {e}", path.display()))?
                .len()
                > 4 * 1024 * 1024
            {
                return Err(format!(
                    "analysis/open {}: JSON exceeds 4 MiB",
                    path.display()
                ));
            }
        }
        let manifest: serde_json::Value = serde_json::from_reader(
            File::open(directory.join("manifest.json")).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        if manifest["analysis_format_version"].as_u64() != Some(1) {
            return Err("analysis/open: unsupported analysis_format_version".into());
        }
        let summary: Summary = serde_json::from_reader(
            File::open(directory.join("summary.json")).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        for name in [
            "manifest.json",
            "processed.csv",
            "spectrum.csv",
            "band-power.csv",
            "quality-events.csv",
        ] {
            if !directory.join(name).is_file() {
                return Err(format!("analysis: missing {name}"));
            }
        }
        Ok(Self { directory, summary })
    }
}

#[derive(Debug, Serialize)]
struct Manifest {
    analysis_format_version: u32,
    lemon_version: String,
    created_at: String,
    source: String,
    source_format: String,
    source_path: String,
    source_hash: String,
    source_hashes: BTreeMap<String, String>,
    statistics_basis: String,
    mains_interval_hz: [f64; 2],
    source_metadata: Metadata,
    channels: Vec<String>,
    units: Option<String>,
    sample_rate_hz: f64,
    sample_rate_source: String,
    duration_seconds: f64,
    processing_config: core_types::ProcessingConfig,
    filter_order: String,
    filter_sequence: Vec<String>,
    spectral_config: SpectralConfig,
    configured_bands: Vec<core_types::Band>,
    baseline: Vec<Vec<f64>>,
    import_warnings: Vec<String>,
    time_normalization: String,
    analysis_warnings: Vec<String>,
    output_files: Vec<String>,
    psd_units: String,
}
#[derive(Debug, Serialize)]
struct SpectralConfig {
    window_samples: usize,
    hop_samples: usize,
    window: String,
    aggregation: String,
}
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct QualityCount {
    pub count: u64,
    pub fraction: f64,
}
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct BandSummary {
    pub low_hz: f64,
    pub high_hz: f64,
    pub mean_absolute_power: f64,
    pub median_absolute_power: Option<f64>,
    pub mean_relative_power: f64,
    pub median_relative_power: Option<f64>,
    pub min_relative_power: f64,
    pub max_relative_power: f64,
    pub mean_baseline_change_pct: Option<f64>,
    pub valid_window_count: u64,
}
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChannelSummary {
    pub raw_min: f64,
    pub raw_max: f64,
    pub raw_mean: f64,
    pub raw_stddev: f64,
    pub filtered_min: Option<f64>,
    pub filtered_max: Option<f64>,
    pub filtered_mean: Option<f64>,
    pub filtered_stddev: Option<f64>,
    pub quality_distribution: BTreeMap<String, QualityCount>,
    pub dominant_peak_frequency_hz: Option<f64>,
    pub dominant_peak_power: Option<f64>,
    pub mains_power_before: f64,
    pub mains_power_after: f64,
    pub mains_reduction_percent: Option<f64>,
    pub band_statistics: BTreeMap<String, BandSummary>,
}
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Summary {
    pub duration_seconds: f64,
    pub channel_count: usize,
    pub sample_count_by_channel: BTreeMap<String, u64>,
    pub valid_filtered_count_by_channel: BTreeMap<String, u64>,
    pub missing_count_by_channel: BTreeMap<String, u64>,
    pub lost_sample_count: u64,
    pub invalid_packet_count: u64,
    pub outlier_count: u64,
    pub saturation_count: u64,
    pub disconnection_count: u64,
    pub continuous_segment_count: u64,
    pub sample_rate_hz: f64,
    pub sample_rate_source: String,
    pub channels: BTreeMap<String, ChannelSummary>,
    pub warnings: Vec<String>,
    pub dominant_band_by_channel: BTreeMap<String, String>,
}

#[derive(Default, Clone)]
struct Running {
    n: u64,
    mean: f64,
    m2: f64,
    min: f64,
    max: f64,
    values: Vec<f64>,
}
impl Running {
    fn add(&mut self, v: f64) {
        if !v.is_finite() {
            return;
        }
        if self.n == 0 {
            self.min = v;
            self.max = v;
        } else {
            self.min = self.min.min(v);
            self.max = self.max.max(v);
        }
        self.n += 1;
        let d = v - self.mean;
        self.mean += d / self.n as f64;
        self.m2 += d * (v - self.mean);
        if self.values.len() < 100_000 {
            self.values.push(v);
        }
    }
    fn std(&self) -> f64 {
        if self.n > 1 {
            (self.m2 / (self.n - 1) as f64).sqrt()
        } else {
            0.
        }
    }
    fn median(&self) -> Option<f64> {
        if self.values.is_empty() || self.n > 100_000 {
            None
        } else {
            let mut v = self.values.clone();
            v.sort_by(f64::total_cmp);
            Some((v[(v.len() - 1) / 2] + v[v.len() / 2]) / 2.)
        }
    }
}
#[derive(Default)]
struct BandAgg {
    abs: Running,
    rel: Running,
    base: Running,
}
#[derive(Default)]
struct ChAgg {
    raw: Running,
    samples: u64,
    filtered: Running,
    quality: BTreeMap<String, u64>,
    bands: BTreeMap<String, BandAgg>,
    mean_psd: Vec<f64>,
    mains_before: Running,
    mains_after: Running,
    segments: u64,
    id: String,
    missing: u64,
    previous_sequence: Option<u64>,
    windows: u64,
    previous_time: Option<u64>,
    previous_flags: Vec<SignalFlag>,
    source: String,
    force_reset: bool,
    window_times: VecDeque<u64>,
}
#[derive(Default, Clone, Copy)]
struct FlagCounts {
    lost: u64,
    invalid: u64,
    outlier: u64,
    saturation: u64,
    disconnected: u64,
}
fn qname(q: SignalQuality) -> String {
    format!("{q:?}")
}
fn flag_name(f: SignalFlag) -> &'static str {
    match f {
        SignalFlag::LostSample => "LostSample",
        SignalFlag::InvalidPacket => "InvalidPacket",
        SignalFlag::Disconnected => "Disconnected",
        SignalFlag::Outlier => "Outlier",
        SignalFlag::Saturation => "Saturation",
        SignalFlag::LowQuality => "LowQuality",
        SignalFlag::ChannelMismatch => "ChannelMismatch",
        SignalFlag::InsufficientData => "InsufficientData",
    }
}
fn flag_text(flags: &[SignalFlag]) -> String {
    flags
        .iter()
        .map(|f| flag_name(*f))
        .collect::<Vec<_>>()
        .join("|")
}
fn source_hashes(input: &Path, meta: &Metadata) -> Result<BTreeMap<String, String>, String> {
    let files = if input.is_dir() && input.join("metadata.json").is_file() {
        ["metadata.json", "raw.csv", "events.csv"]
            .iter()
            .map(|n| input.join(n))
            .collect::<Vec<_>>()
    } else if let Some(p) = &meta.provenance {
        p.files_used.iter().map(PathBuf::from).collect()
    } else {
        vec![input.to_path_buf()]
    };
    files
        .iter()
        .map(|p| Ok((p.display().to_string(), hash_path(p)?)))
        .collect()
}
fn hash_path(path: &Path) -> Result<String, AnalysisError> {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let mut files = Vec::new();
    if path.is_dir() {
        for e in fs::read_dir(path).map_err(|e| format!("analyze/hash {}: {e}", path.display()))? {
            let p = e.map_err(|e| e.to_string())?.path();
            if p.is_file() {
                files.push(p)
            }
        }
        files.sort();
    } else {
        files.push(path.to_path_buf());
    }
    for p in files {
        p.file_name().hash(&mut h);
        let mut f = File::open(&p).map_err(|e| format!("analyze/hash {}: {e}", p.display()))?;
        let mut b = [0u8; 8192];
        loop {
            let n = f
                .read(&mut b)
                .map_err(|e| format!("analyze/hash {}: {e}", p.display()))?;
            if n == 0 {
                break;
            }
            h.write(&b[..n]);
        }
    }
    Ok(format!("{:016x}", h.finish()))
}
fn split_block(block: RawSignalBlock, size: usize) -> Vec<RawSignalBlock> {
    let max = block
        .channels
        .iter()
        .map(|c| c.samples.len())
        .max()
        .unwrap_or(0);
    (0..max)
        .step_by(size.max(1))
        .map(|off| RawSignalBlock {
            sequence: block
                .sequence
                .saturating_mul(1_000_000)
                .saturating_add(off as u64),
            started_at: block
                .channels
                .iter()
                .filter_map(|c| c.samples.get(off).map(|s| s.timestamp_ns))
                .min()
                .unwrap_or(block.started_at),
            sample_rate_hz: block.sample_rate_hz,
            source: block.source.clone(),
            synchronized_clock: block.synchronized_clock,
            channels: block
                .channels
                .iter()
                .map(|c| ChannelSamples {
                    id: c.id.clone(),
                    device: c.device.clone(),
                    samples: c.samples.iter().skip(off).take(size).cloned().collect(),
                })
                .collect(),
            flags: if off == 0 {
                block.flags.clone()
            } else {
                vec![]
            },
        })
        .collect()
}
fn source_format(meta: &Metadata) -> String {
    meta.provenance
        .as_ref()
        .map(|p| p.original_format.clone())
        .unwrap_or_else(|| "lemon-session".into())
}
pub fn analyze(
    input: AnalysisInput,
    options: AnalysisOptions,
) -> Result<AnalysisBundle, AnalysisError> {
    let base = options.config.clone().unwrap_or_default();
    let prepared = external::prepare(
        &input.path,
        &external::Options {
            sample_rate: options.sample_rate,
            units: options.units.clone(),
            ..options.import.clone()
        },
        &base,
    )?;
    let session = prepared.path.clone();
    let meta = storage::read_metadata(&session)?;
    let mut config = options
        .config
        .clone()
        .unwrap_or_else(|| meta.config.clone());
    config.source.mode = "replay".into();
    config.source.replay_path = session.display().to_string();
    config.source.channels = meta.config.source.channels;
    config.source.sample_rate_hz = options
        .sample_rate
        .unwrap_or(meta.config.source.sample_rate_hz);
    if let Some(u) = options.units.clone() {
        config.source.units = Some(u)
    }
    let sample_rate_source = if options.sample_rate.is_some() {
        "cli".to_string()
    } else {
        meta.provenance
            .as_ref()
            .map(|p| p.sample_rate_source.clone())
            .unwrap_or_else(|| "native-metadata".into())
    };
    config
        .validate()
        .map_err(|e| format!("analyze/DSP {}: {e}", input.path.display()))?;
    let output = make_output(&input.path, &options, &meta)?;
    validate_output(&input.path, &output, options.overwrite)?;
    let staging = Staging::new(&output)?;
    let temp = &staging.0;
    let mut processed =
        csv::Writer::from_path(temp.join("processed.csv")).map_err(|e| e.to_string())?;
    processed
        .write_record([
            "timestamp_ns",
            "sequence",
            "channel",
            "raw",
            "filtered",
            "quality",
            "flags",
        ])
        .map_err(|e| e.to_string())?;
    let mut spectrum =
        csv::Writer::from_path(temp.join("spectrum.csv")).map_err(|e| e.to_string())?;
    spectrum
        .write_record([
            "window_index",
            "window_start_ns",
            "window_end_ns",
            "channel",
            "frequency_hz",
            "raw_psd",
            "filtered_psd",
        ])
        .map_err(|e| e.to_string())?;
    let mut bands =
        csv::Writer::from_path(temp.join("band-power.csv")).map_err(|e| e.to_string())?;
    bands
        .write_record([
            "window_index",
            "window_start_ns",
            "window_end_ns",
            "channel",
            "band",
            "low_hz",
            "high_hz",
            "absolute",
            "relative",
            "smoothed",
            "baseline_change_pct",
        ])
        .map_err(|e| e.to_string())?;
    let mut quality =
        csv::Writer::from_path(temp.join("quality-events.csv")).map_err(|e| e.to_string())?;
    quality
        .write_record(["timestamp_ns", "channel", "kind", "severity", "details"])
        .map_err(|e| e.to_string())?;
    let mut reader = SessionReader::open(&session)?;
    // A separate existing Processor per channel exposes every frame, including an
    // asynchronous channel's startup/tail. Baselines keep their original channel index.
    let mut channel_configs = Vec::new();
    let mut processors = Vec::new();
    for ci in 0..config.source.channels {
        let mut one = config.clone();
        one.source.channels = 1;
        one.processing.baseline = config
            .processing
            .baseline
            .get(ci)
            .cloned()
            .into_iter()
            .collect();
        processors.push(Processor::new(&one)?);
        channel_configs.push(one);
    }
    let mut agg: Vec<ChAgg> = (0..config.source.channels)
        .map(|_| ChAgg::default())
        .collect();
    let mut last_time = 0;
    let mut window_index = 0u64;
    let mut processor_sequence = 0u64;
    let mut previous_block: Option<u64> = None;
    let mut first_time: Option<u64> = None;
    let mut segments = 0u64;
    let mut flag_counts = FlagCounts::default();
    let mut warnings = meta
        .provenance
        .as_ref()
        .map(|p| p.import_warnings.clone())
        .unwrap_or_default();
    for (i, left) in config.processing.bands.iter().enumerate() {
        for right in config.processing.bands.iter().skip(i + 1) {
            if left.low_hz < right.high_hz && right.low_hz < left.high_hz {
                warnings.push(format!(
                    "Configured bands {}–{} Hz and {}–{} Hz overlap; relative powers are not independent.",
                    left.low_hz, left.high_hz, right.low_hz, right.high_hz
                ));
            }
        }
    }
    for warning in &warnings {
        quality
            .write_record(["0", "", "ImportWarning", "warning", warning])
            .map_err(|e| e.to_string())?;
    }
    while let Some(mut block) = reader.next_block()? {
        if previous_block.is_some_and(|n| block.sequence != n.saturating_add(1)) {
            block.flags.push(SignalFlag::LostSample);
        }
        previous_block = Some(block.sequence);
        if block.flags.contains(&SignalFlag::Disconnected) {
            flag_counts.disconnected += 1;
            for (ci, ch) in block.channels.iter().enumerate() {
                agg[ci].force_reset = true;
                quality
                    .write_record([
                        block.started_at.to_string(),
                        ch.id.clone(),
                        "Disconnected".into(),
                        "error".into(),
                        "source disconnection".into(),
                    ])
                    .map_err(|e| e.to_string())?;
            }
        }
        for mut part in split_block(block, 1) {
            part.sequence = processor_sequence;
            processor_sequence += 1;
            for (ci, channel) in part.channels.iter().enumerate() {
                if channel.samples.is_empty() {
                    continue;
                }
                let sample = &channel.samples[0];
                let invalid_order = agg[ci]
                    .previous_sequence
                    .is_some_and(|n| sample.sequence <= n)
                    || agg[ci]
                        .previous_time
                        .is_some_and(|t| sample.timestamp_ns <= t);
                let boundary = agg[ci].force_reset
                    || (!agg[ci].source.is_empty() && agg[ci].source != part.source)
                    || sample.flags.iter().chain(part.flags.iter()).any(|f| {
                        matches!(
                            f,
                            SignalFlag::LostSample
                                | SignalFlag::InvalidPacket
                                | SignalFlag::Disconnected
                        )
                    })
                    || agg[ci].previous_time.is_some_and(|t| {
                        sample.timestamp_ns.saturating_sub(t) as f64
                            > 1.5e9 / config.source.sample_rate_hz
                    })
                    || agg[ci]
                        .previous_sequence
                        .is_some_and(|n| sample.sequence != n.saturating_add(1));
                let starts_segment = agg[ci].previous_time.is_none() || boundary;
                if boundary {
                    agg[ci].window_times.clear();
                }
                if boundary && !invalid_order {
                    processors[ci] = Processor::new(&channel_configs[ci])?;
                }
                agg[ci].source = part.source.clone();
                agg[ci].force_reset = false;
                let mut one = part.clone();
                one.channels = vec![channel.clone()];
                one.sequence = agg[ci].samples;
                one.synchronized_clock = true;
                let mut out = processors[ci].process(one)?;
                if boundary && !out.flags.contains(&SignalFlag::LostSample) {
                    out.flags.push(SignalFlag::LostSample);
                }
                let mismatch = config.source.channels == 2
                    && (!part.synchronized_clock
                        || part.channels.iter().any(|ch| ch.samples.is_empty())
                        || part.channels[0]
                            .samples
                            .first()
                            .zip(part.channels[1].samples.first())
                            .is_some_and(|(a, b)| {
                                a.timestamp_ns.abs_diff(b.timestamp_ns) as f64 / 1e6
                                    > config.processing.sync_tolerance_ms
                            }));
                if mismatch {
                    out.flags.push(SignalFlag::ChannelMismatch);
                    out.channel_quality.fill(SignalQuality::Poor);
                }
                agg[ci].previous_time = Some(sample.timestamp_ns);

                flag_counts.invalid += out.invalid_samples;
                flag_counts.outlier += out.outliers;
                if out.flags.contains(&SignalFlag::Saturation) {
                    flag_counts.saturation += 1;
                }
                last_time = out
                    .raw_channels
                    .iter()
                    .flat_map(|c| c.samples.last().map(|s| s.timestamp_ns))
                    .max()
                    .unwrap_or(last_time)
                    .max(last_time);
                for ch in &out.raw_channels {
                    agg[ci].id = ch.id.clone();
                    let filtered = out.filtered_channels.first();
                    let q = out
                        .channel_quality
                        .first()
                        .copied()
                        .unwrap_or(SignalQuality::Unknown);
                    let flags = flag_text(&out.flags);
                    for (i, s) in ch.samples.iter().enumerate() {
                        first_time =
                            Some(first_time.map_or(s.timestamp_ns, |t| t.min(s.timestamp_ns)));
                        if let Some(previous) = agg[ci].previous_sequence {
                            let lost = s.sequence.saturating_sub(previous.saturating_add(1));
                            agg[ci].missing += lost;
                            flag_counts.lost += lost;
                        }
                        agg[ci].previous_sequence = Some(s.sequence);
                        let fv = filtered.and_then(|f| f.samples.get(i)).copied().flatten();
                        processed
                            .write_record([
                                s.timestamp_ns.to_string(),
                                s.sequence.to_string(),
                                ch.id.clone(),
                                s.value.to_string(),
                                fv.map(|v| v.to_string()).unwrap_or_default(),
                                qname(q),
                                flags.clone(),
                            ])
                            .map_err(|e| e.to_string())?;
                        agg[ci].samples += 1;
                        agg[ci].raw.add(s.value);
                        if let Some(v) = fv {
                            agg[ci].filtered.add(v);
                            agg[ci].window_times.push_back(s.timestamp_ns);
                            if agg[ci].window_times.len() > config.processing.window_samples {
                                agg[ci].window_times.pop_front();
                            }
                        } else {
                            agg[ci].window_times.clear();
                        }
                    }
                    *agg[ci].quality.entry(qname(q)).or_default() += ch.samples.len() as u64;
                    if starts_segment {
                        agg[ci].segments += 1;
                        segments += 1;
                        quality
                            .write_record([
                                sample.timestamp_ns.to_string(),
                                ch.id.clone(),
                                "SegmentStart".into(),
                                "info".into(),
                                "continuous channel segment".into(),
                            ])
                            .map_err(|e| e.to_string())?;
                    }
                    for f in out
                        .flags
                        .iter()
                        .filter(|f| !agg[ci].previous_flags.contains(f))
                    {
                        let severity =
                            if matches!(f, SignalFlag::Disconnected | SignalFlag::InvalidPacket) {
                                "error"
                            } else {
                                "warning"
                            };
                        quality
                            .write_record([
                                sample.timestamp_ns.to_string(),
                                ch.id.clone(),
                                flag_name(*f).to_string(),
                                severity.to_string(),
                                "processor flag".into(),
                            ])
                            .map_err(|e| e.to_string())?;
                    }
                }
                agg[ci].previous_flags = out.flags.clone();
                if let Some(sp) = &out.spectrum {
                    let start = agg[ci].window_times.front().copied().ok_or_else(|| {
                        "analyze/DSP: spectral frame has no sample timestamps".to_string()
                    })?;
                    for csp in &sp.channels {
                        for (k, freq) in sp.frequencies.iter().enumerate() {
                            spectrum
                                .write_record([
                                    agg[ci].windows.to_string(),
                                    start.to_string(),
                                    sp.timestamp_ns.to_string(),
                                    csp.channel.clone(),
                                    freq.to_string(),
                                    csp.raw_psd.get(k).unwrap_or(&0.).to_string(),
                                    csp.filtered_psd.get(k).unwrap_or(&0.).to_string(),
                                ])
                                .map_err(|e| e.to_string())?;
                        }
                        if agg[ci].mean_psd.is_empty() {
                            agg[ci].mean_psd.resize(csp.filtered_psd.len(), 0.);
                        }
                        let count = (agg[ci].windows + 1) as f64;
                        for (mean, value) in agg[ci].mean_psd.iter_mut().zip(&csp.filtered_psd) {
                            *mean += (value - *mean) / count;
                        }
                        agg[ci].mains_before.add(
                            csp.raw_psd
                                .iter()
                                .enumerate()
                                .filter(|(k, _)| {
                                    (*k as f64 * config.source.sample_rate_hz
                                        / config.processing.window_samples as f64
                                        - config.processing.notch_hz)
                                        .abs()
                                        <= 2.
                                })
                                .map(|(_, v)| *v)
                                .sum::<f64>()
                                * config.source.sample_rate_hz
                                / config.processing.window_samples as f64,
                        );
                        agg[ci].mains_after.add(
                            csp.filtered_psd
                                .iter()
                                .enumerate()
                                .filter(|(k, _)| {
                                    (*k as f64 * config.source.sample_rate_hz
                                        / config.processing.window_samples as f64
                                        - config.processing.notch_hz)
                                        .abs()
                                        <= 2.
                                })
                                .map(|(_, v)| *v)
                                .sum::<f64>()
                                * config.source.sample_rate_hz
                                / config.processing.window_samples as f64,
                        );
                    }
                    if let Some(bp) = &out.band_power {
                        for b in bp {
                            let ch = ci;
                            let band = config.processing.bands.iter().find(|x| x.name == b.band);
                            if let Some(band) = band {
                                bands
                                    .write_record([
                                        agg[ci].windows.to_string(),
                                        start.to_string(),
                                        sp.timestamp_ns.to_string(),
                                        b.channel.clone(),
                                        b.band.clone(),
                                        band.low_hz.to_string(),
                                        band.high_hz.to_string(),
                                        b.absolute.to_string(),
                                        b.relative.to_string(),
                                        b.smoothed.to_string(),
                                        b.baseline_change_pct
                                            .map(|v| v.to_string())
                                            .unwrap_or_default(),
                                    ])
                                    .map_err(|e| e.to_string())?;
                                let a = &mut agg[ch].bands.entry(b.band.clone()).or_default();
                                a.abs.add(b.absolute);
                                a.rel.add(b.relative);
                                if let Some(v) = b.baseline_change_pct {
                                    a.base.add(v);
                                }
                            }
                        }
                    }
                    window_index += 1;
                    agg[ci].windows += 1;
                }
            }
        }
    }
    let mut events = storage::EventReader::open(&session)?;
    while let Some(event) = events.next_event()? {
        quality
            .write_record([
                event.timestamp_ns.to_string(),
                String::new(),
                format!("Source/{}", event.kind),
                "info".into(),
                event.text,
            ])
            .map_err(|e| e.to_string())?;
    }
    processed.flush().map_err(|e| e.to_string())?;
    spectrum.flush().map_err(|e| e.to_string())?;
    bands.flush().map_err(|e| e.to_string())?;
    quality.flush().map_err(|e| e.to_string())?;
    warnings.push("Quality uses one-sample blocks independently per channel; no channel asymmetry is reported. Exact band medians are null beyond 100000 windows. Complete spectral windows may contain artifacts; consult quality-events.csv.".into());
    if window_index == 0 {
        warnings.push("No complete spectral window was available; spectrum and band-power files contain headers only.".into());
    }
    let summary = build_summary(
        &agg,
        &config,
        last_time.saturating_sub(first_time.unwrap_or(last_time)),
        segments,
        flag_counts,
        window_index,
        sample_rate_source.clone(),
        &warnings,
    );
    let md = Manifest {
        analysis_format_version: 1,
        lemon_version: env!("CARGO_PKG_VERSION").into(),
        created_at: format!(
            "unix-ms:{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_millis())
        ),
        source: input.path.display().to_string(),
        source_format: source_format(&meta),
        source_path: input.path.display().to_string(),
        source_hash: hash_path(&input.path)?,
        source_hashes: source_hashes(&input.path, &meta)?,
        statistics_basis: "quality: sample-weighted, one-sample Processor per channel; missing: sequence gaps; segments: summed per channel; complete windows include artifacts; medians exact up to 100000 values then null; stddev uses n-1".into(),
        mains_interval_hz: [config.processing.notch_hz - 2., config.processing.notch_hz + 2.],
        source_metadata: meta.clone(),
        channels: agg.iter().map(|a| a.id.clone()).collect(),
        units: options.units.clone().or_else(|| meta.units.clone()),
        sample_rate_hz: config.source.sample_rate_hz,
        sample_rate_source,
        duration_seconds: last_time.saturating_sub(first_time.unwrap_or(last_time)) as f64 / 1e9,
        processing_config: config.processing.clone(),
        filter_order: "two biquads plus optional notch (Processor)".into(),
        filter_sequence: vec![
            "high-pass".into(),
            "low-pass".into(),
            "notch (optional)".into(),
        ],
        spectral_config: SpectralConfig {
            window_samples: config.processing.window_samples,
            hop_samples: config.processing.hop_samples,
            window: "Hann".into(),
            aggregation: "peak of mean filtered PSD over complete continuous windows; DC excluded; high-pass through low-pass".into(),
        },
        configured_bands: config.processing.bands.clone(),
        baseline: config.processing.baseline.clone(),
        import_warnings: meta
            .provenance
            .as_ref()
            .map(|p| p.import_warnings.clone())
            .unwrap_or_default(),
        time_normalization: meta
            .provenance
            .as_ref()
            .map(|p| p.time_normalization.clone())
            .unwrap_or_else(|| meta.timestamp_basis.clone()),
        analysis_warnings: warnings.clone(),
        output_files: vec![
            "manifest.json".to_string(),
            "summary.json".to_string(),
            "summary.md".to_string(),
            "processed.csv".to_string(),
            "spectrum.csv".to_string(),
            "band-power.csv".to_string(),
            "quality-events.csv".to_string(),
        ],
        psd_units: "Processor PSD units (signal units squared/Hz)".into(),
    };
    serde_json::to_writer_pretty(
        File::create(temp.join("manifest.json")).map_err(|e| e.to_string())?,
        &md,
    )
    .map_err(|e| e.to_string())?;
    serde_json::to_writer_pretty(
        File::create(temp.join("summary.json")).map_err(|e| e.to_string())?,
        &summary,
    )
    .map_err(|e| e.to_string())?;
    fs::write(temp.join("summary.md"), summary_markdown(&md, &summary))
        .map_err(|e| e.to_string())?;
    let backup = staging.0.with_extension("previous");
    let replacing = output.exists();
    if replacing {
        validate_output(&input.path, &output, options.overwrite)?;
        if backup.exists() {
            return Err("analyze/output: backup path already exists".into());
        }
        fs::rename(&output, &backup).map_err(|e| format!("analyze/output backup: {e}"))?;
    }
    if let Err(e) = fs::rename(temp, &output) {
        if replacing {
            fs::rename(&backup, &output).map_err(|restore| {
                format!(
                    "analyze/output: {e}; previous report retained at {}: {restore}",
                    backup.display()
                )
            })?;
        }
        return Err(format!("analyze/output publish: {e}"));
    }
    if replacing {
        fs::remove_dir_all(backup).map_err(|e| format!("analyze/output backup cleanup: {e}"))?;
    }
    Ok(AnalysisBundle {
        directory: output,
        summary,
    })
}
// Only a complete, recognizable bundle may be replaced. Never remove an input/ancestor.
fn validate_output(input: &Path, output: &Path, overwrite: bool) -> Result<(), String> {
    if !output.exists() {
        return Ok(());
    }
    let target = output.canonicalize().map_err(|e| e.to_string())?;
    let input = input.canonicalize().map_err(|e| e.to_string())?;
    if input.starts_with(&target) || target.starts_with(&input) {
        return Err("analyze/output: output overlaps input; choose a separate directory".into());
    }
    if !overwrite {
        return Err(
            "analyze/output: directory exists; choose another path or use --overwrite".into(),
        );
    }
    let allowed = [
        "manifest.json",
        "summary.json",
        "summary.md",
        "processed.csv",
        "spectrum.csv",
        "band-power.csv",
        "quality-events.csv",
    ];
    if !target.join("manifest.json").is_file() || !target.join("summary.json").is_file() {
        return Err("analyze/output: --overwrite requires an existing AnalysisBundle".into());
    }
    for entry in fs::read_dir(&target).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if !entry.file_type().map_err(|e| e.to_string())?.is_file()
            || !allowed.contains(&entry.file_name().to_string_lossy().as_ref())
        {
            return Err("analyze/output: refusing to overwrite unrelated files".into());
        }
    }
    Ok(())
}
struct Staging(PathBuf);
impl Staging {
    fn new(output: &Path) -> Result<Self, String> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let parent = output
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        for _ in 0..1000 {
            let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = parent.join(format!(
                ".lemon-analysis-{}-{n}.partial",
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.to_string()),
            }
        }
        Err("analyze/output: cannot allocate staging directory".into())
    }
}
impl Drop for Staging {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn make_output(
    _input: &Path,
    o: &AnalysisOptions,
    meta: &Metadata,
) -> Result<PathBuf, AnalysisError> {
    if let Some(p) = &o.output {
        if let Some(parent) = p.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)
                .map_err(|e| format!("analyze/output {}: {e}", parent.display()))?;
        }
        return Ok(p.clone());
    }
    let parent = Path::new(&meta.config.recording_dir);
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    for n in 0..1000 {
        let p = parent.join(format!(
            "analysis-{}-{}",
            meta.config
                .session
                .name
                .replace(|c: char| !c.is_ascii_alphanumeric(), "_"),
            n
        ));
        if !p.exists() {
            return Ok(p);
        }
    }
    Err("analyze/output: cannot create unique directory".into())
}
#[allow(clippy::too_many_arguments)]
fn build_summary(
    ag: &[ChAgg],
    config: &Config,
    last: u64,
    segments: u64,
    flags: FlagCounts,
    _windows: u64,
    sample_rate_source: String,
    w: &[String],
) -> Summary {
    let has_overlapping_bands = config.processing.bands.iter().enumerate().any(|(i, left)| {
        config.processing.bands.iter().skip(i + 1).any(|right| {
            (left.low_hz != right.low_hz || left.high_hz != right.high_hz)
                && left.low_hz < right.high_hz
                && right.low_hz < left.high_hz
        })
    });
    let mut channels = BTreeMap::new();
    let mut sample = BTreeMap::new();
    let mut valid = BTreeMap::new();
    let mut missing = BTreeMap::new();
    let mut dominant = BTreeMap::new();
    for (i, a) in ag.iter().enumerate() {
        let id = if a.id.is_empty() {
            format!("ch{}", i + 1)
        } else {
            a.id.clone()
        };
        let total = a.samples;
        sample.insert(id.clone(), total);
        valid.insert(id.clone(), a.filtered.n);
        missing.insert(id.clone(), a.missing);
        let dist = ["Good", "Acceptable", "Poor", "Disconnected", "Unknown"]
            .into_iter()
            .map(|k| {
                let n = a.quality.get(k).copied().unwrap_or(0);
                (
                    k.to_string(),
                    QualityCount {
                        count: n,
                        fraction: n as f64 / total.max(1) as f64,
                    },
                )
            })
            .collect();
        let mut bs = BTreeMap::new();
        let mut best: Option<(String, f64)> = None;
        for band in &config.processing.bands {
            if let Some(x) = a.bands.get(&band.name) {
                let r = BandSummary {
                    low_hz: band.low_hz,
                    high_hz: band.high_hz,
                    mean_absolute_power: x.abs.mean,
                    median_absolute_power: x.abs.median(),
                    mean_relative_power: x.rel.mean,
                    median_relative_power: x.rel.median(),
                    min_relative_power: x.rel.min,
                    max_relative_power: x.rel.max,
                    mean_baseline_change_pct: (x.base.n > 0).then_some(x.base.mean),
                    valid_window_count: x.rel.n,
                };
                if best
                    .as_ref()
                    .is_none_or(|(_, v)| r.mean_relative_power > *v)
                {
                    best = Some((band.name.clone(), r.mean_relative_power))
                }
                let name = config
                    .processing
                    .bands
                    .iter()
                    .filter(|b| b.low_hz == band.low_hz && b.high_hz == band.high_hz)
                    .map(|b| b.name.as_str())
                    .collect::<Vec<_>>()
                    .join("/");
                bs.insert(name, r);
            }
        }
        if !has_overlapping_bands {
            if let Some((name, _)) = best {
                dominant.insert(id.clone(),format!("Среди настроенных диапазонов наибольшая средняя относительная мощность наблюдалась в диапазоне {name}; границы {low:.1}–{high:.1} Гц.",low=config.processing.bands.iter().find(|b|b.name==name).map_or(0.,|b|b.low_hz),high=config.processing.bands.iter().find(|b|b.name==name).map_or(0.,|b|b.high_hz)));
            }
        }
        let reduction =
            (config.processing.notch && a.mains_before.n > 0 && a.mains_before.mean > 1e-18)
                .then_some((1. - a.mains_after.mean / a.mains_before.mean) * 100.);
        let peak = a
            .mean_psd
            .iter()
            .enumerate()
            .filter(|(k, _)| {
                let hz = *k as f64 * config.source.sample_rate_hz
                    / config.processing.window_samples as f64;
                *k > 0
                    && hz >= config.processing.high_pass_hz
                    && hz <= config.processing.low_pass_hz
            })
            .max_by(|a, b| a.1.total_cmp(b.1));
        channels.insert(
            id,
            ChannelSummary {
                raw_min: a.raw.min,
                raw_max: a.raw.max,
                raw_mean: a.raw.mean,
                raw_stddev: a.raw.std(),
                filtered_min: (a.filtered.n > 0).then_some(a.filtered.min),
                filtered_max: (a.filtered.n > 0).then_some(a.filtered.max),
                filtered_mean: (a.filtered.n > 0).then_some(a.filtered.mean),
                filtered_stddev: (a.filtered.n > 0).then_some(a.filtered.std()),
                quality_distribution: dist,
                dominant_peak_frequency_hz: peak.map(|(k, _)| {
                    k as f64 * config.source.sample_rate_hz
                        / config.processing.window_samples as f64
                }),
                dominant_peak_power: peak.map(|(_, p)| *p),
                mains_power_before: a.mains_before.mean,
                mains_power_after: a.mains_after.mean,
                mains_reduction_percent: reduction,
                band_statistics: bs,
            },
        );
    }
    Summary {
        duration_seconds: last as f64 / 1e9,
        channel_count: ag.len(),
        sample_count_by_channel: sample,
        valid_filtered_count_by_channel: valid,
        missing_count_by_channel: missing,
        lost_sample_count: flags.lost,
        invalid_packet_count: flags.invalid,
        outlier_count: flags.outlier,
        saturation_count: flags.saturation,
        disconnection_count: flags.disconnected,
        continuous_segment_count: segments,
        sample_rate_hz: config.source.sample_rate_hz,
        sample_rate_source,
        channels,
        warnings: w.to_vec(),
        dominant_band_by_channel: dominant,
    }
}
fn summary_markdown(m: &Manifest, s: &Summary) -> String {
    let mut out = String::from("# LEMON Signal Analysis\n\n## Recording\n\n");
    out.push_str(&format!("Source: `{}`\n\nDuration: {:.3} s\nChannels: {}\nSample rate: {:.6} Hz ({})\n\n## Import\n\nFormat: `{}`\nHash: `{}`\n\n## Processing performed\n\n- Applied high-pass filter: {} Hz.\n- Applied low-pass filter: {} Hz.\n- Applied notch filter: {}{}\n- Spectrum: Hann window, {} samples, hop {} samples.\n\n## Signal continuity and quality\n\nSegments: {}\n",m.source_path,s.duration_seconds,s.channel_count,s.sample_rate_hz,s.sample_rate_source,m.source_format,m.source_hash,m.processing_config.high_pass_hz,m.processing_config.low_pass_hz,if m.processing_config.notch{"enabled"}else{"disabled"},if m.processing_config.notch{format!(" at {} Hz",m.processing_config.notch_hz)}else{String::new()},m.spectral_config.window_samples,m.spectral_config.hop_samples,s.continuous_segment_count));
    out.push_str(&format!("\nStatistics basis: {}\n\nMains measurement interval: {}–{} Hz; PSD integrated with bin width.\n\nPeak aggregation: {}.\n", m.statistics_basis, m.mains_interval_hz[0], m.mains_interval_hz[1], m.spectral_config.aggregation));
    for (ch, c) in &s.channels {
        out.push_str(&format!("- {}: raw samples {}, valid filtered {}, dominant peak {} Hz, power reduction after processing {}%.\n",ch,s.sample_count_by_channel.get(ch).unwrap_or(&0),s.valid_filtered_count_by_channel.get(ch).unwrap_or(&0),c.dominant_peak_frequency_hz.map(|v| format!("{v:.6}")).unwrap_or_else(|| "not available".into()),c.mains_reduction_percent.map(|v| format!("{v:.3}")).unwrap_or_else(|| "not available".into())));
    }
    out.push_str("\n## Spectral measurements\n\nEach recorded spectrum is a PSD frame from the existing Processor; no cognitive interpretation is applied.\n\n## Configured frequency bands\n\n");
    for (channel, c) in &s.channels {
        out.push_str(&format!("\nChannel: {}\n\n", channel));
        for (band, b) in &c.band_statistics {
            out.push_str(&format!(
                "- {} {}–{} Hz: mean relative power {:.6}, valid windows {}.\n",
                band, b.low_hz, b.high_hz, b.mean_relative_power, b.valid_window_count
            ));
        }
    }
    out.push_str("\n## Warnings and limitations\n\n");
    for w in &s.warnings {
        out.push_str(&format!("- {}\n", w));
    }
    out.push_str("\nРезультаты описывают численные характеристики зарегистрированного сигнала и выполненную обработку. Интерпретация нейрофизиологического, когнитивного или медицинского значения выполняется профильным специалистом.\n");
    out
}
