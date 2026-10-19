use serde::Deserialize;
use std::{collections::BTreeSet, path::PathBuf};

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

impl Config {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.mode != "condition-comparison" {
            return Err("mode must be condition-comparison".into());
        }
        if self.channels.is_empty() || self.channels.len() > 32 {
            return Err("channels must contain 1..32 entries".into());
        }
        if self.conditions.is_empty() {
            return Err("conditions must contain at least one comparison bundle".into());
        }
        let mut unique = BTreeSet::new();
        if self
            .channels
            .iter()
            .any(|channel| channel.trim().is_empty() || !unique.insert(channel))
        {
            return Err("channels must be non-empty and unique".into());
        }
        if !self.window_seconds.is_finite()
            || self.window_seconds <= 0.0
            || !self.hop_seconds.is_finite()
            || self.hop_seconds <= 0.0
        {
            return Err("window_seconds and hop_seconds must be positive finite values".into());
        }
        for (name, range) in [("mu_hz", self.bands.mu_hz), ("beta_hz", self.bands.beta_hz)] {
            if !range[0].is_finite()
                || !range[1].is_finite()
                || range[0] < 0.0
                || range[0] >= range[1]
            {
                return Err(format!("invalid {name} range"));
            }
        }
        if self.bands.mu_hz[0] < self.bands.beta_hz[1]
            && self.bands.beta_hz[0] < self.bands.mu_hz[1]
        {
            return Err("Mu and Beta ranges must not overlap".into());
        }
        let mut labels = BTreeSet::new();
        for label in std::iter::once(&self.baseline.label)
            .chain(self.conditions.iter().map(|condition| &condition.label))
        {
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
}
