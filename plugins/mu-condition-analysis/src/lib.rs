//! Descriptive Mu/Beta comparison between separate AnalysisBundle conditions.
mod plot;

use lemon_plugin_sdk::{
    write_plugin_result, Artifact, ArtifactKind, Bundle, PluginResult, PluginStatus,
};
use rustfft::{num_complex::Complex, FftPlanner};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    fs::{self, File},
    path::PathBuf,
};

pub const PLUGIN_ID: &str = "mu-condition-analysis";
pub const PLUGIN_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone)]
pub struct Arguments {
    pub input: PathBuf,
    pub config: PathBuf,
    pub output: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub mode: String,
    pub channels: Vec<String>,
    pub bands: Bands,
    pub window_seconds: f64,
    pub hop_seconds: f64,
    pub baseline: Baseline,
    pub conditions: Vec<Condition>,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bands {
    pub mu_hz: [f64; 2],
    pub beta_hz: [f64; 2],
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Baseline {
    pub label: String,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Condition {
    pub label: String,
    pub analysis_bundle: PathBuf,
}

#[derive(Debug, Clone)]
struct Sample {
    timestamp_ns: u64,
    sequence: u64,
    value: Option<f64>,
    quality: String,
    flags: String,
}
#[derive(Debug, Clone)]
struct Event {
    timestamp_ns: u64,
    channel: Option<String>,
    reason: String,
}
type SamplesByChannel = BTreeMap<String, Vec<Sample>>;
#[derive(Debug, Clone, Serialize, Deserialize)]
struct WindowPower {
    bundle: String,
    condition: String,
    channel: String,
    window_index: usize,
    window_start_ns: u64,
    window_end_ns: u64,
    midpoint_seconds: f64,
    band: String,
    low_hz: f64,
    high_hz: f64,
    power: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ExcludedWindow {
    bundle: String,
    condition: String,
    channel: String,
    window_index: usize,
    window_start_ns: u64,
    window_end_ns: u64,
    reason: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ConditionResult {
    condition: String,
    channel: String,
    band: String,
    valid_window_count: usize,
    excluded_window_count: usize,
    median_power: Option<f64>,
    iqr_power: Option<f64>,
    baseline_median_power: Option<f64>,
    relative_change_percent: Option<f64>,
    change_db: Option<f64>,
    change_direction: Option<String>,
}
#[derive(Debug, Serialize)]
struct Summary {
    title: &'static str,
    plugin_id: &'static str,
    plugin_version: &'static str,
    input_bundle_versions: BTreeMap<String, String>,
    synchronization_status: BTreeMap<String, String>,
    sample_rate_hz: f64,
    sample_rates_hz: BTreeMap<String, f64>,
    sample_rate_sources: BTreeMap<String, String>,
    quality_assessment: BTreeMap<String, String>,
    units: Option<String>,
    channels: Vec<String>,
    window_seconds: f64,
    hop_seconds: f64,
    window_samples: usize,
    hop_samples: usize,
    window_samples_by_input: BTreeMap<String, usize>,
    hop_samples_by_input: BTreeMap<String, usize>,
    window_function: &'static str,
    psd_method: &'static str,
    bands_hz: BTreeMap<String, [f64; 2]>,
    valid_windows: BTreeMap<String, usize>,
    excluded_windows: BTreeMap<String, usize>,
    exclusion_reasons: BTreeMap<String, usize>,
    warnings: Vec<String>,
    limitations: Vec<String>,
}

pub fn parse_args() -> Result<Arguments, String> {
    if env::var("LEMON_PLUGIN_API").as_deref() != Ok("1") {
        return Err("LEMON_PLUGIN_API=1 is required".into());
    }
    let mut input = None;
    let mut config = None;
    let mut output = None;
    let mut args = env::args_os().skip(1);
    while let Some(arg) = args.next() {
        let target = match arg.to_str() {
            Some("--input") => &mut input,
            Some("--config") => &mut config,
            Some("--output") => &mut output,
            _ => return Err(format!("unexpected argument {arg:?}")),
        };
        *target = Some(PathBuf::from(args.next().ok_or("missing argument value")?));
    }
    Ok(Arguments {
        input: input.ok_or("--input is required")?,
        config: config.ok_or("--config is required")?,
        output: output.ok_or("--output is required")?,
    })
}

pub fn run(args: Arguments) -> Result<(), String> {
    let config_path = fs::canonicalize(&args.config)
        .map_err(|e| format!("config {}: {e}", args.config.display()))?;
    let config: Config =
        serde_json::from_reader(File::open(&config_path).map_err(|e| e.to_string())?)
            .map_err(|e| format!("config JSON: {e}"))?;
    validate_config(&config)?;
    fs::create_dir_all(&args.output).map_err(|e| format!("output: {e}"))?;
    let baseline = Bundle::open(&args.input).map_err(|e| e.to_string())?;
    let config_dir = config_path
        .parent()
        .ok_or("config has no parent directory")?;
    let mut inputs = vec![(config.baseline.label.clone(), baseline)];
    for condition in &config.conditions {
        let path = if condition.analysis_bundle.is_absolute() {
            condition.analysis_bundle.clone()
        } else {
            config_dir.join(&condition.analysis_bundle)
        };
        inputs.push((
            condition.label.clone(),
            Bundle::open(path).map_err(|e| format!("condition {:?}: {e}", condition.label))?,
        ));
    }
    let mut warnings = validate_bundles(&config, &inputs)?;
    let fs_hz = inputs[0].1.sample_rate_hz();
    let window_samples = (config.window_seconds * fs_hz).round() as usize;
    let hop_samples = (config.hop_seconds * fs_hz).round() as usize;
    if window_samples < 4 || hop_samples == 0 {
        return Err("window_seconds/hop_seconds produce an unusable sample count".into());
    }
    let mut powers = Vec::new();
    let mut excluded = Vec::new();
    let mut versions = BTreeMap::new();
    let mut synchronization = BTreeMap::new();
    let mut sample_rates = BTreeMap::new();
    let mut sample_rate_sources = BTreeMap::new();
    let mut quality_assessment = BTreeMap::new();
    let mut window_sizes = BTreeMap::new();
    let mut hop_sizes = BTreeMap::new();
    for (label, bundle) in &inputs {
        let bundle_rate = bundle.sample_rate_hz();
        let bundle_window_samples = (config.window_seconds * bundle_rate).round() as usize;
        let bundle_hop_samples = (config.hop_seconds * bundle_rate).round() as usize;
        versions.insert(
            label.clone(),
            format!(
                "format {} / LEMON {}",
                bundle.manifest().analysis_format_version,
                bundle.manifest().lemon_version
            ),
        );
        synchronization.insert(
            label.clone(),
            if bundle
                .manifest()
                .time_normalization
                .contains("synchronized_clock=false")
            {
                "synchronized_clock=false".into()
            } else {
                "not explicitly established by AnalysisBundle metadata".into()
            },
        );
        sample_rates.insert(label.clone(), bundle_rate);
        sample_rate_sources.insert(label.clone(), bundle.manifest().sample_rate_source.clone());
        window_sizes.insert(label.clone(), bundle_window_samples);
        hop_sizes.insert(label.clone(), bundle_hop_samples);
        let sample_quality_reliable = !bundle
            .manifest()
            .analysis_warnings
            .iter()
            .any(|warning| warning.contains("one-sample blocks"));
        if sample_quality_reliable {
            quality_assessment.insert(
                label.clone(),
                "sample quality labels and timestamped quality events applied".into(),
            );
        } else {
            quality_assessment.insert(
                label.clone(),
                "sample quality labels not used: AnalysisBundle reports one-sample-block quality; explicit flags, discontinuities and timestamped quality events remain applied".into(),
            );
            warnings.push(format!(
                "Condition {label:?} reports one-sample-block quality assessment; sample labels were not used to exclude windows. Explicit flags, discontinuities and timestamped quality events remain applied."
            ));
        }
        let (samples, events) = load_bundle(bundle, &config.channels)?;
        let bundle_name = bundle
            .directory()
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("analysis-bundle");
        for channel in &config.channels {
            analyze_channel(
                label,
                bundle_name,
                channel,
                samples.get(channel).map(Vec::as_slice).unwrap_or(&[]),
                &events,
                &config,
                bundle_window_samples,
                bundle_hop_samples,
                bundle_rate,
                sample_quality_reliable,
                &mut powers,
                &mut excluded,
            );
        }
    }
    let results = aggregate(&config, &powers, &excluded, &mut warnings);
    write_powers(args.output.join("windowed-band-power.csv"), &powers)?;
    write_excluded(args.output.join("excluded-windows.csv"), &excluded)?;
    write_results(args.output.join("channel-condition-results.csv"), &results)?;
    let mut valid_counts = BTreeMap::new();
    for row in &powers {
        *valid_counts
            .entry(format!("{} / {}", row.condition, row.channel))
            .or_insert(0) += usize::from(row.band == "Mu");
    }
    let mut excluded_counts = BTreeMap::new();
    let mut reasons = BTreeMap::new();
    for row in &excluded {
        *excluded_counts
            .entry(format!("{} / {}", row.condition, row.channel))
            .or_insert(0) += 1;
        *reasons.entry(row.reason.clone()).or_insert(0) += 1;
    }
    for (label, _) in &inputs {
        for channel in &config.channels {
            if !valid_counts.contains_key(&format!("{label} / {channel}")) {
                warnings.push(format!(
                    "No valid complete windows for condition {label:?}, channel {channel:?}."
                ));
            }
        }
    }
    warnings.sort();
    warnings.dedup();
    let summary = Summary {
        title: "Mu-band condition comparison",
        plugin_id: PLUGIN_ID,
        plugin_version: PLUGIN_VERSION,
        input_bundle_versions: versions,
        synchronization_status: synchronization,
        sample_rate_hz: fs_hz,
        sample_rates_hz: sample_rates,
        sample_rate_sources,
        quality_assessment,
        units: inputs[0].1.units().map(str::to_owned),
        channels: config.channels.clone(),
        window_seconds: config.window_seconds,
        hop_seconds: config.hop_seconds,
        window_samples,
        hop_samples,
        window_samples_by_input: window_sizes,
        hop_samples_by_input: hop_sizes,
        window_function: "Hann",
        psd_method:
            "one-sided periodogram; sum of PSD bins multiplied by frequency-bin width",
        bands_hz: BTreeMap::from([
            ("Mu".into(), config.bands.mu_hz),
            ("Beta".into(), config.bands.beta_hz),
        ]),
        valid_windows: valid_counts,
        excluded_windows: excluded_counts,
        exclusion_reasons: reasons,
        warnings: warnings.clone(),
        limitations: vec![
            "Separate recordings without baseline_start, cue, task_start and task_end markers are compared descriptively; this is not event-locked MuERD, ERD or ERS.".into(),
            "Channel labels are retained as recorded. A0/A2 are not assigned to C3/C4 or hemispheres.".into(),
            "When synchronized_clock=false, phase, coherence and asymmetry interpretation is unavailable.".into(),
            "ChannelMismatch alone does not exclude a per-channel power window; no cross-channel metric is calculated.".into(),
            "No p-values or claims of statistical significance are produced.".into(),
        ],
    };
    serde_json::to_writer_pretty(
        File::create(args.output.join("summary.json")).map_err(|e| e.to_string())?,
        &summary,
    )
    .map_err(|e| e.to_string())?;
    let figures = plot::create_figures(
        &args.output,
        &config.channels,
        &results,
        &powers,
        &config.bands,
        inputs[0].1.units().unwrap_or("signal units"),
    )?;
    let mut artifacts = vec![
        artifact(
            "summary.json",
            ArtifactKind::Data,
            "Method, inputs, window counts, warnings and limitations",
        ),
        artifact(
            "channel-condition-results.csv",
            ArtifactKind::Table,
            "Descriptive condition comparison by channel and band",
        ),
        artifact(
            "windowed-band-power.csv",
            ArtifactKind::Table,
            "Power for every valid analysis window",
        ),
        artifact(
            "excluded-windows.csv",
            ArtifactKind::Table,
            "Excluded windows and their reasons",
        ),
    ];
    for figure in figures {
        artifacts.push(artifact(
            &figure,
            ArtifactKind::Figure,
            "Static descriptive plot",
        ));
    }
    let result = PluginResult {
        schema_version: 1,
        plugin: lemon_plugin_sdk::PluginIdentity {
            id: PLUGIN_ID.into(),
            version: PLUGIN_VERSION.into(),
        },
        status: if warnings.is_empty() {
            PluginStatus::Completed
        } else {
            PluginStatus::CompletedWithWarnings
        },
        artifacts,
        warnings,
        error: None,
    };
    write_plugin_result(&args.output, &result).map_err(|e| e.to_string())?;
    Ok(())
}

fn artifact(path: &str, kind: ArtifactKind, description: &str) -> Artifact {
    Artifact {
        path: path.into(),
        kind,
        description: description.into(),
    }
}
fn validate_config(c: &Config) -> Result<(), String> {
    if c.mode != "condition-comparison" {
        return Err("mode must be condition-comparison".into());
    }
    if c.channels.is_empty() || c.channels.len() > 32 {
        return Err("channels must contain 1..32 entries".into());
    }
    if c.conditions.is_empty() {
        return Err("conditions must contain at least one comparison bundle".into());
    }
    let mut unique = BTreeSet::new();
    if c.channels
        .iter()
        .any(|v| v.trim().is_empty() || !unique.insert(v))
    {
        return Err("channels must be non-empty and unique".into());
    }
    if !c.window_seconds.is_finite()
        || c.window_seconds <= 0.
        || !c.hop_seconds.is_finite()
        || c.hop_seconds <= 0.
    {
        return Err("window_seconds and hop_seconds must be positive finite values".into());
    }
    for (name, b) in [("mu_hz", c.bands.mu_hz), ("beta_hz", c.bands.beta_hz)] {
        if !b[0].is_finite() || !b[1].is_finite() || b[0] < 0. || b[0] >= b[1] {
            return Err(format!("invalid {name} range"));
        }
    }
    if c.bands.mu_hz[0] < c.bands.beta_hz[1] && c.bands.beta_hz[0] < c.bands.mu_hz[1] {
        return Err("Mu and Beta ranges must not overlap".into());
    }
    let mut labels = BTreeSet::new();
    for label in std::iter::once(&c.baseline.label).chain(c.conditions.iter().map(|v| &v.label)) {
        if label.trim().is_empty()
            || label.len() > 128
            || label.chars().any(char::is_control)
            || !labels.insert(label)
        {
            return Err(
                "condition labels must be unique, non-empty, printable and at most 128 bytes"
                    .into(),
            );
        }
    }
    Ok(())
}
fn validate_bundles(config: &Config, inputs: &[(String, Bundle)]) -> Result<Vec<String>, String> {
    let baseline = &inputs[0].1;
    let mut warnings = Vec::new();
    for (label, bundle) in inputs {
        for channel in &config.channels {
            if !bundle.channels().contains(channel) {
                return Err(format!(
                    "condition {label:?} does not contain channel {channel:?}"
                ));
            }
        }
        let rate_delta = (bundle.sample_rate_hz() - baseline.sample_rate_hz()).abs();
        let relative_rate_delta = rate_delta / baseline.sample_rate_hz();
        if relative_rate_delta > 0.005 {
            return Err(format!("condition {label:?} has incompatible sample rate"));
        }
        if rate_delta > 1e-9 {
            warnings.push(format!(
                "Condition {label:?} sample rate ({:.9} Hz) differs slightly from background ({:.9} Hz); each bundle is windowed and transformed using its own recorded rate.",
                bundle.sample_rate_hz(),
                baseline.sample_rate_hz()
            ));
        }
        if config.bands.mu_hz[1] > bundle.sample_rate_hz() / 2.
            || config.bands.beta_hz[1] > bundle.sample_rate_hz() / 2.
        {
            return Err(format!(
                "configured band exceeds Nyquist frequency for condition {label:?}"
            ));
        }
        if bundle.units() != baseline.units() {
            return Err(format!("condition {label:?} has incompatible units"));
        }
        if bundle.manifest().processing_config != baseline.manifest().processing_config {
            return Err(format!(
                "condition {label:?} has incompatible processing_config"
            ));
        }
    }
    Ok(warnings)
}
fn load_bundle(
    bundle: &Bundle,
    channels: &[String],
) -> Result<(SamplesByChannel, Vec<Event>), String> {
    let selected: BTreeSet<_> = channels.iter().cloned().collect();
    let mut samples: BTreeMap<String, Vec<Sample>> =
        channels.iter().map(|c| (c.clone(), Vec::new())).collect();
    for row in bundle.processed().map_err(|e| e.to_string())? {
        let row = row.map_err(|e| e.to_string())?;
        if selected.contains(&row.channel) {
            samples.entry(row.channel).or_default().push(Sample {
                timestamp_ns: row.timestamp_ns,
                sequence: row.sequence,
                value: row.filtered,
                quality: row.quality,
                flags: row.flags,
            });
        }
    }
    for rows in samples.values_mut() {
        rows.sort_by_key(|s| (s.timestamp_ns, s.sequence));
    }
    let mut events = Vec::new();
    for row in bundle.quality_events().map_err(|e| e.to_string())? {
        let row = row.map_err(|e| e.to_string())?;
        if severe_event(&row.kind, &row.severity) {
            events.push(Event {
                timestamp_ns: row.timestamp_ns,
                channel: row.channel.filter(|c| !c.is_empty()),
                reason: format!("quality event: {}", row.kind),
            });
        }
    }
    Ok((samples, events))
}
fn severe_event(kind: &str, severity: &str) -> bool {
    let k = kind.to_ascii_lowercase();
    severity.eq_ignore_ascii_case("error")
        || [
            "lost",
            "invalid",
            "disconnect",
            "outlier",
            "saturation",
            "lowquality",
            "insufficientdata",
            "gap",
        ]
        .iter()
        .any(|v| k.contains(v))
}

#[allow(clippy::too_many_arguments)]
fn analyze_channel(
    label: &str,
    bundle_name: &str,
    channel: &str,
    samples: &[Sample],
    events: &[Event],
    config: &Config,
    window: usize,
    hop: usize,
    fs_hz: f64,
    sample_quality_reliable: bool,
    powers: &mut Vec<WindowPower>,
    excluded: &mut Vec<ExcludedWindow>,
) {
    if samples.is_empty() {
        return;
    }
    let expected_ns = 1e9 / fs_hz;
    for (index, start) in (0..samples.len()).step_by(hop).enumerate() {
        if start + window > samples.len() {
            break;
        }
        let slice = &samples[start..start + window];
        let from = slice[0].timestamp_ns;
        let to = slice[window - 1].timestamp_ns;
        let mut reasons = BTreeSet::new();
        if slice.iter().any(|s| s.value.is_none()) {
            reasons.insert("missing filtered sample".to_string());
        }
        if sample_quality_reliable
            && slice
                .iter()
                .any(|s| !matches!(s.quality.as_str(), "Good" | "Acceptable" | "Fair"))
        {
            reasons.insert("quality label below acceptable or unknown".to_string());
        }
        let relevant_flags: BTreeSet<_> = slice
            .iter()
            .flat_map(|sample| sample.flags.split('|'))
            .filter(|flag| {
                matches!(
                    *flag,
                    "LostSample"
                        | "InvalidPacket"
                        | "Disconnected"
                        | "Outlier"
                        | "Saturation"
                        | "LowQuality"
                        | "InsufficientData"
                )
            })
            .collect();
        if !relevant_flags.is_empty() {
            reasons.insert(format!(
                "signal flags: {}",
                relevant_flags.into_iter().collect::<Vec<_>>().join("|")
            ));
        }
        if slice
            .windows(2)
            .any(|p| p[1].sequence != p[0].sequence.saturating_add(1))
        {
            reasons.insert("sequence discontinuity".to_string());
        }
        if slice.windows(2).any(|p| {
            ((p[1].timestamp_ns.saturating_sub(p[0].timestamp_ns) as f64) - expected_ns).abs()
                > expected_ns * 0.51
        }) {
            reasons.insert("timestamp discontinuity".to_string());
        }
        for event in events.iter().filter(|e| {
            e.timestamp_ns >= from
                && e.timestamp_ns <= to
                && e.channel.as_deref().is_none_or(|c| c == channel)
        }) {
            reasons.insert(event.reason.clone());
        }
        if reasons.is_empty() {
            let values: Vec<f64> = slice.iter().filter_map(|s| s.value).collect();
            let bins = psd(&values, fs_hz);
            for (band, range) in [("Mu", config.bands.mu_hz), ("Beta", config.bands.beta_hz)] {
                powers.push(WindowPower {
                    bundle: bundle_name.to_string(),
                    condition: label.into(),
                    channel: channel.into(),
                    window_index: index,
                    window_start_ns: from,
                    window_end_ns: to,
                    midpoint_seconds: (from as f64 + to as f64) / 2e9,
                    band: band.into(),
                    low_hz: range[0],
                    high_hz: range[1],
                    power: band_power(&bins, fs_hz, values.len(), range),
                });
            }
        } else {
            excluded.push(ExcludedWindow {
                bundle: bundle_name.to_string(),
                condition: label.into(),
                channel: channel.into(),
                window_index: index,
                window_start_ns: from,
                window_end_ns: to,
                reason: reasons.into_iter().collect::<Vec<_>>().join("; "),
            });
        }
    }
}
fn psd(values: &[f64], fs_hz: f64) -> Vec<f64> {
    let n = values.len();
    let hann: Vec<f64> = (0..n)
        .map(|i| 0.5 - 0.5 * (2. * std::f64::consts::PI * i as f64 / n as f64).cos())
        .collect();
    let mean = values.iter().sum::<f64>() / n as f64;
    let mut buffer: Vec<Complex<f64>> = values
        .iter()
        .zip(&hann)
        .map(|(v, w)| Complex::new((v - mean) * w, 0.))
        .collect();
    FftPlanner::new().plan_fft_forward(n).process(&mut buffer);
    let scale = fs_hz * hann.iter().map(|w| w * w).sum::<f64>();
    buffer[..=n / 2]
        .iter()
        .enumerate()
        .map(|(i, v)| v.norm_sqr() / scale * if i > 0 && i < n / 2 { 2. } else { 1. })
        .collect()
}
fn band_power(psd: &[f64], fs_hz: f64, n: usize, range: [f64; 2]) -> f64 {
    let df = fs_hz / n as f64;
    psd.iter()
        .enumerate()
        .filter(|(i, _)| {
            let f = *i as f64 * df;
            f >= range[0] && f < range[1]
        })
        .map(|(_, v)| v * df)
        .sum()
}
fn aggregate(
    config: &Config,
    powers: &[WindowPower],
    excluded: &[ExcludedWindow],
    warnings: &mut Vec<String>,
) -> Vec<ConditionResult> {
    let mut grouped: BTreeMap<(String, String, String), Vec<f64>> = BTreeMap::new();
    for p in powers {
        grouped
            .entry((p.condition.clone(), p.channel.clone(), p.band.clone()))
            .or_default()
            .push(p.power);
    }
    let mut excluded_counts: BTreeMap<(String, String), usize> = BTreeMap::new();
    for e in excluded {
        *excluded_counts
            .entry((e.condition.clone(), e.channel.clone()))
            .or_default() += 1;
    }
    let mut out = Vec::new();
    for condition in
        std::iter::once(&config.baseline.label).chain(config.conditions.iter().map(|c| &c.label))
    {
        for channel in &config.channels {
            for band in ["Mu", "Beta"] {
                let key = (condition.clone(), channel.clone(), band.into());
                let values = grouped.get(&key).map(Vec::as_slice).unwrap_or(&[]);
                let median = quantile(values, 0.5);
                let iqr = quantile(values, 0.75)
                    .zip(quantile(values, 0.25))
                    .map(|(a, b)| a - b);
                let base = grouped
                    .get(&(config.baseline.label.clone(), channel.clone(), band.into()))
                    .and_then(|v| quantile(v, 0.5));
                let ratio = if condition == &config.baseline.label {
                    None
                } else {
                    median.zip(base).and_then(|(m, b)| {
                        if b > f64::EPSILON && m >= 0. {
                            Some(m / b)
                        } else {
                            None
                        }
                    })
                };
                if condition != &config.baseline.label && ratio.is_none() {
                    warnings.push(format!("Cannot calculate change for condition {condition:?}, channel {channel:?}, band {band}: baseline or condition median is unavailable or baseline is near zero."));
                }
                out.push(ConditionResult {
                    condition: condition.clone(),
                    channel: channel.clone(),
                    band: band.into(),
                    valid_window_count: values.len(),
                    excluded_window_count: *excluded_counts
                        .get(&(condition.clone(), channel.clone()))
                        .unwrap_or(&0),
                    median_power: median,
                    iqr_power: iqr,
                    baseline_median_power: base,
                    relative_change_percent: ratio.map(|r| 100. * (r - 1.)),
                    change_db: ratio.filter(|r| *r > 0.).map(|r| 10. * r.log10()),
                    change_direction: ratio.map(|r| {
                        if r < 1.0 {
                            "decrease relative to background"
                        } else if r > 1.0 {
                            "increase relative to background"
                        } else {
                            "no change relative to background"
                        }
                        .into()
                    }),
                });
            }
        }
    }
    out
}
fn quantile(values: &[f64], q: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let pos = q * (sorted.len() - 1) as f64;
    let lo = pos.floor() as usize;
    let hi = pos.ceil() as usize;
    Some(sorted[lo] + (sorted[hi] - sorted[lo]) * (pos - lo as f64))
}
fn writer(path: PathBuf, headers: &[&str]) -> Result<csv::Writer<File>, String> {
    let mut writer = csv::WriterBuilder::new()
        .has_headers(false)
        .from_path(&path)
        .map_err(|e| e.to_string())?;
    writer.write_record(headers).map_err(|e| e.to_string())?;
    Ok(writer)
}
fn write_powers(path: PathBuf, rows: &[WindowPower]) -> Result<(), String> {
    let mut writer = writer(
        path,
        &[
            "bundle",
            "condition",
            "channel",
            "window_index",
            "window_start_ns",
            "window_end_ns",
            "midpoint_seconds",
            "band",
            "low_hz",
            "high_hz",
            "power",
        ],
    )?;
    for row in rows {
        writer.serialize(row).map_err(|e| e.to_string())?;
    }
    writer.flush().map_err(|e| e.to_string())
}
fn write_excluded(path: PathBuf, rows: &[ExcludedWindow]) -> Result<(), String> {
    let mut writer = writer(
        path,
        &[
            "bundle",
            "condition",
            "channel",
            "window_index",
            "window_start_ns",
            "window_end_ns",
            "reason",
        ],
    )?;
    for row in rows {
        writer.serialize(row).map_err(|e| e.to_string())?;
    }
    writer.flush().map_err(|e| e.to_string())
}
fn write_results(path: PathBuf, rows: &[ConditionResult]) -> Result<(), String> {
    let mut writer = writer(
        path,
        &[
            "condition",
            "channel",
            "band",
            "valid_window_count",
            "excluded_window_count",
            "median_power",
            "iqr_power",
            "baseline_median_power",
            "relative_change_percent",
            "change_db",
            "change_direction",
        ],
    )?;
    for row in rows {
        writer.serialize(row).map_err(|e| e.to_string())?;
    }
    writer.flush().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        path::Path,
        sync::atomic::{AtomicUsize, Ordering},
    };
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    fn temp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "lemon-mu-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&p).expect("temp");
        p
    }
    fn bundle(
        root: &Path,
        name: &str,
        rate: f64,
        seconds: f64,
        mu: f64,
        beta: f64,
        gap: bool,
    ) -> PathBuf {
        let dir = root.join(name);
        fs::create_dir(&dir).expect("bundle");
        let channels = ["A0", "A2"];
        fs::write(dir.join("manifest.json"),serde_json::to_vec(&serde_json::json!({"analysis_format_version":1,"lemon_version":"0.2.0","created_at":"0","source":name,"source_format":"synthetic","source_path":name,"channels":channels,"units":"adc_counts_u8","sample_rate_hz":rate,"sample_rate_source":"confirmed","duration_seconds":seconds,"processing_config":{"filter":"same"},"spectral_config":{},"configured_bands":[]})).expect("json")).expect("manifest");
        fs::write(dir.join("summary.json"),serde_json::to_vec(&serde_json::json!({"duration_seconds":seconds,"channel_count":2,"sample_count_by_channel":{"A0":0,"A2":0},"valid_filtered_count_by_channel":{"A0":0,"A2":0},"missing_count_by_channel":{"A0":0,"A2":0},"lost_sample_count":0,"invalid_packet_count":0,"outlier_count":0,"saturation_count":0,"disconnection_count":0,"continuous_segment_count":1,"sample_rate_hz":rate,"sample_rate_source":"confirmed","channels":{},"warnings":[],"dominant_band_by_channel":{}})).expect("json")).expect("summary");
        fs::write(dir.join("summary.md"), "fixture").expect("md");
        let mut processed = csv::Writer::from_path(dir.join("processed.csv")).expect("processed");
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
            .expect("header");
        let count = (rate * seconds) as usize;
        for n in 0..count {
            let t = n as f64 / rate;
            for (ci, channel) in channels.iter().enumerate() {
                let value = 128.
                    + mu * (2. * std::f64::consts::PI * 10. * t).sin()
                    + beta * (2. * std::f64::consts::PI * 20. * t).sin()
                    + ci as f64 * 0.01;
                processed
                    .write_record([
                        ((n as f64 * 1e9 / rate).round() as u64).to_string(),
                        n.to_string(),
                        channel.to_string(),
                        value.to_string(),
                        if gap && n == count / 2 {
                            String::new()
                        } else {
                            value.to_string()
                        },
                        "Good".into(),
                        String::new(),
                    ])
                    .expect("row");
            }
        }
        processed.flush().expect("flush");
        for (file,header) in [("spectrum.csv","window_index,window_start_ns,window_end_ns,channel,frequency_hz,raw_psd,filtered_psd\n"),("band-power.csv","window_index,window_start_ns,window_end_ns,channel,band,low_hz,high_hz,absolute,relative,smoothed,baseline_change_pct\n"),("quality-events.csv","timestamp_ns,channel,kind,severity,details\n")]{fs::write(dir.join(file),header).expect("file");}
        dir
    }
    fn config(root: &Path, condition: &Path, channels: &[&str]) -> PathBuf {
        let path = root.join("config.json");
        let relative = condition.file_name().expect("name").to_string_lossy();
        fs::write(&path,serde_json::to_vec_pretty(&serde_json::json!({"mode":"condition-comparison","channels":channels,"bands":{"mu_hz":[8.0,13.0],"beta_hz":[13.0,30.0]},"window_seconds":2.0,"hop_seconds":0.5,"baseline":{"label":"background"},"conditions":[{"label":"condition","analysis_bundle":relative}]})).expect("json")).expect("config");
        path
    }
    fn rows(path: &Path) -> Vec<ConditionResult> {
        csv::Reader::from_path(path)
            .expect("csv")
            .deserialize()
            .map(|r| r.expect("row"))
            .collect()
    }
    #[test]
    fn reduced_mu_is_negative_and_beta_is_measured() {
        let root = temp("decrease");
        let baseline = bundle(&root, "baseline", 100., 8., 20., 2., false);
        let condition = bundle(&root, "condition", 100., 8., 5., 4., false);
        let config = config(&root, &condition, &["A0", "A2"]);
        let before = fs::read(baseline.join("processed.csv")).expect("before");
        let output = root.join("output");
        run(Arguments {
            input: baseline.clone(),
            config,
            output: output.clone(),
        })
        .expect("run");
        let result = rows(&output.join("channel-condition-results.csv"));
        let mu = result
            .iter()
            .find(|r| r.condition == "condition" && r.channel == "A0" && r.band == "Mu")
            .expect("mu");
        assert!(mu.relative_change_percent.is_some_and(|v| v < -80.));
        assert!(mu.change_db.is_some_and(|v| v < -10.));
        assert_eq!(
            mu.change_direction.as_deref(),
            Some("decrease relative to background")
        );
        let beta = result
            .iter()
            .find(|r| r.condition == "condition" && r.channel == "A0" && r.band == "Beta")
            .expect("beta");
        assert!(beta.relative_change_percent.is_some_and(|v| v > 200.));
        assert_eq!(
            fs::read(baseline.join("processed.csv")).expect("after"),
            before
        );
        let plugin: PluginResult =
            serde_json::from_reader(File::open(output.join("plugin-result.json")).expect("result"))
                .expect("result JSON");
        assert!(matches!(plugin.status, PluginStatus::Completed));
        assert_eq!(plugin.artifacts.len(), 8);
        for artifact in plugin.artifacts {
            assert!(output.join(artifact.path).is_file());
        }
        let windowed =
            fs::read_to_string(output.join("windowed-band-power.csv")).expect("window powers");
        assert!(!windowed.contains(root.to_string_lossy().as_ref()));
        let _ = fs::remove_dir_all(root);
    }
    #[test]
    fn gaps_exclude_windows_with_reasons() {
        let root = temp("gap");
        let baseline = bundle(&root, "baseline", 100., 8., 20., 2., false);
        let condition = bundle(&root, "condition", 100., 8., 10., 2., true);
        let processed_path = condition.join("processed.csv");
        let processed = fs::read_to_string(&processed_path)
            .expect("processed")
            .replacen(",Good,", ",Poor,", 1);
        fs::write(&processed_path, processed).expect("processed");
        let output = root.join("output");
        run(Arguments {
            input: baseline,
            config: config(&root, &condition, &["A0"]),
            output: output.clone(),
        })
        .expect("run");
        let excluded: Vec<ExcludedWindow> =
            csv::Reader::from_path(output.join("excluded-windows.csv"))
                .expect("csv")
                .deserialize()
                .map(|r| r.expect("row"))
                .collect();
        assert!(excluded
            .iter()
            .any(|r| r.condition == "condition" && r.reason.contains("missing filtered")));
        assert!(excluded
            .iter()
            .any(|r| r.condition == "condition" && r.reason.contains("quality label")));
        let _ = fs::remove_dir_all(root);
    }
    #[test]
    fn unreliable_one_sample_quality_is_reported_but_not_treated_as_window_quality() {
        let root = temp("quality-policy");
        let baseline = bundle(&root, "baseline", 100., 4., 20., 2., false);
        let condition = bundle(&root, "condition", 100., 4., 10., 2., false);
        for directory in [&baseline, &condition] {
            let processed = fs::read_to_string(directory.join("processed.csv"))
                .expect("processed")
                .replace(",Good,", ",Poor,")
                .replace(",Poor,\n", ",Poor,ChannelMismatch\n");
            fs::write(directory.join("processed.csv"), processed).expect("processed");
            let manifest_path = directory.join("manifest.json");
            let mut manifest: serde_json::Value =
                serde_json::from_reader(File::open(&manifest_path).expect("manifest"))
                    .expect("manifest JSON");
            manifest["analysis_warnings"] =
                serde_json::json!(["Quality uses one-sample blocks independently per channel."]);
            fs::write(
                manifest_path,
                serde_json::to_vec(&manifest).expect("manifest JSON"),
            )
            .expect("manifest");
        }
        let output = root.join("output");
        run(Arguments {
            input: baseline,
            config: config(&root, &condition, &["A0"]),
            output: output.clone(),
        })
        .expect("run");
        let plugin: PluginResult =
            serde_json::from_reader(File::open(output.join("plugin-result.json")).expect("result"))
                .expect("result JSON");
        assert!(matches!(plugin.status, PluginStatus::CompletedWithWarnings));
        assert!(plugin
            .warnings
            .iter()
            .any(|warning| warning.contains("one-sample-block quality")));
        assert!(rows(&output.join("channel-condition-results.csv"))
            .iter()
            .any(|row| row.valid_window_count > 0));
        let _ = fs::remove_dir_all(root);
    }
    #[test]
    fn incompatible_bundle_and_unknown_channel_are_rejected() {
        let root = temp("invalid");
        let baseline = bundle(&root, "baseline", 100., 4., 20., 2., false);
        let condition = bundle(&root, "condition", 128., 4., 10., 2., false);
        let error = run(Arguments {
            input: baseline.clone(),
            config: config(&root, &condition, &["A0"]),
            output: root.join("rate-output"),
        })
        .expect_err("rate");
        assert!(error.contains("sample rate"));
        let compatible = bundle(&root, "compatible", 100., 4., 10., 2., false);
        let error = run(Arguments {
            input: baseline,
            config: config(&root, &compatible, &["UNKNOWN"]),
            output: root.join("channel-output"),
        })
        .expect_err("channel");
        assert!(error.contains("does not contain channel"));
        let _ = fs::remove_dir_all(root);
    }
    #[test]
    fn short_recording_completes_with_warning_and_no_empty_figures() {
        let root = temp("short");
        let baseline = bundle(&root, "baseline", 100., 1., 20., 2., false);
        let condition = bundle(&root, "condition", 100., 1., 10., 2., false);
        let output = root.join("output");
        run(Arguments {
            input: baseline,
            config: config(&root, &condition, &["A0"]),
            output: output.clone(),
        })
        .expect("run");
        let plugin: PluginResult =
            serde_json::from_reader(File::open(output.join("plugin-result.json")).expect("result"))
                .expect("json");
        assert!(matches!(plugin.status, PluginStatus::CompletedWithWarnings));
        assert!(plugin
            .warnings
            .iter()
            .any(|w| w.contains("No valid complete windows")));
        assert!(!output.join("figures/mu-band-comparison.png").exists());
        assert!(fs::read_to_string(output.join("excluded-windows.csv"))
            .expect("excluded")
            .starts_with("bundle,condition"));
        let _ = fs::remove_dir_all(root);
    }
}
