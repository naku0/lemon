use crate::{config::Config, markers::Trial};
use processing::{
    integrate_band_power, one_sided_psd,
    windows::{AnalysisSample, WindowCandidate, WindowGenerator, WindowingConfig},
    WindowFunction,
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Serialize)]
pub struct TrialRow {
    pub dsp_algorithm_version: u32,
    pub psd_algorithm_version: u32,
    pub windowing_algorithm_version: u32,
    pub bundle: String,
    pub trial_number: u32,
    pub condition: String,
    pub channel: String,
    pub band: String,
    pub anchor_timestamp_ns: Option<u64>,
    pub baseline_window_count: usize,
    pub analysis_window_count: usize,
    pub baseline_median_power: Option<f64>,
    pub analysis_median_power: Option<f64>,
    pub relative_change_percent: Option<f64>,
    pub change_db: Option<f64>,
    pub change_direction: String,
    pub included: bool,
    pub exclusion_reasons: String,
}
#[derive(Debug, Serialize)]
pub struct WindowRow {
    pub dsp_algorithm_version: u32,
    pub psd_algorithm_version: u32,
    pub windowing_algorithm_version: u32,
    pub trial_number: u32,
    pub condition: String,
    pub channel: String,
    pub band: String,
    pub relative_time_seconds: f64,
    pub window_start_ns: u64,
    pub window_end_ns: u64,
    pub segment_id: u64,
    pub interval: String,
    pub absolute_power: f64,
    pub baseline_power: Option<f64>,
    pub relative_change_percent: Option<f64>,
    pub change_db: Option<f64>,
    pub included: bool,
}
#[derive(Debug, Serialize)]
pub struct ExcludedRow {
    pub dsp_algorithm_version: u32,
    pub psd_algorithm_version: u32,
    pub windowing_algorithm_version: u32,
    pub trial_number: u32,
    pub condition: String,
    pub channel: String,
    pub reason_codes: String,
    pub details: String,
}
#[derive(Debug, Serialize)]
pub struct RejectedRow {
    pub dsp_algorithm_version: u32,
    pub psd_algorithm_version: u32,
    pub windowing_algorithm_version: u32,
    pub trial_number: u32,
    pub condition: String,
    pub channel: String,
    pub window_index: u64,
    pub window_start_ns: u64,
    pub window_end_ns: u64,
    pub segment_id: Option<u64>,
    pub reason_codes: String,
}
#[derive(Debug, Serialize)]
pub struct Aggregate {
    pub dsp_algorithm_version: u32,
    pub psd_algorithm_version: u32,
    pub windowing_algorithm_version: u32,
    pub condition: String,
    pub channel: String,
    pub band: String,
    pub relative_time_seconds: Option<f64>,
    pub trial_count: usize,
    pub median_percent: f64,
    pub q1_percent: f64,
    pub q3_percent: f64,
}
#[derive(Default)]
pub struct Results {
    pub trials: Vec<TrialRow>,
    pub windows: Vec<WindowRow>,
    pub excluded: Vec<ExcludedRow>,
    pub rejected: Vec<RejectedRow>,
}
// Linear quantiles (type 7); median averages the two middle values for even N.
pub fn quantile(v: &[f64], q: f64) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    let mut v = v.to_vec();
    v.sort_by(f64::total_cmp);
    let i = (v.len() - 1) as f64 * q;
    let lo = i.floor() as usize;
    let hi = i.ceil() as usize;
    Some(v[lo] * (1. - i.fract()) + v[hi] * i.fract())
}
fn change(power: Option<f64>, baseline: Option<f64>) -> (Option<f64>, Option<f64>) {
    match (power, baseline) {
        (Some(p), Some(b)) if p >= 0. && b > 0. => {
            let ratio = p / b;
            let pct = 100. * (ratio - 1.);
            let db = 10. * ratio.log10();
            (pct.is_finite().then_some(pct), db.is_finite().then_some(db))
        }
        _ => (None, None),
    }
}
pub fn relative_ns(time: u64, anchor: u64) -> i128 {
    time as i128 - anchor as i128
}
fn bounds(anchor: u64, interval: [f64; 2]) -> [i128; 2] {
    interval.map(|s| anchor as i128 + (s * 1e9).round() as i128)
}
fn codes(reasons: &BTreeSet<String>) -> String {
    reasons.iter().cloned().collect::<Vec<_>>().join("|")
}

pub fn analyze_trial(
    c: &Config,
    wc: &WindowingConfig,
    trial: &Trial,
    channel: &str,
    samples: &[AnalysisSample],
    bundle_hash: &str,
    out: &mut Results,
) -> Result<(), String> {
    let mut reasons = trial.reasons.clone();
    let mut windows = Vec::new();
    let psd_version = processing::PSD_ALGORITHM_VERSION;
    let win_version = processing::windows::WINDOWING_ALGORITHM_VERSION;
    if let Some(anchor) = trial.anchor {
        let epoch = bounds(anchor, c.epoch_seconds);
        let baseline = bounds(anchor, c.baseline_seconds);
        let analysis = bounds(anchor, c.analysis_seconds);
        for (requested, phase, reason) in [
            (baseline, trial.baseline, "BaselineOutsidePhase"),
            (analysis, trial.task, "AnalysisOutsidePhase"),
        ] {
            if phase.is_none_or(|p| requested[0] < p[0] as i128 || requested[1] > p[1] as i128) {
                reasons.insert(reason.into());
            }
        }
        if trial
            .extent
            .is_none_or(|p| epoch[0] < p[0] as i128 || epoch[1] > p[1] as i128)
        {
            reasons.insert("EpochOutsideTrial".into());
        }
        let start = samples.partition_point(|s| (s.timestamp_ns as i128) < epoch[0]);
        let end = samples.partition_point(|s| (s.timestamp_ns as i128) < epoch[1]);
        let epoch_samples = &samples[start..end];
        let dt = 1e9 / wc.sample_rate_hz;
        if epoch_samples
            .first()
            .is_none_or(|s| (s.timestamp_ns as i128 - epoch[0]) as f64 > dt * 0.51)
            || epoch_samples
                .last()
                .is_none_or(|s| (epoch[1] - s.timestamp_ns as i128) as f64 - dt > dt * 0.51)
        {
            reasons.insert("IncompleteEpoch".into());
        }
        let mut generator = WindowGenerator::new(wc.clone())?;
        let mut consume = |candidate: WindowCandidate| -> Result<(), String> {
            match candidate {
                WindowCandidate::Accepted {
                    segment_id,
                    start_timestamp_ns,
                    end_timestamp_ns,
                    samples,
                    ..
                } => {
                    // End-exclusive support includes the last sample's sampling period.
                    let support_end = |bound: i128| (end_timestamp_ns as i128 - bound) as f64 + dt;
                    let interval = if start_timestamp_ns as i128 >= baseline[0]
                        && support_end(baseline[1]) <= 1.
                    {
                        "baseline"
                    } else if start_timestamp_ns as i128 >= analysis[0]
                        && support_end(analysis[1]) <= 1.
                    {
                        "analysis"
                    } else {
                        return Ok(());
                    };
                    let values = samples
                        .iter()
                        .map(|s| {
                            s.value
                                .ok_or("accepted window has no filtered value".to_string())
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    let psd =
                        one_sided_psd(&values, wc.sample_rate_hz, WindowFunction::HannSymmetric)?;
                    for (band, range) in [("Mu", c.bands.mu_hz), ("Beta", c.bands.beta_hz)] {
                        let power = integrate_band_power(&psd, range[0], range[1])?;
                        if !power.is_finite() {
                            return Err("band power overflow".into());
                        }
                        windows.push(WindowRow {
                            dsp_algorithm_version: 3,
                            psd_algorithm_version: psd_version,
                            windowing_algorithm_version: win_version,
                            trial_number: trial.number,
                            condition: trial.condition.clone(),
                            channel: channel.into(),
                            band: band.into(),
                            relative_time_seconds: relative_ns(start_timestamp_ns, anchor) as f64
                                / 1e9,
                            window_start_ns: start_timestamp_ns,
                            window_end_ns: end_timestamp_ns,
                            segment_id,
                            interval: interval.into(),
                            absolute_power: power,
                            baseline_power: None,
                            relative_change_percent: None,
                            change_db: None,
                            included: false,
                        });
                    }
                }
                WindowCandidate::Rejected {
                    index,
                    segment_id,
                    start_timestamp_ns,
                    end_timestamp_ns,
                    reasons: rs,
                    ..
                } => {
                    for reason in &rs {
                        if reason.code() != "InsufficientData" {
                            reasons.insert(reason.code().into());
                        }
                    }
                    out.rejected.push(RejectedRow {
                        dsp_algorithm_version: 3,
                        psd_algorithm_version: psd_version,
                        windowing_algorithm_version: win_version,
                        trial_number: trial.number,
                        condition: trial.condition.clone(),
                        channel: channel.into(),
                        window_index: index,
                        window_start_ns: start_timestamp_ns,
                        window_end_ns: end_timestamp_ns,
                        segment_id,
                        reason_codes: rs.iter().map(|r| r.code()).collect::<Vec<_>>().join("|"),
                    });
                }
            }
            Ok(())
        };
        for sample in epoch_samples {
            for candidate in generator.push(sample.clone()) {
                consume(candidate)?;
            }
        }
        if let Some(tail) = generator.finish() {
            consume(tail)?;
        }
        if epoch_samples
            .iter()
            .map(|s| s.segment_id)
            .collect::<BTreeSet<_>>()
            .len()
            > 1
        {
            reasons.insert("MixedSegment".into());
        }
    } else {
        reasons.insert("MissingAnchor".into());
    }
    let baseline_count = windows
        .iter()
        .filter(|w| w.interval == "baseline" && w.band == "Mu")
        .count();
    let analysis_count = windows
        .iter()
        .filter(|w| w.interval == "analysis" && w.band == "Mu")
        .count();
    if baseline_count < c.minimum_baseline_windows {
        reasons.insert("InsufficientBaselineWindows".into());
    }
    if analysis_count < c.minimum_analysis_windows {
        reasons.insert("InsufficientAnalysisWindows".into());
    }
    let mut stats = Vec::new();
    for band in ["Mu", "Beta"] {
        let baseline: Vec<_> = windows
            .iter()
            .filter(|w| w.band == band && w.interval == "baseline")
            .map(|w| w.absolute_power)
            .collect();
        let analysis: Vec<_> = windows
            .iter()
            .filter(|w| w.band == band && w.interval == "analysis")
            .map(|w| w.absolute_power)
            .collect();
        let base = quantile(&baseline, 0.5);
        let power = quantile(&analysis, 0.5);
        if base.is_some_and(|b| b <= 0.) {
            reasons.insert(format!("NonPositiveBaseline{band}"));
        }
        if let (Some(p), Some(b)) = (power, base) {
            if b > 0. && !(p / b).is_finite() {
                reasons.insert("NormalizationOverflow".into());
            }
        }
        stats.push((band, base, power));
    }
    let included = reasons.is_empty();
    for (band, base, power) in stats {
        let (pct, db) = if included {
            change(power, base)
        } else {
            (None, None)
        };
        for window in windows.iter_mut().filter(|w| w.band == band) {
            window.baseline_power = base;
            window.included = included;
            if included {
                (window.relative_change_percent, window.change_db) =
                    change(Some(window.absolute_power), base);
            }
        }
        out.trials.push(TrialRow {
            dsp_algorithm_version: 3,
            psd_algorithm_version: psd_version,
            windowing_algorithm_version: win_version,
            bundle: bundle_hash.into(),
            trial_number: trial.number,
            condition: trial.condition.clone(),
            channel: channel.into(),
            band: band.into(),
            anchor_timestamp_ns: trial.anchor,
            baseline_window_count: baseline_count,
            analysis_window_count: analysis_count,
            baseline_median_power: base,
            analysis_median_power: power,
            relative_change_percent: pct,
            change_db: db,
            change_direction: match pct {
                Some(p) if p < 0. => "decrease relative to trial baseline",
                Some(p) if p > 0. => "increase relative to trial baseline",
                Some(_) => "unchanged relative to trial baseline",
                None => "not evaluated",
            }
            .into(),
            included,
            exclusion_reasons: codes(&reasons),
        });
    }
    if !included {
        out.excluded.push(ExcludedRow{dsp_algorithm_version:3,psd_algorithm_version:psd_version,windowing_algorithm_version:win_version,trial_number:trial.number,condition:trial.condition.clone(),channel:channel.into(),reason_codes:codes(&reasons),details:format!("baseline windows={baseline_count}; analysis windows={analysis_count}; see rejected-windows.csv")});
    }
    out.windows.extend(windows);
    Ok(())
}

pub fn aggregates(out: &Results, timecourse: bool) -> Vec<Aggregate> {
    type Key = (String, String, String, Option<i64>);
    let mut groups: BTreeMap<Key, BTreeMap<u32, f64>> = BTreeMap::new();
    if timecourse {
        for w in out.windows.iter().filter(|w| w.included) {
            if let Some(value) = w.relative_change_percent {
                groups
                    .entry((
                        w.condition.clone(),
                        w.channel.clone(),
                        w.band.clone(),
                        Some((w.relative_time_seconds * 1e9).round() as i64),
                    ))
                    .or_default()
                    .insert(w.trial_number, value);
            }
        }
    } else {
        for t in out.trials.iter().filter(|t| t.included) {
            if let Some(value) = t.relative_change_percent {
                groups
                    .entry((t.condition.clone(), t.channel.clone(), t.band.clone(), None))
                    .or_default()
                    .insert(t.trial_number, value);
            }
        }
    }
    groups
        .into_iter()
        .map(|((condition, channel, band, time), values)| {
            let v: Vec<_> = values.values().copied().collect();
            Aggregate {
                dsp_algorithm_version: 3,
                psd_algorithm_version: processing::PSD_ALGORITHM_VERSION,
                windowing_algorithm_version: processing::windows::WINDOWING_ALGORITHM_VERSION,
                condition,
                channel,
                band,
                relative_time_seconds: time.map(|t| t as f64 / 1e9),
                trial_count: v.len(),
                median_percent: quantile(&v, 0.5).unwrap_or(0.),
                q1_percent: quantile(&v, 0.25).unwrap_or(0.),
                q3_percent: quantile(&v, 0.75).unwrap_or(0.),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quantiles_and_zero_power_have_defined_semantics() {
        assert_eq!(quantile(&[1., 2., 3., 4.], 0.5), Some(2.5));
        assert_eq!(quantile(&[1., 2., 3., 4.], 0.25), Some(1.75));
        assert_eq!(quantile(&[], 0.5), None);
        assert_eq!(change(Some(0.), Some(2.)), (Some(-100.), None));
        assert_eq!(change(Some(1.), Some(0.)), (None, None));
    }
    #[test]
    fn relative_time_retains_nanoseconds_for_large_source_timestamps() {
        let anchor = u64::MAX - 100;
        assert_eq!(relative_ns(anchor - 1, anchor), -1);
        assert_eq!(relative_ns(anchor + 1, anchor), 1);
    }
}
