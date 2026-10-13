//! Descriptive Mu/Beta comparison between separate AnalysisBundle conditions.
mod plot;

use lemon_plugin_sdk::{
    write_plugin_result, Artifact, ArtifactKind, Bundle, PluginResult, PluginStatus,
};
use processing::windows::{
    flags_from_codes, AnalysisSample, LegacySegments, SignalFlag, WindowCandidate, WindowGenerator,
    WindowingConfig,
};
use processing::{integrate_band_power, one_sided_psd, WindowFunction};
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
    segment_id: u64,
    sample_valid: bool,
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
    dsp_algorithm_version: u32,
    psd_algorithm_version: u32,
    window_function: String,
    bundle: String,
    condition: String,
    channel: String,
    window_index: u64,
    segment_id: Option<u64>,
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
    dsp_algorithm_version: u32,
    bundle: String,
    condition: String,
    channel: String,
    window_index: u64,
    segment_id: Option<u64>,
    window_start_ns: u64,
    window_end_ns: u64,
    reason: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ConditionResult {
    dsp_algorithm_version: u32,
    psd_algorithm_version: u32,
    window_function: String,
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
    dsp_algorithm_versions: BTreeMap<String, u32>,
    psd_algorithm_versions: BTreeMap<String, u32>,
    computed_psd_algorithm_version: u32,
    windowing_algorithm_version: u32,
    input_windowing_algorithm_versions: BTreeMap<String, u32>,
    input_processing_configs: BTreeMap<String, serde_json::Value>,
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
    bundle_excluded_windows: BTreeMap<String, usize>,
    bundle_exclusion_reasons: BTreeMap<String, BTreeMap<String, usize>>,
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
    let mut dsp_versions = BTreeMap::new();
    let mut psd_versions = BTreeMap::new();
    let mut bundle_excluded_windows = BTreeMap::new();
    let mut bundle_exclusion_reasons = BTreeMap::new();
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
        dsp_versions.insert(label.clone(), bundle.manifest().dsp_algorithm_version);
        psd_versions.insert(label.clone(), bundle.manifest().psd_algorithm_version);
        let mut source_excluded = 0;
        let mut source_reasons = BTreeMap::new();
        if let Some(rows) = bundle.excluded_windows().map_err(|e| e.to_string())? {
            for row in rows {
                let row = row.map_err(|e| e.to_string())?;
                for reason in row
                    .reason_codes
                    .split('|')
                    .filter(|reason| !reason.is_empty())
                {
                    *source_reasons.entry(reason.to_owned()).or_insert(0) += 1;
                }
                source_excluded += 1;
            }
        }
        bundle_excluded_windows.insert(label.clone(), source_excluded);
        bundle_exclusion_reasons.insert(label.clone(), source_reasons);
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
        let sample_quality_reliable = bundle.manifest().analysis_format_version < 2
            && !bundle
                .manifest()
                .analysis_warnings
                .iter()
                .any(|warning| warning.contains("one-sample blocks"));
        if sample_quality_reliable {
            quality_assessment.insert(
                label.clone(),
                "sample quality labels and timestamped quality events applied".into(),
            );
        } else if bundle.manifest().analysis_format_version >= 2 {
            quality_assessment.insert(label.clone(), "v2 sample_valid, segment_id and artifact flags applied; display quality does not override eligibility".into());
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
                bundle.manifest().dsp_algorithm_version,
                processing::PSD_ALGORITHM_VERSION,
                sample_quality_reliable,
                bundle.manifest().analysis_format_version < 2,
                bundle.manifest().reject_low_quality_windows,
                &mut powers,
                &mut excluded,
            )?;
        }
    }
    let results = aggregate(
        &config,
        &powers,
        &excluded,
        inputs[0].1.manifest().dsp_algorithm_version,
        processing::PSD_ALGORITHM_VERSION,
        &mut warnings,
    );
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
        dsp_algorithm_versions: dsp_versions,
        psd_algorithm_versions: psd_versions,
        computed_psd_algorithm_version: processing::PSD_ALGORITHM_VERSION,
        windowing_algorithm_version: processing::windows::WINDOWING_ALGORITHM_VERSION,
        input_windowing_algorithm_versions: inputs.iter().map(|(label,bundle)| (label.clone(), bundle.manifest().windowing_algorithm_version)).collect(),
        input_processing_configs: inputs.iter().map(|(label, bundle)| (label.clone(), bundle.manifest().processing_config.clone())).collect(),
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
        window_function: "hann_symmetric",
        psd_method:
            "one-sided periodogram; sum of PSD bins multiplied by frequency-bin width",
        bands_hz: BTreeMap::from([
            ("Mu".into(), config.bands.mu_hz),
            ("Beta".into(), config.bands.beta_hz),
        ]),
        valid_windows: valid_counts,
        excluded_windows: excluded_counts,
        exclusion_reasons: reasons,
        bundle_excluded_windows,
        bundle_exclusion_reasons,
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
        if relative_rate_delta > 1e-9 {
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
        let required_processing_fields = [
            "high_pass_hz",
            "low_pass_hz",
            "notch",
            "notch_hz",
            "notch_q",
            "window_samples",
            "hop_samples",
            "window_function",
            "artifact_policy",
            "filter_warmup_seconds",
            "reject_low_quality_windows",
            "expected_abs_max",
            "outlier_step",
            "flat_stddev",
            "mains_ratio_limit",
        ];
        for field in required_processing_fields {
            if bundle.manifest().processing_config.get(field)
                != baseline.manifest().processing_config.get(field)
            {
                return Err(format!(
                    "condition {label:?} has incompatible processing_config.{field}"
                ));
            }
        }
        if bundle.manifest().dsp_algorithm_version != baseline.manifest().dsp_algorithm_version
            || bundle.manifest().windowing_algorithm_version
                != baseline.manifest().windowing_algorithm_version
            || bundle.manifest().psd_algorithm_version != baseline.manifest().psd_algorithm_version
            || bundle.manifest().processing_mode != baseline.manifest().processing_mode
            || bundle.manifest().window_function != baseline.manifest().window_function
            || bundle.manifest().spectral_config != baseline.manifest().spectral_config
            || bundle.manifest().configured_bands != baseline.manifest().configured_bands
        {
            return Err(format!(
                "condition {label:?} has incompatible DSP/PSD provenance"
            ));
        }
        if bundle.manifest().processing_config != baseline.manifest().processing_config {
            warnings.push(format!(
                "Condition {label:?} differs in secondary processing configuration fields; required DSP compatibility fields match."
            ));
        }
    }
    if baseline.manifest().windowing_algorithm_version == 0 {
        warnings.push("Input bundles predate the shared windowing contract; plugin candidates use windowing_algorithm_version=1.".into());
    }
    if baseline.manifest().dsp_algorithm_version < 2 {
        warnings.push(
            "Input bundles use legacy DSP algorithm version 1; artifact-free spectral eligibility cannot be established as strongly as for AnalysisBundle v2.".into(),
        );
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
                segment_id: row.segment_id,
                sample_valid: row.sample_valid && !row.filter_warmup && !row.artifact_rejected,
            });
        }
    }
    let mut events = Vec::new();
    for row in bundle.quality_events().map_err(|e| e.to_string())? {
        let row = row.map_err(|e| e.to_string())?;
        if severe_event(&row.kind, &row.severity)
            && (bundle.manifest().reject_low_quality_windows
                || !row.kind.to_ascii_lowercase().contains("lowquality"))
        {
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
    dsp_algorithm_version: u32,
    psd_algorithm_version: u32,
    sample_quality_reliable: bool,
    legacy: bool,
    reject_low_quality: bool,
    powers: &mut Vec<WindowPower>,
    excluded: &mut Vec<ExcludedWindow>,
) -> Result<(), String> {
    let mut window_config = WindowingConfig::new(window, hop, fs_hz);
    window_config.reject_low_quality = reject_low_quality;
    let mut segments = LegacySegments::new(window_config.clone())?;
    let mut generator = WindowGenerator::new(window_config)?;
    let mut consume = |candidate| -> Result<(), String> {
        match candidate {
            WindowCandidate::Rejected {
                index,
                segment_id,
                start_timestamp_ns,
                end_timestamp_ns,
                reasons,
                ..
            } => {
                excluded.push(ExcludedWindow {
                    dsp_algorithm_version,
                    bundle: bundle_name.into(),
                    condition: label.into(),
                    channel: channel.into(),
                    window_index: index,
                    segment_id,
                    window_start_ns: start_timestamp_ns,
                    window_end_ns: end_timestamp_ns,
                    reason: reasons
                        .iter()
                        .map(|reason| reason.code())
                        .collect::<Vec<_>>()
                        .join("|"),
                });
            }
            WindowCandidate::Accepted {
                index,
                segment_id,
                start_timestamp_ns,
                end_timestamp_ns,
                samples,
            } => {
                let values = samples
                    .iter()
                    .map(|sample| sample.value.ok_or("plugin: accepted value missing"))
                    .collect::<Result<Vec<_>, _>>()?;
                let psd = one_sided_psd(&values, fs_hz, WindowFunction::HannSymmetric)?;
                for (band, range) in [("Mu", config.bands.mu_hz), ("Beta", config.bands.beta_hz)] {
                    powers.push(WindowPower {
                        dsp_algorithm_version,
                        psd_algorithm_version,
                        window_function: "hann_symmetric".into(),
                        bundle: bundle_name.into(),
                        condition: label.into(),
                        channel: channel.into(),
                        window_index: index,
                        segment_id: Some(segment_id),
                        window_start_ns: start_timestamp_ns,
                        window_end_ns: end_timestamp_ns,
                        midpoint_seconds: (start_timestamp_ns as f64 + end_timestamp_ns as f64)
                            / 2e9,
                        band: band.into(),
                        low_hz: range[0],
                        high_hz: range[1],
                        power: integrate_band_power(&psd, range[0], range[1])?,
                    });
                }
            }
        }
        Ok(())
    };
    for sample in samples {
        let mut flags = flags_from_codes(&sample.flags);
        // v2 per-sample metadata is authoritative. Legacy event records supply
        // conservative additional flags, not a second window-validation pass.
        if legacy {
            if sample_quality_reliable
                && !matches!(sample.quality.as_str(), "Good" | "Acceptable" | "Fair")
            {
                flags.push(SignalFlag::LowQuality);
            }
            for event in events.iter().filter(|event| {
                event.timestamp_ns == sample.timestamp_ns
                    && event.channel.as_deref().is_none_or(|id| id == channel)
            }) {
                let code = event.reason.trim_start_matches("quality event: ");
                flags.extend(flags_from_codes(code));
            }
        }
        let segment_id = if legacy {
            segments.segment(sample.sequence, sample.timestamp_ns)
        } else {
            sample.segment_id
        };
        for candidate in generator.push(AnalysisSample {
            sequence: sample.sequence,
            timestamp_ns: sample.timestamp_ns,
            segment_id,
            value: sample.value,
            eligible: sample.sample_valid,
            flags,
        }) {
            consume(candidate)?;
        }
    }
    if let Some(candidate) = generator.finish() {
        consume(candidate)?;
    }
    Ok(())
}
fn aggregate(
    config: &Config,
    powers: &[WindowPower],
    excluded: &[ExcludedWindow],
    dsp_algorithm_version: u32,
    psd_algorithm_version: u32,
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
                    dsp_algorithm_version,
                    psd_algorithm_version,
                    window_function: "hann_symmetric".into(),
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
            "dsp_algorithm_version",
            "psd_algorithm_version",
            "window_function",
            "bundle",
            "condition",
            "channel",
            "window_index",
            "segment_id",
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
            "dsp_algorithm_version",
            "bundle",
            "condition",
            "channel",
            "window_index",
            "segment_id",
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
            "dsp_algorithm_version",
            "psd_algorithm_version",
            "window_function",
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
        fs::write(dir.join("manifest.json"),serde_json::to_vec(&serde_json::json!({"analysis_format_version":2,"dsp_algorithm_version":3,"windowing_algorithm_version":1,"psd_algorithm_version":2,"processing_mode":"causal_live_compatible","window_function":"hann_symmetric","filter_warmup_seconds":2.0,"reject_low_quality_windows":true,"lemon_version":"0.2.0","created_at":"0","source":name,"source_format":"synthetic","source_path":name,"channels":channels,"units":"adc_counts_u8","sample_rate_hz":rate,"sample_rate_source":"confirmed","duration_seconds":seconds,"processing_config":{"filter":"same"},"spectral_config":{"window_samples":200,"hop_samples":50,"window":"hann_symmetric"},"configured_bands":[]})).expect("json")).expect("manifest");
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
                "segment_id",
                "sample_valid",
                "filter_warmup",
                "artifact_rejected",
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
                        "0".into(),
                        (!gap || n != count / 2).to_string(),
                        "false".into(),
                        (gap && n == count / 2).to_string(),
                        "Good".into(),
                        String::new(),
                    ])
                    .expect("row");
            }
        }
        processed.flush().expect("flush");
        for (file,header) in [("spectrum.csv","window_index,window_start_ns,window_end_ns,channel,segment_id,frequency_hz,raw_psd,filtered_psd,window_function,sample_count,frequency_resolution_hz\n"),("band-power.csv","window_index,window_start_ns,window_end_ns,channel,segment_id,band,low_hz,high_hz,absolute,relative,smoothed,baseline_change_pct,quality_status\n"),("quality-events.csv","timestamp_ns,channel,kind,severity,details\n"),("excluded-windows.csv","channel,segment_id,window_index,window_start_ns,window_end_ns,reason_codes,details\n")]{fs::write(dir.join(file),header).expect("file");}
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
    fn analysis_and_plugin_share_candidate_grid_reasons_and_power() {
        use core_types::{
            ChannelSamples, Config as CoreConfig, RawSignalBlock, Sample as RawSample,
        };
        let root = temp("shared-grid");
        let mut c = CoreConfig::default();
        c.source.channels = 1;
        c.source.sample_rate_hz = 100.0;
        c.source.mains = 0.0;
        c.processing.low_pass_hz = 45.0;
        c.processing.notch = false;
        c.processing.window_samples = 200;
        c.processing.hop_samples = 50;
        c.processing.filter_warmup_seconds = 0.1;
        c.processing.outlier_step = 150.0;
        c.processing.reject_low_quality_windows = false;
        let mut paths = Vec::new();
        for chunk in [1, 37, 800] {
            let session = root.join(format!("native-{chunk}"));
            let mut writer =
                storage::SessionWriter::create_at(&session, &storage::Metadata::new(c.clone()))
                    .expect("writer");
            for (block_index, start) in (0..800).step_by(chunk).enumerate() {
                let samples = (start..(start + chunk).min(800))
                    .map(|index| {
                        let sequence = index as u64 + u64::from(index >= 515);
                        let value = if index == 237 {
                            250.0
                        } else {
                            f64::from(index >= 515) * 200.0
                                + (std::f64::consts::TAU * 10.0 * index as f64 / 100.0).sin()
                        };
                        RawSample {
                            sequence,
                            timestamp_ns: sequence * 10_000_000,
                            value,
                            flags: vec![],
                        }
                    })
                    .collect();
                writer
                    .write_block(&RawSignalBlock {
                        sequence: block_index as u64,
                        started_at: start as u64 * 10_000_000,
                        sample_rate_hz: 100.0,
                        source: "synthetic".into(),
                        synchronized_clock: true,
                        flags: vec![],
                        channels: vec![ChannelSamples {
                            id: "A0".into(),
                            device: "fixture".into(),
                            samples,
                        }],
                    })
                    .expect("block");
            }
            writer.flush().expect("flush");
            let analysis = analysis::analyze(
                analysis::AnalysisInput { path: session },
                analysis::AnalysisOptions {
                    output: Some(root.join(format!("analysis-{chunk}"))),
                    ..Default::default()
                },
            )
            .expect("analysis");
            paths.push(analysis.directory);
        }
        for name in [
            "processed.csv",
            "spectrum.csv",
            "band-power.csv",
            "excluded-windows.csv",
        ] {
            let expected = fs::read(paths[0].join(name)).expect("file");
            for directory in &paths[1..] {
                assert_eq!(
                    fs::read(directory.join(name)).expect("file"),
                    expected,
                    "{name}"
                );
            }
        }
        let output = root.join("plugin-output");
        run(Arguments {
            input: paths[0].clone(),
            config: config(&root, &paths[0], &["A0"]),
            output: output.clone(),
        })
        .expect("plugin");
        let bundle = Bundle::open(&paths[0]).expect("bundle");
        let expected_exclusions = bundle
            .excluded_windows()
            .expect("reader")
            .expect("v2")
            .map(|row| {
                let row = row.expect("row");
                (
                    row.window_index,
                    Some(row.segment_id),
                    row.window_start_ns,
                    row.window_end_ns,
                    row.reason_codes,
                )
            })
            .collect::<Vec<_>>();
        let actual_exclusions = csv::Reader::from_path(output.join("excluded-windows.csv"))
            .expect("csv")
            .deserialize::<ExcludedWindow>()
            .map(|row| row.expect("row"))
            .filter(|row| row.condition == "background")
            .map(|row| {
                (
                    row.window_index,
                    row.segment_id,
                    row.window_start_ns,
                    row.window_end_ns,
                    row.reason,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(actual_exclusions, expected_exclusions);
        let expected = bundle
            .band_power()
            .expect("bands")
            .map(|row| row.expect("row"))
            .filter(|row| row.low_hz == 8.0 && row.high_hz == 13.0)
            .collect::<Vec<_>>();
        assert!(!expected.is_empty());
        let actual = csv::Reader::from_path(output.join("windowed-band-power.csv"))
            .expect("csv")
            .deserialize::<WindowPower>()
            .map(|row| row.expect("row"))
            .filter(|row| row.condition == "background" && row.band == "Mu")
            .collect::<Vec<_>>();
        assert_eq!(actual.len(), expected.len());
        for (a, b) in actual.iter().zip(expected) {
            assert_eq!(
                (
                    a.window_index,
                    a.segment_id,
                    a.window_start_ns,
                    a.window_end_ns
                ),
                (
                    b.window_index,
                    Some(b.segment_id),
                    b.window_start_ns,
                    b.window_end_ns
                )
            );
            assert!((a.power - b.absolute).abs() < 1e-12);
        }
        assert!(
            actual
                .iter()
                .any(|row| row.window_start_ns == 248 * 10_000_000),
            "off-hop artifact restarts grid after warmup"
        );
        let _ = fs::remove_dir_all(root);
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
        let powers: Vec<WindowPower> =
            csv::Reader::from_path(output.join("windowed-band-power.csv"))
                .expect("window powers")
                .deserialize()
                .map(|row| row.expect("window row"))
                .collect();
        let first_mu = powers
            .iter()
            .find(|row| {
                row.condition == "background"
                    && row.channel == "A0"
                    && row.band == "Mu"
                    && row.window_index == 0
            })
            .expect("first Mu window");
        let samples = (0..200)
            .map(|index| {
                let time = index as f64 / 100.0;
                128.0
                    + 20.0 * (std::f64::consts::TAU * 10.0 * time).sin()
                    + 2.0 * (std::f64::consts::TAU * 20.0 * time).sin()
            })
            .collect::<Vec<_>>();
        let shared =
            one_sided_psd(&samples, 100.0, WindowFunction::HannSymmetric).expect("shared PSD");
        let expected = integrate_band_power(&shared, 8.0, 13.0).expect("shared Mu power");
        assert!((first_mu.power - expected).abs() < 1e-12);
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
            .replacen(
                ",true,false,false,Good,",
                ",false,false,false,Poor,LowQuality",
                1,
            );
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
            .any(|r| r.condition == "condition" && r.reason.contains("MissingFilteredSample")));
        assert!(excluded
            .iter()
            .any(|r| r.condition == "condition" && r.reason.contains("IneligibleSample")));
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
            manifest
                .as_object_mut()
                .expect("object")
                .remove("windowing_algorithm_version");
            manifest["analysis_format_version"] = serde_json::json!(1);
            manifest["dsp_algorithm_version"] = serde_json::json!(1);
            manifest["psd_algorithm_version"] = serde_json::json!(1);
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
            .starts_with("dsp_algorithm_version,bundle,condition"));
        let _ = fs::remove_dir_all(root);
    }
}
