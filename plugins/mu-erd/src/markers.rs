//! Validate the public experiment sidecar without depending on the experiment engine.
use crate::config::Config;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Deserialize, Serialize)]
pub struct Sidecar {
    pub schema_version: u32,
    pub protocol: Protocol,
    pub status: String,
    pub marker_timestamp_basis: String,
    pub resolved_plan: Vec<PlanTrial>,
    pub events: Vec<Event>,
}
#[derive(Debug, Deserialize, Serialize)]
pub struct Protocol {
    pub id: String,
    pub source_file_hash: String,
}
#[derive(Debug, Deserialize, Serialize)]
pub struct PlanTrial {
    pub trial_number: u32,
    pub condition: String,
}
#[derive(Debug, Deserialize, Serialize)]
pub struct Event {
    pub timestamp_ns: u64,
    pub kind: String,
    pub text: String,
}
#[derive(Debug, Deserialize)]
struct Payload {
    protocol_id: String,
    trial_number: Option<u32>,
    condition: Option<String>,
}
#[derive(Debug)]
pub struct Trial {
    pub number: u32,
    pub condition: String,
    pub anchor: Option<u64>,
    pub baseline: Option<[u64; 2]>,
    pub task: Option<[u64; 2]>,
    pub extent: Option<[u64; 2]>,
    pub reasons: BTreeSet<String>,
}
impl Sidecar {
    pub fn trials(&self, c: &Config) -> Result<Vec<Trial>, String> {
        if self.schema_version != 1
            || self.protocol.id.is_empty()
            || self.protocol.source_file_hash.is_empty()
        {
            return Err("invalid experiment schema or protocol identity/hash".into());
        }
        if self.status != "completed" && !(self.status == "cancelled" && c.allow_cancelled) {
            return Err(
                "experiment must be completed; cancelled requires allow_cancelled=true".into(),
            );
        }
        if !["source_timeline", "last_observed_sample"]
            .contains(&self.marker_timestamp_basis.as_str())
        {
            return Err("unsupported marker timestamp basis".into());
        }
        let plan: BTreeMap<_, _> = self
            .resolved_plan
            .iter()
            .map(|t| (t.trial_number, t.condition.as_str()))
            .collect();
        if plan.is_empty() || plan.len() != self.resolved_plan.len() || plan.contains_key(&0) {
            return Err("resolved plan must contain unique positive trial numbers".into());
        }
        for condition in &c.conditions {
            if !plan.values().any(|v| *v == condition) {
                return Err(format!("unknown condition {condition}"));
            }
        }
        let mut by_trial: BTreeMap<u32, Vec<&Event>> = BTreeMap::new();
        let mut prev = None;
        for event in &self.events {
            if prev.is_some_and(|p| event.timestamp_ns < p) {
                return Err("experiment timestamps are not monotonic".into());
            }
            prev = Some(event.timestamp_ns);
            let p: Payload = serde_json::from_str(&event.text)
                .map_err(|e| format!("invalid event payload: {e}"))?;
            if p.protocol_id != self.protocol.id {
                return Err("event protocol_id differs from sidecar".into());
            }
            if let Some(n) = p.trial_number {
                if plan.get(&n).copied() != p.condition.as_deref() {
                    return Err("event trial/condition differs from resolved plan".into());
                }
                by_trial.entry(n).or_default().push(event);
            }
        }
        if !self.events.iter().any(|e| e.kind == c.anchor_event) {
            return Err("anchor event TaskStart is missing".into());
        }
        let kinds = [
            "TrialStart",
            "BaselineStart",
            "BaselineEnd",
            "Cue",
            "TaskStart",
            "TaskEnd",
            "RestStart",
            "RestEnd",
            "TrialEnd",
        ];
        let mut out = Vec::new();
        for p in self
            .resolved_plan
            .iter()
            .filter(|t| c.conditions.contains(&t.condition))
        {
            let events = by_trial.get(&p.trial_number).cloned().unwrap_or_default();
            let mut reasons = BTreeSet::new();
            let mut times = BTreeMap::new();
            let mut positions = Vec::new();
            for kind in kinds {
                let found: Vec<_> = events
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| e.kind == kind)
                    .collect();
                if found.len() != 1 {
                    reasons.insert(
                        if found.is_empty() {
                            "IncompleteTrial"
                        } else {
                            "DuplicateMarker"
                        }
                        .into(),
                    );
                } else {
                    times.insert(kind, found[0].1.timestamp_ns);
                    positions.push(found[0].0);
                }
            }
            if positions.windows(2).any(|w| w[0] >= w[1]) {
                reasons.insert("MarkerOrder".into());
            }
            if events
                .iter()
                .any(|e| matches!(e.kind.as_str(), "ExperimentPaused" | "ExperimentResumed"))
            {
                reasons.insert("PausedTrial".into());
            }
            let interval = |a, b| times.get(a).zip(times.get(b)).map(|(x, y)| [*x, *y]);
            let baseline = interval("BaselineStart", "BaselineEnd");
            let task = interval("TaskStart", "TaskEnd");
            if [baseline, task].into_iter().flatten().any(|r| r[0] >= r[1]) {
                reasons.insert("InvalidPhaseDuration".into());
            }
            out.push(Trial {
                number: p.trial_number,
                condition: p.condition.clone(),
                anchor: times.get("TaskStart").copied(),
                baseline,
                task,
                extent: interval("TrialStart", "TrialEnd"),
                reasons,
            });
        }
        // Full trials must not overlap in the recording timeline.
        let extents: Vec<_> = out.iter().filter_map(|t| t.extent).collect();
        if extents.windows(2).any(|w| w[0][1] > w[1][0]) {
            return Err("trial timelines overlap".into());
        }
        Ok(out)
    }
}
