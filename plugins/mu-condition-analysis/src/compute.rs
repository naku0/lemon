use processing::windows::{
    flags_from_codes, AnalysisSample, LegacySegments, SignalFlag, WindowCandidate, WindowGenerator,
    WindowingConfig,
};
use processing::{integrate_band_power, one_sided_psd, WindowFunction};
use std::collections::BTreeMap;

use crate::{
    input::{Event, Sample},
    results::{ConditionResult, ExcludedWindow, WindowPower},
    Config,
};

pub(crate) struct ChannelAnalysis<'a> {
    pub(crate) condition: &'a str,
    pub(crate) bundle_name: &'a str,
    pub(crate) channel: &'a str,
    pub(crate) samples: &'a [Sample],
    pub(crate) events: &'a [Event],
    pub(crate) config: &'a Config,
    pub(crate) window_samples: usize,
    pub(crate) hop_samples: usize,
    pub(crate) sample_rate_hz: f64,
    pub(crate) dsp_algorithm_version: u32,
    pub(crate) psd_algorithm_version: u32,
    pub(crate) sample_quality_reliable: bool,
    pub(crate) legacy: bool,
    pub(crate) reject_low_quality: bool,
}

#[derive(Default)]
pub(crate) struct ChannelAnalysisResult {
    pub(crate) powers: Vec<WindowPower>,
    pub(crate) excluded: Vec<ExcludedWindow>,
}

pub(crate) fn analyze_channel(input: ChannelAnalysis<'_>) -> Result<ChannelAnalysisResult, String> {
    let mut window_config = WindowingConfig::new(
        input.window_samples,
        input.hop_samples,
        input.sample_rate_hz,
    );
    window_config.reject_low_quality = input.reject_low_quality;
    let mut segments = LegacySegments::new(window_config.clone())?;
    let mut generator = WindowGenerator::new(window_config)?;
    let mut output = ChannelAnalysisResult::default();
    for sample in input.samples {
        let mut flags = flags_from_codes(&sample.flags);
        // v2 per-sample metadata is authoritative. Legacy event records supply
        // conservative additional flags, not a second window-validation pass.
        if input.legacy {
            if input.sample_quality_reliable
                && !matches!(sample.quality.as_str(), "Good" | "Acceptable" | "Fair")
            {
                flags.push(SignalFlag::LowQuality);
            }
            for event in input.events.iter().filter(|event| {
                event.timestamp_ns == sample.timestamp_ns
                    && event
                        .channel
                        .as_deref()
                        .is_none_or(|id| id == input.channel)
            }) {
                let code = event.reason.trim_start_matches("quality event: ");
                flags.extend(flags_from_codes(code));
            }
        }
        let segment_id = if input.legacy {
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
            consume_candidate(candidate, &input, &mut output)?;
        }
    }
    if let Some(candidate) = generator.finish() {
        consume_candidate(candidate, &input, &mut output)?;
    }
    Ok(output)
}

fn consume_candidate(
    candidate: WindowCandidate,
    input: &ChannelAnalysis<'_>,
    output: &mut ChannelAnalysisResult,
) -> Result<(), String> {
    match candidate {
        WindowCandidate::Rejected {
            index,
            segment_id,
            start_timestamp_ns,
            end_timestamp_ns,
            reasons,
            ..
        } => {
            output.excluded.push(ExcludedWindow {
                dsp_algorithm_version: input.dsp_algorithm_version,
                bundle: input.bundle_name.into(),
                condition: input.condition.into(),
                channel: input.channel.into(),
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
            let psd = one_sided_psd(&values, input.sample_rate_hz, WindowFunction::HannSymmetric)?;
            for (band, range) in [
                ("Mu", input.config.bands.mu_hz),
                ("Beta", input.config.bands.beta_hz),
            ] {
                output.powers.push(WindowPower {
                    dsp_algorithm_version: input.dsp_algorithm_version,
                    psd_algorithm_version: input.psd_algorithm_version,
                    window_function: "hann_symmetric".into(),
                    bundle: input.bundle_name.into(),
                    condition: input.condition.into(),
                    channel: input.channel.into(),
                    window_index: index,
                    segment_id: Some(segment_id),
                    window_start_ns: start_timestamp_ns,
                    window_end_ns: end_timestamp_ns,
                    midpoint_seconds: (start_timestamp_ns as f64 + end_timestamp_ns as f64) / 2e9,
                    band: band.into(),
                    low_hz: range[0],
                    high_hz: range[1],
                    power: integrate_band_power(&psd, range[0], range[1])?,
                });
            }
        }
    }
    Ok(())
}

pub(crate) fn aggregate(
    config: &Config,
    powers: &[WindowPower],
    excluded: &[ExcludedWindow],
    dsp_algorithm_version: u32,
    psd_algorithm_version: u32,
    warnings: &mut Vec<String>,
) -> Vec<ConditionResult> {
    let mut grouped: BTreeMap<(String, String, String), Vec<f64>> = BTreeMap::new();
    for power in powers {
        grouped
            .entry((
                power.condition.clone(),
                power.channel.clone(),
                power.band.clone(),
            ))
            .or_default()
            .push(power.power);
    }
    let mut excluded_counts: BTreeMap<(String, String), usize> = BTreeMap::new();
    for window in excluded {
        *excluded_counts
            .entry((window.condition.clone(), window.channel.clone()))
            .or_default() += 1;
    }
    let mut results = Vec::new();
    for condition in std::iter::once(&config.baseline.label)
        .chain(config.conditions.iter().map(|condition| &condition.label))
    {
        for channel in &config.channels {
            for band in ["Mu", "Beta"] {
                let key = (condition.clone(), channel.clone(), band.into());
                let values = grouped.get(&key).map(Vec::as_slice).unwrap_or(&[]);
                let median = quantile(values, 0.5);
                let iqr = quantile(values, 0.75)
                    .zip(quantile(values, 0.25))
                    .map(|(high, low)| high - low);
                let baseline = grouped
                    .get(&(config.baseline.label.clone(), channel.clone(), band.into()))
                    .and_then(|values| quantile(values, 0.5));
                let ratio = if condition == &config.baseline.label {
                    None
                } else {
                    median.zip(baseline).and_then(|(value, baseline)| {
                        if baseline > f64::EPSILON && value >= 0.0 {
                            Some(value / baseline)
                        } else {
                            None
                        }
                    })
                };
                if condition != &config.baseline.label && ratio.is_none() {
                    warnings.push(format!("Cannot calculate change for condition {condition:?}, channel {channel:?}, band {band}: baseline or condition median is unavailable or baseline is near zero."));
                }
                results.push(ConditionResult {
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
                    baseline_median_power: baseline,
                    relative_change_percent: ratio.map(|value| 100.0 * (value - 1.0)),
                    change_db: ratio
                        .filter(|value| *value > 0.0)
                        .map(|value| 10.0 * value.log10()),
                    change_direction: ratio.map(|value| {
                        if value < 1.0 {
                            "decrease relative to background"
                        } else if value > 1.0 {
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
    results
}

fn quantile(values: &[f64], q: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let position = q * (sorted.len() - 1) as f64;
    let low = position.floor() as usize;
    let high = position.ceil() as usize;
    Some(sorted[low] + (sorted[high] - sorted[low]) * (position - low as f64))
}
