//! Event-locked descriptive power analysis over a read-only AnalysisBundle.
mod compute;
mod config;
mod markers;
mod plot;
use compute::{aggregates, analyze_trial, Results};
pub use config::Config;
use lemon_plugin_sdk::{
    write_plugin_result, Artifact, ArtifactKind, Bundle, PluginResult, PluginStatus,
};
use markers::Sidecar;
use processing::windows::{flags_from_codes, AnalysisSample, SignalFlag};
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    fs::{self, File},
    io::{BufReader, Read},
    path::{Path, PathBuf},
};

pub const PLUGIN_ID: &str = "mu-erd";
pub const PLUGIN_VERSION: &str = env!("CARGO_PKG_VERSION");
#[derive(Debug)]
pub struct Arguments {
    pub input: PathBuf,
    pub config: PathBuf,
    pub output: PathBuf,
}
pub fn parse_args() -> Result<Arguments, String> {
    if env::var("LEMON_PLUGIN_API").as_deref() != Ok("1") {
        return Err("LEMON_PLUGIN_API=1 is required".into());
    }
    let (mut input, mut config, mut output) = (None, None, None);
    let mut args = env::args_os().skip(1);
    while let Some(arg) = args.next() {
        let target = match arg.to_str() {
            Some("--input") => &mut input,
            Some("--config") => &mut config,
            Some("--output") => &mut output,
            _ => return Err(format!("unexpected argument {arg:?}")),
        };
        if target.is_some() {
            return Err("duplicate argument".into());
        }
        *target = Some(PathBuf::from(args.next().ok_or("missing argument value")?));
    }
    Ok(Arguments {
        input: input.ok_or("--input required")?,
        config: config.ok_or("--config required")?,
        output: output.ok_or("--output required")?,
    })
}
fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let f = File::open(path).map_err(|e| e.to_string())?;
    if f.metadata().map_err(|e| e.to_string())?.len() > 4 * 1024 * 1024 {
        return Err("JSON exceeds 4 MiB".into());
    }
    serde_json::from_reader(f).map_err(|e| e.to_string())
}
fn hashes(b: &Bundle) -> Result<BTreeMap<String, String>, String> {
    let mut hashes = BTreeMap::new();
    for name in [
        "manifest.json",
        "summary.json",
        "summary.md",
        "processed.csv",
        "spectrum.csv",
        "band-power.csv",
        "quality-events.csv",
        "excluded-windows.csv",
        "experiment.json",
    ] {
        let mut reader = BufReader::new(
            File::open(b.directory().join(name)).map_err(|e| format!("{name}: {e}"))?,
        );
        let mut hash = Sha256::new();
        let mut buf = [0; 65536];
        loop {
            let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            hash.update(&buf[..n]);
        }
        hashes.insert(name.into(), format!("sha256:{:x}", hash.finalize()));
    }
    Ok(hashes)
}
fn samples(b: &Bundle, c: &Config) -> Result<BTreeMap<String, Vec<AnalysisSample>>, String> {
    let mut result: BTreeMap<String, Vec<AnalysisSample>> =
        c.channels.iter().map(|c| (c.clone(), Vec::new())).collect();
    let mut count = 0;
    for row in b.processed().map_err(|e| e.to_string())? {
        let r = row.map_err(|e| e.to_string())?;
        if let Some(channel) = result.get_mut(&r.channel) {
            count += 1;
            if count > 10_000_000 {
                return Err("selected data exceed 10 million samples; split recording".into());
            }
            if channel
                .last()
                .is_some_and(|last| r.timestamp_ns <= last.timestamp_ns)
            {
                return Err("processed.csv channel timestamps must be strictly increasing".into());
            }
            let mut flags = flags_from_codes(&r.flags);
            if r.filter_warmup {
                flags.push(SignalFlag::FilterWarmup);
            }
            // v2 display-quality can lag actual per-sample eligibility. Explicit flags
            // and sample_valid are authoritative, as in the shared analysis pipeline.
            channel.push(AnalysisSample {
                sequence: r.sequence,
                timestamp_ns: r.timestamp_ns,
                segment_id: r.segment_id,
                value: r.filtered,
                eligible: r.sample_valid && !r.artifact_rejected,
                flags,
            });
        }
    }
    // Timestamped quality flags apply to the corresponding sample (or the next
    // sample within one period), never to the entire recording.
    for event in b.quality_events().map_err(|e| e.to_string())? {
        let e = event.map_err(|e| e.to_string())?;
        let flags = flags_from_codes(&e.kind);
        if flags.is_empty() {
            continue;
        }
        for (channel, rows) in &mut result {
            if e.channel.as_ref().is_some_and(|c| c != channel) {
                continue;
            }
            let i = rows.partition_point(|s| s.timestamp_ns < e.timestamp_ns);
            if let Some(row) = rows.get_mut(i) {
                if (row.timestamp_ns - e.timestamp_ns) as f64 <= 1e9 / b.sample_rate_hz() {
                    row.flags.extend(&flags);
                }
            }
        }
    }
    Ok(result)
}
fn table<T: Serialize>(
    dir: &Path,
    name: &str,
    headers: &str,
    rows: &[T],
) -> Result<Artifact, String> {
    let mut writer = csv::WriterBuilder::new()
        .has_headers(false)
        .from_path(dir.join(name))
        .map_err(|e| e.to_string())?;
    writer
        .write_record(
            format!(
                "dsp_algorithm_version,psd_algorithm_version,windowing_algorithm_version,{headers}"
            )
            .split(','),
        )
        .map_err(|e| e.to_string())?;
    for row in rows {
        writer.serialize(row).map_err(|e| e.to_string())?;
    }
    writer.flush().map_err(|e| e.to_string())?;
    Ok(Artifact {
        path: name.into(),
        kind: ArtifactKind::Table,
        description: "Descriptive power data; versions in every row".into(),
    })
}
fn prepare_output(path: &Path, input: &Path) -> Result<PathBuf, String> {
    // Direct execution is safe too; the platform normally prepares this directory.
    if path.exists() {
        let resolved = fs::canonicalize(path).map_err(|e| e.to_string())?;
        if resolved.starts_with(input) || input.starts_with(&resolved) {
            return Err("output overlaps input bundle".into());
        }
        for entry in fs::read_dir(&resolved).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            // Platform opens these diagnostics before spawning the plugin.
            let allowed_log = matches!(
                entry.file_name().to_str(),
                Some(".lemon-plugin-stdout.log" | ".lemon-plugin-stderr.log")
            );
            if !allowed_log || !entry.file_type().map_err(|e| e.to_string())?.is_file() {
                return Err("output must be empty (except platform diagnostics)".into());
            }
        }
        return Ok(resolved);
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let parent = fs::canonicalize(parent).map_err(|e| format!("output parent must exist: {e}"))?;
    if parent.starts_with(input) {
        return Err("output cannot be inside input bundle".into());
    }
    let target = parent.join(path.file_name().ok_or("output directory name required")?);
    fs::create_dir(&target).map_err(|e| e.to_string())?;
    Ok(target)
}
pub fn run(a: Arguments) -> Result<(), String> {
    let config: Config = read_json(&a.config)?;
    let bundle = Bundle::open(&a.input).map_err(|e| e.to_string())?;
    let wc = config.validate(&bundle)?;
    // Size-limit before SDK parsing; SDK remains the only bundle reader.
    if fs::metadata(bundle.directory().join("experiment.json"))
        .map_err(|e| format!("experiment.json required: {e}"))?
        .len()
        > 4 * 1024 * 1024
    {
        return Err("experiment.json exceeds 4 MiB".into());
    }
    let sidecar: Sidecar = serde_json::from_value(
        bundle
            .experiment()
            .map_err(|e| e.to_string())?
            .ok_or("experiment.json required")?,
    )
    .map_err(|e| e.to_string())?;
    let trials = sidecar.trials(&config)?;
    let mut input_exclusions: BTreeMap<String, usize> = BTreeMap::new();
    for row in bundle
        .excluded_windows()
        .map_err(|e| e.to_string())?
        .ok_or("excluded-windows.csv required")?
    {
        let r = row.map_err(|e| e.to_string())?;
        for code in r.reason_codes.split('|').filter(|c| !c.is_empty()) {
            *input_exclusions.entry(code.into()).or_default() += 1;
        }
    }
    let input_hashes = hashes(&bundle)?;
    let hash = format!(
        "sha256:{:x}",
        Sha256::digest(serde_json::to_vec(&input_hashes).map_err(|e| e.to_string())?)
    );
    let data = samples(&bundle, &config)?;
    let mut results = Results::default();
    for trial in &trials {
        for channel in &config.channels {
            analyze_trial(
                &config,
                &wc,
                trial,
                channel,
                &data[channel],
                &hash,
                &mut results,
            )?;
        }
    }
    let curves = aggregates(&results, true);
    let comparisons = aggregates(&results, false);
    let mut warnings=vec!["Software markers do not guarantee sub-millisecond stimulus/EEG synchronization.".into(),"Per-sample v2 eligibility/flags and timestamped quality events are authoritative; display quality labels are not reinterpreted.".into()];
    if sidecar.marker_timestamp_basis == "last_observed_sample" {
        warnings.push("Markers use the last observed sample, not hardware triggers.".into());
    }
    if sidecar.status == "cancelled" {
        warnings.push(
            "Cancelled experiment explicitly allowed; only complete unpaused trials are eligible."
                .into(),
        );
    }
    let mut reasons: BTreeMap<String, usize> = BTreeMap::new();
    for e in &results.excluded {
        for code in e.reason_codes.split('|') {
            *reasons.entry(code.into()).or_default() += 1;
        }
    }
    if !results.excluded.is_empty() {
        warnings.push(format!(
            "{} trial-channel pairs excluded; see excluded-trials.csv",
            results.excluded.len()
        ));
    }
    if comparisons.is_empty() {
        warnings.push("No eligible trial comparisons; no figures generated.".into());
    }
    if results
        .windows
        .iter()
        .any(|w| w.included && w.absolute_power == 0.)
    {
        warnings.push(
            "Zero power has -100% change; change_db is empty because log10(0) is not finite."
                .into(),
        );
    }
    let output = prepare_output(&a.output, bundle.directory())?;
    let mut artifacts=vec![
        table(&output,"trial-results.csv","bundle,trial_number,condition,channel,band,anchor_timestamp_ns,baseline_window_count,analysis_window_count,baseline_median_power,analysis_median_power,relative_change_percent,change_db,change_direction,included,exclusion_reasons",&results.trials)?,
        table(&output,"windowed-power.csv","trial_number,condition,channel,band,relative_time_seconds,window_start_ns,window_end_ns,segment_id,interval,absolute_power,baseline_power,relative_change_percent,change_db,included",&results.windows)?,
        table(&output,"excluded-trials.csv","trial_number,condition,channel,reason_codes,details",&results.excluded)?,
        table(&output,"rejected-windows.csv","trial_number,condition,channel,window_index,window_start_ns,window_end_ns,segment_id,reason_codes",&results.rejected)?,
        table(&output,"timecourse.csv","condition,channel,band,relative_time_seconds,trial_count,median_percent,q1_percent,q3_percent",&curves)?,
        table(&output,"condition-summary.csv","condition,channel,band,relative_time_seconds,trial_count,median_percent,q1_percent,q3_percent",&comparisons)?,
    ];
    let versions = format!(
        "DSP 3 / PSD {} / WINDOWING {}",
        processing::PSD_ALGORITHM_VERSION,
        processing::windows::WINDOWING_ALGORITHM_VERSION
    );
    for path in plot::create(&output, &config, &curves, &comparisons, &versions)? {
        artifacts.push(Artifact {
            path,
            kind: ArtifactKind::Figure,
            description: format!("Median and IQR across trials; {versions}"),
        });
    }
    let included_ids: BTreeSet<_> = results
        .trials
        .iter()
        .filter(|r| r.included)
        .map(|r| r.trial_number)
        .collect();
    let fully_included = trials
        .iter()
        .filter(|t| !results.excluded.iter().any(|r| r.trial_number == t.number))
        .count();
    let mut per_channel = BTreeMap::new();
    for ch in &config.channels {
        let excluded = results.excluded.iter().filter(|r| r.channel == *ch).count();
        per_channel.insert(
            ch,
            json!({"total":trials.len(),"included":trials.len()-excluded,"excluded":excluded}),
        );
    }
    let m = bundle.manifest();
    let summary = json!({
        "title":"Event-locked Mu-band power analysis","plugin":{"id":PLUGIN_ID,"version":PLUGIN_VERSION},
        "input_bundle":{"hash":hash,"hash_algorithm":"SHA256 of sorted JSON map of file SHA256 digests","files":input_hashes,"analysis_format_version":m.analysis_format_version,"lemon_version":m.lemon_version},
        "dsp_algorithm_version":m.dsp_algorithm_version,"psd_algorithm_version":m.psd_algorithm_version,"windowing_algorithm_version":m.windowing_algorithm_version,
        "protocol":sidecar.protocol,"experiment_status":sidecar.status,"marker_timestamp_basis":sidecar.marker_timestamp_basis,
        "config":config,"sample_rate_hz":bundle.sample_rate_hz(),"units":bundle.units(),"absolute_power_units":format!("{}^2",bundle.units().unwrap_or("unknown")),
        "processing_config":m.processing_config,"input_spectral_config":m.spectral_config,"processing_mode":m.processing_mode,
        "psd":{"method":"mean-centered one-sided periodogram","window_function":"hann_symmetric","window_samples":wc.window_samples,"hop_samples":wc.hop_samples,"frequency_resolution_hz":bundle.sample_rate_hz()/wc.window_samples as f64,"integration":"sum PSD[k]*df; low <= f < high"},
        "total_trials":trials.len(),"included_trials":included_ids.len(),"excluded_trials":trials.len()-included_ids.len(),"fully_included_trials":fully_included,"trial_count_definition":"included if at least one channel eligible; see per_channel_counts for partial exclusions","per_channel_counts":per_channel,
        "exclusion_reasons":reasons,"input_excluded_window_reasons":input_exclusions,
        "aggregation":"type-7 median/Q1/Q3 across included trials at exactly equal window-start offsets (nanoseconds); no interpolation or pooling distinct offsets",
        "trial_policy":"any artifact, discontinuity, warmup or LowQuality flag in epoch excludes that trial-channel; incomplete generator tails alone do not exclude a trial with enough fully contained windows",
        "warnings":warnings,"physiological_interpretation":false,
        "limitations":["Single bundle only; descriptive statistics, no significance tests.","Source channel names are retained; no electrode/hemisphere mapping inferred.","No phase, coherence or asymmetry analysis.","Overlapping windows are correlated; trial_count counts distinct trials.","Paused trials are excluded; missing or duplicate markers cannot be reconstructed."]
    });
    fs::write(
        output.join("summary.json"),
        serde_json::to_vec_pretty(&summary).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    artifacts.push(Artifact {
        path: "summary.json".into(),
        kind: ArtifactKind::Data,
        description: "Provenance, settings, counts and limitations".into(),
    });
    let mut result = PluginResult::completed(PLUGIN_ID, PLUGIN_VERSION, artifacts);
    result.warnings = warnings;
    result.status = PluginStatus::CompletedWithWarnings;
    write_plugin_result(&output, &result).map_err(|e| e.to_string())?;
    Ok(())
}
