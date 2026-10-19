use lemon_plugin_sdk::Bundle;
use processing::windows::{WindowGenerator, WindowingConfig, WINDOWING_ALGORITHM_VERSION};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub mode: String,
    pub channels: Vec<String>,
    pub conditions: Vec<String>,
    pub anchor_event: String,
    pub epoch_seconds: [f64; 2],
    pub baseline_seconds: [f64; 2],
    pub analysis_seconds: [f64; 2],
    pub bands: Bands,
    pub window_seconds: f64,
    pub hop_seconds: f64,
    pub minimum_baseline_windows: usize,
    pub minimum_analysis_windows: usize,
    #[serde(default)]
    pub allow_cancelled: bool,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Bands {
    pub mu_hz: [f64; 2],
    pub beta_hz: [f64; 2],
}
impl Config {
    pub fn validate(&self, b: &Bundle) -> Result<WindowingConfig, String> {
        if self.mode != "event-locked" {
            return Err("mode must be event-locked".into());
        }
        // Version 1 supports a task anchor only, so baseline/task phase checks are unambiguous.
        if self.anchor_event != "TaskStart" {
            return Err("anchor_event must be TaskStart in version 1".into());
        }
        for (label, values) in [
            ("channels", &self.channels),
            ("conditions", &self.conditions),
        ] {
            if values.is_empty()
                || values.len() > 64
                || values.iter().any(|s| s.trim().is_empty() || s.len() > 128)
                || values.iter().collect::<BTreeSet<_>>().len() != values.len()
            {
                return Err(format!(
                    "{label} must contain 1..64 unique nonempty names (max 128 bytes)"
                ));
            }
        }
        for ch in &self.channels {
            if !b.channels().contains(ch) {
                return Err(format!("unknown channel {ch}"));
            }
        }
        for r in [
            self.epoch_seconds,
            self.baseline_seconds,
            self.analysis_seconds,
        ] {
            if !r.iter().all(|v| v.is_finite() && v.abs() <= 86400.) || r[0] >= r[1] {
                return Err("intervals must be finite, ordered and within +/-86400 seconds".into());
            }
        }
        if self.baseline_seconds[1] > 0.
            || self.analysis_seconds[0] < 0.
            || [self.baseline_seconds, self.analysis_seconds]
                .iter()
                .any(|r| r[0] < self.epoch_seconds[0] || r[1] > self.epoch_seconds[1])
        {
            return Err(
                "epoch must contain baseline (ending <=0) and analysis (starting >=0)".into(),
            );
        }
        if self.minimum_baseline_windows == 0 || self.minimum_analysis_windows == 0 {
            return Err("minimum window counts must be positive".into());
        }
        let fs = b.sample_rate_hz();
        let counts = [self.window_seconds, self.hop_seconds].map(|s| s * fs);
        if !fs.is_finite()
            || fs <= 0.
            || counts
                .iter()
                .any(|n| !n.is_finite() || *n < 1. || *n > 65536. || (n.round() - n).abs() > 1e-6)
        {
            return Err("window/hop must produce positive integral sample counts <=65536".into());
        }
        let wc = WindowingConfig::new(counts[0].round() as usize, counts[1].round() as usize, fs);
        WindowGenerator::new(wc.clone())?;
        for r in [self.bands.mu_hz, self.bands.beta_hz] {
            if !r.iter().all(|v| v.is_finite()) || r[0] <= 0. || r[0] >= r[1] || r[1] >= fs / 2. {
                return Err("bands must be finite, positive and strictly below Nyquist".into());
            }
            let df = fs / wc.window_samples as f64;
            if (r[0] / df).ceil() * df >= r[1] {
                return Err("band contains no PSD bins for this window".into());
            }
        }
        if self.bands.mu_hz[0] < self.bands.beta_hz[1]
            && self.bands.beta_hz[0] < self.bands.mu_hz[1]
        {
            return Err("bands must not overlap".into());
        }
        let m = b.manifest();
        if m.analysis_format_version != 2
            || m.dsp_algorithm_version != 3
            || m.psd_algorithm_version != processing::PSD_ALGORITHM_VERSION
            || m.windowing_algorithm_version != WINDOWING_ALGORITHM_VERSION
            || m.window_function != "hann_symmetric"
            || m.processing_mode != "causal_live_compatible"
        {
            return Err("requires AnalysisBundle v2, DSP v3 and compatible PSD/windowing/causal Hann provenance".into());
        }
        if !m.processing_config.is_object()
            || !m.spectral_config.is_object()
            || m.processing_config.as_object().is_none_or(|v| v.is_empty())
            || m.spectral_config["window_samples"].as_u64().is_none()
            || m.spectral_config["hop_samples"].as_u64().is_none()
        {
            return Err("manifest processing/spectral parameters are missing".into());
        }
        Ok(wc)
    }
}
