//! Deterministic experiment protocols and a source-timeline-driven phase engine.
use core_types::{Event, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Protocol {
    pub schema_version: u32,
    pub id: String,
    pub display_name: String,
    pub description: String,
    pub preparation_seconds: f64,
    pub randomization: Randomization,
    pub trials: Vec<TrialTemplate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Randomization {
    pub mode: RandomizationMode,
    #[serde(default)]
    pub seed: Option<u64>,
    #[serde(default)]
    pub avoid_immediate_repeats: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RandomizationMode {
    Fixed,
    Seeded,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrialTemplate {
    pub condition: String,
    pub label: String,
    pub repetitions: u32,
    pub baseline_seconds: f64,
    pub cue_seconds: f64,
    pub task_seconds: f64,
    pub rest_seconds: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedTrial {
    pub trial_number: u32,
    pub condition: String,
    pub label: String,
    pub baseline_seconds: f64,
    pub cue_seconds: f64,
    pub task_seconds: f64,
    pub rest_seconds: f64,
}

impl Protocol {
    pub fn load(path: &Path) -> Result<(Self, String)> {
        let bytes =
            fs::read(path).map_err(|e| format!("experiment/protocol {}: {e}", path.display()))?;
        if bytes.len() > 1_048_576 {
            return Err("experiment/protocol: file exceeds 1 MiB".into());
        }
        let protocol: Self = serde_json::from_slice(&bytes)
            .map_err(|e| format!("experiment/protocol {}: {e}", path.display()))?;
        protocol.validate()?;
        Ok((protocol, format!("sha256:{:x}", Sha256::digest(bytes))))
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(format!(
                "experiment/protocol: unsupported schema_version {}",
                self.schema_version
            ));
        }
        if !valid_id(&self.id) {
            return Err(
                "experiment/protocol: id must contain lowercase ASCII letters, digits and hyphens"
                    .into(),
            );
        }
        if !self.preparation_seconds.is_finite() || self.preparation_seconds < 0.0 {
            return Err(
                "experiment/protocol: preparation_seconds must be finite and non-negative".into(),
            );
        }
        if self.trials.is_empty() {
            return Err("experiment/protocol: at least one trial is required".into());
        }
        if self.randomization.mode == RandomizationMode::Seeded && self.randomization.seed.is_none()
        {
            return Err("experiment/protocol: seeded randomization requires seed".into());
        }
        if self.randomization.mode == RandomizationMode::Fixed && self.randomization.seed.is_some()
        {
            return Err("experiment/protocol: fixed randomization must not specify seed".into());
        }
        let mut counts = BTreeMap::<&str, usize>::new();
        let mut total = 0usize;
        for trial in &self.trials {
            if trial.condition.trim().is_empty() || trial.label.trim().is_empty() {
                return Err("experiment/protocol: condition and label must not be empty".into());
            }
            if trial.repetitions == 0 {
                return Err("experiment/protocol: repetitions must be positive".into());
            }
            for (name, value) in [
                ("baseline_seconds", trial.baseline_seconds),
                ("cue_seconds", trial.cue_seconds),
                ("task_seconds", trial.task_seconds),
                ("rest_seconds", trial.rest_seconds),
            ] {
                if !value.is_finite() || value <= 0.0 {
                    return Err(format!(
                        "experiment/protocol: {name} must be finite and positive"
                    ));
                }
            }
            let repetitions = trial.repetitions as usize;
            total = total
                .checked_add(repetitions)
                .ok_or("experiment/protocol: trial count overflow")?;
            *counts.entry(&trial.condition).or_default() += repetitions;
        }
        if total > 100_000 {
            return Err("experiment/protocol: resolved plan exceeds 100000 trials".into());
        }
        if self.randomization.avoid_immediate_repeats
            && counts.values().copied().max().unwrap_or(0) > total.div_ceil(2)
        {
            return Err(
                "experiment/protocol: avoid_immediate_repeats is impossible for these repetitions"
                    .into(),
            );
        }
        Ok(())
    }

    pub fn resolve(&self) -> Result<Vec<ResolvedTrial>> {
        self.validate()?;
        let mut expanded = Vec::new();
        for trial in &self.trials {
            for _ in 0..trial.repetitions {
                expanded.push(trial.clone());
            }
        }
        if self.randomization.mode == RandomizationMode::Seeded {
            let mut rng = DeterministicRng(self.randomization.seed.unwrap_or(0));
            if self.randomization.avoid_immediate_repeats {
                let mut remaining = expanded;
                let mut ordered = Vec::with_capacity(remaining.len());
                while !remaining.is_empty() {
                    let allowed = remaining
                        .iter()
                        .enumerate()
                        .filter(|(_, item)| {
                            if ordered.last().is_some_and(|last: &TrialTemplate| {
                                last.condition == item.condition
                            }) {
                                return false;
                            }
                            let mut counts = BTreeMap::<&str, usize>::new();
                            for candidate in &remaining {
                                *counts.entry(&candidate.condition).or_default() += 1;
                            }
                            if let Some(count) = counts.get_mut(item.condition.as_str()) {
                                *count -= 1;
                            }
                            let left = remaining.len() - 1;
                            counts.values().copied().max().unwrap_or(0) <= left.div_ceil(2)
                        })
                        .map(|(index, _)| index)
                        .collect::<Vec<_>>();
                    if allowed.is_empty() {
                        return Err(
                            "experiment/protocol: cannot resolve without immediate repeats".into(),
                        );
                    }
                    let picked = allowed[rng.index(allowed.len())];
                    ordered.push(remaining.remove(picked));
                }
                expanded = ordered;
            } else {
                for index in (1..expanded.len()).rev() {
                    let picked = rng.index(index + 1);
                    expanded.swap(index, picked);
                }
            }
        } else if self.randomization.avoid_immediate_repeats
            && expanded
                .windows(2)
                .any(|w| w[0].condition == w[1].condition)
        {
            return Err("experiment/protocol: fixed order contains an immediate repeat".into());
        }
        Ok(expanded
            .into_iter()
            .enumerate()
            .map(|(index, trial)| ResolvedTrial {
                trial_number: index as u32 + 1,
                condition: trial.condition,
                label: trial.label,
                baseline_seconds: trial.baseline_seconds,
                cue_seconds: trial.cue_seconds,
                task_seconds: trial.task_seconds,
                rest_seconds: trial.rest_seconds,
            })
            .collect())
    }

    pub fn scaled(&self, factor: f64) -> Result<Self> {
        if !factor.is_finite() || factor <= 0.0 || factor > 1000.0 {
            return Err("experiment: --time-scale must be finite and in (0,1000]".into());
        }
        let mut scaled = self.clone();
        scaled.preparation_seconds *= factor;
        for trial in &mut scaled.trials {
            trial.baseline_seconds *= factor;
            trial.cue_seconds *= factor;
            trial.task_seconds *= factor;
            trial.rest_seconds *= factor;
        }
        scaled.validate()?;
        Ok(scaled)
    }
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && !value.ends_with('-')
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

struct DeterministicRng(u64);
impl DeterministicRng {
    fn index(&mut self, length: usize) -> usize {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1);
        ((self.0 >> 32) as usize) % length
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Ready,
    Preparation,
    Baseline,
    Cue,
    Task,
    Rest,
    Paused,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone)]
pub enum Control {
    Start,
    PauseResume,
    Cancel(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub protocol_id: String,
    pub display_name: String,
    pub status: Status,
    pub trial_number: Option<u32>,
    pub total_trials: u32,
    pub condition: Option<String>,
    pub label: Option<String>,
    pub phase_remaining_ns: u64,
}

pub struct Engine {
    protocol: Protocol,
    plan: Vec<ResolvedTrial>,
    snapshot: Snapshot,
    trial_index: usize,
    deadline_ns: Option<u64>,
    paused_at_ns: Option<u64>,
    paused_phase: Option<Status>,
}

impl Engine {
    pub fn new(protocol: Protocol, plan: Vec<ResolvedTrial>) -> Result<Self> {
        if plan.is_empty() {
            return Err("experiment: resolved plan is empty".into());
        }
        Ok(Self {
            snapshot: Snapshot {
                protocol_id: protocol.id.clone(),
                display_name: protocol.display_name.clone(),
                status: Status::Ready,
                trial_number: None,
                total_trials: plan.len() as u32,
                condition: None,
                label: None,
                phase_remaining_ns: 0,
            },
            protocol,
            plan,
            trial_index: 0,
            deadline_ns: None,
            paused_at_ns: None,
            paused_phase: None,
        })
    }

    pub fn snapshot(&self) -> Snapshot {
        self.snapshot.clone()
    }
    pub fn plan(&self) -> &[ResolvedTrial] {
        &self.plan
    }
    pub fn start(&mut self, now_ns: u64) -> Result<Vec<Event>> {
        if self.snapshot.status != Status::Ready {
            return Err("experiment: can only start from ready state".into());
        }
        let mut events = vec![self.event(now_ns, "ExperimentStart", None)];
        self.snapshot.status = Status::Preparation;
        events.push(self.event(now_ns, "PreparationStart", Some("preparation")));
        self.deadline_ns =
            Some(now_ns.saturating_add(seconds_ns(self.protocol.preparation_seconds)));
        if self.protocol.preparation_seconds == 0.0 {
            events.extend(self.tick(now_ns));
        }
        Ok(events)
    }

    pub fn tick(&mut self, now_ns: u64) -> Vec<Event> {
        if self.snapshot.status == Status::Paused {
            return Vec::new();
        }
        let mut events = Vec::new();
        while self.deadline_ns.is_some_and(|deadline| now_ns >= deadline) {
            match self.snapshot.status {
                Status::Preparation => {
                    events.push(self.event(now_ns, "PreparationEnd", Some("preparation")));
                    self.begin_trial(now_ns, &mut events);
                }
                Status::Baseline => {
                    events.push(self.event(now_ns, "BaselineEnd", Some("baseline")));
                    self.enter(
                        Status::Cue,
                        now_ns,
                        self.current().cue_seconds,
                        "Cue",
                        "cue",
                        &mut events,
                    );
                }
                Status::Cue => self.enter(
                    Status::Task,
                    now_ns,
                    self.current().task_seconds,
                    "TaskStart",
                    "task",
                    &mut events,
                ),
                Status::Task => {
                    events.push(self.event(now_ns, "TaskEnd", Some("task")));
                    self.enter(
                        Status::Rest,
                        now_ns,
                        self.current().rest_seconds,
                        "RestStart",
                        "rest",
                        &mut events,
                    );
                }
                Status::Rest => {
                    events.push(self.event(now_ns, "RestEnd", Some("rest")));
                    events.push(self.event(now_ns, "TrialEnd", None));
                    self.trial_index += 1;
                    if self.trial_index == self.plan.len() {
                        self.snapshot.status = Status::Completed;
                        self.snapshot.phase_remaining_ns = 0;
                        self.deadline_ns = None;
                        events.push(self.event(now_ns, "ExperimentEnd", None));
                    } else {
                        self.begin_trial(now_ns, &mut events);
                    }
                }
                _ => break,
            }
        }
        self.snapshot.phase_remaining_ns = self
            .deadline_ns
            .map_or(0, |deadline| deadline.saturating_sub(now_ns));
        events
    }

    pub fn pause(&mut self, now_ns: u64) -> Result<Event> {
        if !matches!(
            self.snapshot.status,
            Status::Preparation | Status::Baseline | Status::Cue | Status::Task | Status::Rest
        ) {
            return Err("experiment: current state cannot be paused".into());
        }
        self.paused_at_ns = Some(now_ns);
        self.paused_phase = Some(self.snapshot.status);
        self.snapshot.status = Status::Paused;
        Ok(self.event(now_ns, "ExperimentPaused", None))
    }

    pub fn resume(&mut self, now_ns: u64) -> Result<Event> {
        let paused_at = self.paused_at_ns.take().ok_or("experiment: not paused")?;
        if let Some(deadline) = &mut self.deadline_ns {
            *deadline = deadline.saturating_add(now_ns.saturating_sub(paused_at));
        }
        let prior = self
            .paused_phase
            .take()
            .ok_or("experiment: paused phase missing")?;
        self.snapshot.status = prior;
        Ok(self.event(now_ns, "ExperimentResumed", None))
    }

    pub fn cancel(&mut self, now_ns: u64, reason: &str) -> Result<Event> {
        if matches!(
            self.snapshot.status,
            Status::Completed | Status::Cancelled | Status::Failed
        ) {
            return Err("experiment: already finished".into());
        }
        self.snapshot.status = Status::Cancelled;
        self.deadline_ns = None;
        let mut event = self.event(now_ns, "ExperimentCancelled", None);
        event.text = serde_json::to_string(&serde_json::json!({
            "protocol_id": self.protocol.id,
            "trial_number": self.snapshot.trial_number,
            "condition": self.snapshot.condition,
            "reason": reason,
        }))
        .unwrap_or_else(|_| "{}".into());
        Ok(event)
    }

    fn current(&self) -> &ResolvedTrial {
        &self.plan[self.trial_index]
    }
    fn begin_trial(&mut self, now_ns: u64, events: &mut Vec<Event>) {
        let trial = self.current().clone();
        self.snapshot.trial_number = Some(trial.trial_number);
        self.snapshot.condition = Some(trial.condition);
        self.snapshot.label = Some(trial.label);
        events.push(self.event(now_ns, "TrialStart", None));
        self.enter(
            Status::Baseline,
            now_ns,
            trial.baseline_seconds,
            "BaselineStart",
            "baseline",
            events,
        );
    }
    fn enter(
        &mut self,
        status: Status,
        now_ns: u64,
        seconds: f64,
        kind: &str,
        phase: &str,
        events: &mut Vec<Event>,
    ) {
        self.snapshot.status = status;
        self.deadline_ns = Some(now_ns.saturating_add(seconds_ns(seconds)));
        events.push(self.event(now_ns, kind, Some(phase)));
    }
    fn event(&self, timestamp_ns: u64, kind: &str, phase: Option<&str>) -> Event {
        Event {
            timestamp_ns,
            kind: kind.into(),
            text: serde_json::to_string(&serde_json::json!({
                "protocol_id": self.protocol.id,
                "trial_number": self.snapshot.trial_number,
                "condition": self.snapshot.condition,
                "phase": phase,
            }))
            .unwrap_or_else(|_| "{}".into()),
        }
    }
}

fn seconds_ns(seconds: f64) -> u64 {
    (seconds * 1e9).round() as u64
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolIdentity {
    pub id: String,
    pub source_file_hash: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExperimentSidecar {
    pub schema_version: u32,
    pub protocol: ProtocolIdentity,
    pub randomization: Randomization,
    pub resolved_plan: Vec<ResolvedTrial>,
    pub started_at_timestamp_ns: Option<u64>,
    pub ended_at_timestamp_ns: Option<u64>,
    pub status: String,
    pub cancellation_reason: Option<String>,
    pub marker_timestamp_basis: String,
    pub events: Vec<Event>,
}
impl ExperimentSidecar {
    pub fn new(protocol: &Protocol, hash: String, plan: Vec<ResolvedTrial>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            protocol: ProtocolIdentity {
                id: protocol.id.clone(),
                source_file_hash: hash,
            },
            randomization: protocol.randomization.clone(),
            resolved_plan: plan,
            started_at_timestamp_ns: None,
            ended_at_timestamp_ns: None,
            status: "running".into(),
            cancellation_reason: None,
            marker_timestamp_basis: "last_observed_sample".into(),
            events: Vec::new(),
        }
    }
    pub fn write_atomic(&self, directory: &Path) -> Result<PathBuf> {
        let path = directory.join("experiment.json");
        let temp = directory.join("experiment.json.tmp");
        let mut file = File::create(&temp).map_err(|e| format!("experiment/sidecar: {e}"))?;
        serde_json::to_writer_pretty(&mut file, self)
            .map_err(|e| format!("experiment/sidecar: {e}"))?;
        file.write_all(b"\n")
            .map_err(|e| format!("experiment/sidecar: {e}"))?;
        file.sync_all()
            .map_err(|e| format!("experiment/sidecar: {e}"))?;
        fs::rename(&temp, &path).map_err(|e| format!("experiment/sidecar: {e}"))?;
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn protocol() -> Protocol {
        serde_json::from_value(serde_json::json!({
            "schema_version":1,"id":"motor-demo","display_name":"Demo","description":"test",
            "preparation_seconds":0.0,
            "randomization":{"mode":"seeded","seed":42,"avoid_immediate_repeats":true},
            "trials":[
                {"condition":"left","label":"Left","repetitions":2,"baseline_seconds":1.0,"cue_seconds":1.0,"task_seconds":1.0,"rest_seconds":1.0},
                {"condition":"right","label":"Right","repetitions":2,"baseline_seconds":1.0,"cue_seconds":1.0,"task_seconds":1.0,"rest_seconds":1.0}
            ]
        })).expect("protocol")
    }
    #[test]
    fn validation_resolution_and_seed_are_deterministic() {
        let p = protocol();
        p.validate().expect("valid");
        let a = p.resolve().expect("plan");
        let b = p.resolve().expect("plan");
        assert_eq!(a, b);
        assert_eq!(a.len(), 4);
        assert!(a.windows(2).all(|w| w[0].condition != w[1].condition));
        let mut value = serde_json::to_value(p).expect("value");
        value["unknown"] = serde_json::json!(1);
        assert!(serde_json::from_value::<Protocol>(value).is_err());
    }
    #[test]
    fn invalid_protocols_are_rejected() {
        let mut p = protocol();
        p.id = "Bad".into();
        assert!(p.validate().is_err());
        let mut p = protocol();
        p.trials[0].repetitions = 0;
        assert!(p.validate().is_err());
        let mut p = protocol();
        p.trials[0].task_seconds = f64::NAN;
        assert!(p.validate().is_err());
        let mut p = protocol();
        p.randomization.seed = None;
        assert!(p.validate().is_err());
    }
    #[test]
    fn engine_orders_all_trial_markers_and_pause_cancel() {
        let p = protocol();
        let plan = p.resolve().expect("plan");
        let mut engine = Engine::new(p, plan).expect("engine");
        let mut events = engine.start(0).expect("start");
        events.extend(engine.tick(1_000_000_000));
        let paused = engine.pause(1_100_000_000).expect("pause");
        assert_eq!(paused.kind, "ExperimentPaused");
        assert!(engine.tick(9_000_000_000).is_empty());
        assert_eq!(
            engine.resume(9_000_000_000).expect("resume").kind,
            "ExperimentResumed"
        );
        assert_eq!(
            engine.cancel(10_000_000_000, "test").expect("cancel").kind,
            "ExperimentCancelled"
        );
        assert!(events.iter().any(|e| e.kind == "BaselineStart"));
    }
    #[test]
    fn fixed_order_and_impossible_repeat_constraint_are_checked() {
        let mut p = protocol();
        p.randomization = Randomization {
            mode: RandomizationMode::Fixed,
            seed: None,
            avoid_immediate_repeats: false,
        };
        let plan = p.resolve().expect("fixed plan");
        assert_eq!(
            plan.iter()
                .map(|trial| trial.condition.as_str())
                .collect::<Vec<_>>(),
            vec!["left", "left", "right", "right"]
        );
        p.randomization.avoid_immediate_repeats = true;
        assert!(p.resolve().is_err());

        let mut impossible = protocol();
        impossible.trials[0].repetitions = 4;
        impossible.trials[1].repetitions = 1;
        assert!(impossible.validate().is_err());
    }
    #[test]
    fn completed_engine_has_every_phase_for_every_trial() {
        let p = protocol();
        let plan = p.resolve().expect("plan");
        let mut engine = Engine::new(p, plan.clone()).expect("engine");
        let mut events = engine.start(0).expect("start");
        for second in 1..=20 {
            events.extend(engine.tick(second * 1_000_000_000));
        }
        assert_eq!(engine.snapshot().status, Status::Completed);
        assert_eq!(
            events.first().map(|event| event.kind.as_str()),
            Some("ExperimentStart")
        );
        assert_eq!(
            events.last().map(|event| event.kind.as_str()),
            Some("ExperimentEnd")
        );
        for trial in plan {
            let trial_events = events
                .iter()
                .filter(|event| {
                    event
                        .text
                        .contains(&format!("\"trial_number\":{}", trial.trial_number))
                })
                .map(|event| event.kind.as_str())
                .collect::<Vec<_>>();
            for required in [
                "TrialStart",
                "BaselineStart",
                "BaselineEnd",
                "Cue",
                "TaskStart",
                "TaskEnd",
                "RestStart",
                "RestEnd",
                "TrialEnd",
            ] {
                assert!(
                    trial_events.contains(&required),
                    "trial {} missing {required}",
                    trial.trial_number
                );
            }
        }
    }
}
