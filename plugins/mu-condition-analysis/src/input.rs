use lemon_plugin_sdk::Bundle;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone)]
pub(crate) struct Sample {
    pub(crate) timestamp_ns: u64,
    pub(crate) sequence: u64,
    pub(crate) value: Option<f64>,
    pub(crate) quality: String,
    pub(crate) flags: String,
    pub(crate) segment_id: u64,
    pub(crate) sample_valid: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct Event {
    pub(crate) timestamp_ns: u64,
    pub(crate) channel: Option<String>,
    pub(crate) reason: String,
}

pub(crate) type SamplesByChannel = BTreeMap<String, Vec<Sample>>;

pub(crate) fn load_bundle(
    bundle: &Bundle,
    channels: &[String],
) -> Result<(SamplesByChannel, Vec<Event>), String> {
    let selected: BTreeSet<_> = channels.iter().cloned().collect();
    let mut samples: BTreeMap<String, Vec<Sample>> = channels
        .iter()
        .map(|channel| (channel.clone(), Vec::new()))
        .collect();
    for row in bundle.processed().map_err(|error| error.to_string())? {
        let row = row.map_err(|error| error.to_string())?;
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
    for row in bundle.quality_events().map_err(|error| error.to_string())? {
        let row = row.map_err(|error| error.to_string())?;
        if severe_event(&row.kind, &row.severity)
            && (bundle.manifest().reject_low_quality_windows
                || !row.kind.to_ascii_lowercase().contains("lowquality"))
        {
            events.push(Event {
                timestamp_ns: row.timestamp_ns,
                channel: row.channel.filter(|channel| !channel.is_empty()),
                reason: format!("quality event: {}", row.kind),
            });
        }
    }
    Ok((samples, events))
}

fn severe_event(kind: &str, severity: &str) -> bool {
    let normalized_kind = kind.to_ascii_lowercase();
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
        .any(|fragment| normalized_kind.contains(fragment))
}
