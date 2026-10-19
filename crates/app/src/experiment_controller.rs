use app::cli::{self, Action, Cli};
use core_types::{Command, Result, RuntimeStatus};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

pub struct Request {
    protocol: experiment::Protocol,
    protocol_hash: String,
    plan: Vec<experiment::ResolvedTrial>,
    headless: bool,
}

impl Request {
    /// Resolve an experiment command before the runtime starts.
    pub fn from_cli(cli: &mut Cli) -> Result<Option<Self>> {
        let Some(Action::Experiment {
            command:
                cli::ExperimentAction::Run {
                    protocol,
                    demo,
                    time_scale,
                    ..
                },
        }) = &cli.command
        else {
            return Ok(None);
        };
        if *time_scale != 1.0 && !(*demo && cli.headless) {
            return Err("experiment: --time-scale is restricted to --demo --headless".into());
        }
        let (loaded, protocol_hash) = experiment::Protocol::load(protocol)?;
        let protocol = loaded.scaled(*time_scale)?;
        let plan = protocol.resolve()?;
        cli.record = true;
        Ok(Some(Self {
            protocol,
            protocol_hash,
            plan,
            headless: cli.headless,
        }))
    }
}

/// Bridge the pure experiment engine to the runtime event stream and sidecar.
pub fn start(
    request: Request,
    runtime_status: Arc<Mutex<RuntimeStatus>>,
    runtime_commands: SyncSender<Command>,
    stop: Arc<AtomicBool>,
) -> Result<(tui::ExperimentUi, JoinHandle<Result<PathBuf>>)> {
    let engine = experiment::Engine::new(request.protocol.clone(), request.plan.clone())?;
    let snapshot = Arc::new(Mutex::new(engine.snapshot()));
    let snapshot_worker = snapshot.clone();
    let (control_tx, control_rx) = mpsc::sync_channel(16);
    if request.headless {
        control_tx
            .try_send(experiment::Control::Start)
            .map_err(|e| format!("experiment/start: {e}"))?;
    }
    let ui = tui::ExperimentUi {
        snapshot,
        commands: control_tx,
    };
    let worker = thread::spawn(move || {
        let result = (|| -> Result<PathBuf> {
            let mut engine = engine;
            let mut sidecar = experiment::ExperimentSidecar::new(
                &request.protocol,
                request.protocol_hash,
                request.plan,
            );
            let recording_directory = loop {
                if stop.load(Ordering::Relaxed) {
                    return Err(
                        "experiment: recording stopped before session directory was available"
                            .into(),
                    );
                }
                let path = runtime_status
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .recording
                    .clone();
                if let Some(path) = path {
                    let raw = PathBuf::from(path);
                    break raw
                        .parent()
                        .map(PathBuf::from)
                        .ok_or("experiment: invalid recording path")?;
                }
                thread::sleep(Duration::from_millis(5));
            };
            let mut sidecar_path = sidecar.write_atomic(&recording_directory)?;
            let publish = |events: Vec<core_types::Event>,
                           sidecar: &mut experiment::ExperimentSidecar,
                           path: &mut PathBuf|
             -> Result<()> {
                for event in events {
                    if request.headless {
                        println!(
                            "{} ns | {} | {}",
                            event.timestamp_ns, event.kind, event.text
                        );
                    }
                    if runtime_commands
                        .send(Command::StructuredEvent(event.clone()))
                        .is_err()
                        && !stop.load(Ordering::Relaxed)
                    {
                        return Err("experiment: runtime command channel closed".into());
                    }
                    if event.kind == "ExperimentStart" {
                        sidecar.started_at_timestamp_ns = Some(event.timestamp_ns);
                    }
                    if matches!(event.kind.as_str(), "ExperimentEnd" | "ExperimentCancelled") {
                        sidecar.ended_at_timestamp_ns = Some(event.timestamp_ns);
                    }
                    sidecar.events.push(event);
                }
                *path = sidecar.write_atomic(&recording_directory)?;
                Ok(())
            };
            loop {
                let now_ns = runtime_status
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .now_ns;
                while let Ok(control) = control_rx.try_recv() {
                    let events = match control {
                        experiment::Control::Start => {
                            if engine.snapshot().status == experiment::Status::Ready {
                                engine.start(now_ns)?
                            } else {
                                Vec::new()
                            }
                        }
                        experiment::Control::PauseResume => {
                            if engine.snapshot().status == experiment::Status::Paused {
                                vec![engine.resume(now_ns)?]
                            } else if matches!(
                                engine.snapshot().status,
                                experiment::Status::Preparation
                                    | experiment::Status::Baseline
                                    | experiment::Status::Cue
                                    | experiment::Status::Task
                                    | experiment::Status::Rest
                            ) {
                                vec![engine.pause(now_ns)?]
                            } else {
                                Vec::new()
                            }
                        }
                        experiment::Control::Cancel(reason) => {
                            if matches!(
                                engine.snapshot().status,
                                experiment::Status::Completed
                                    | experiment::Status::Cancelled
                                    | experiment::Status::Failed
                            ) {
                                Vec::new()
                            } else {
                                sidecar.cancellation_reason = Some(reason.clone());
                                vec![engine.cancel(now_ns, &reason)?]
                            }
                        }
                    };
                    publish(events, &mut sidecar, &mut sidecar_path)?;
                }
                let events = engine.tick(now_ns);
                if !events.is_empty() {
                    publish(events, &mut sidecar, &mut sidecar_path)?;
                }
                let current = engine.snapshot();
                *snapshot_worker
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner()) = current.clone();
                if matches!(
                    current.status,
                    experiment::Status::Completed | experiment::Status::Cancelled
                ) {
                    sidecar.status = match current.status {
                        experiment::Status::Completed => "completed",
                        _ => "cancelled",
                    }
                    .into();
                    sidecar.write_atomic(&recording_directory)?;
                    return Ok(sidecar_path);
                }
                if stop.load(Ordering::Relaxed) {
                    let fatal = runtime_status
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner())
                        .fatal_error
                        .clone();
                    if !matches!(current.status, experiment::Status::Ready) {
                        let reason = fatal
                            .as_deref()
                            .unwrap_or("LEMON stopped before protocol completion");
                        sidecar.cancellation_reason = Some(reason.to_string());
                        if let Ok(event) = engine.cancel(now_ns, reason) {
                            publish(vec![event], &mut sidecar, &mut sidecar_path)?;
                        }
                    }
                    sidecar.status = if fatal.is_some() {
                        "failed"
                    } else {
                        "cancelled"
                    }
                    .into();
                    sidecar.ended_at_timestamp_ns = Some(now_ns);
                    sidecar.write_atomic(&recording_directory)?;
                    return Ok(sidecar_path);
                }
                thread::sleep(Duration::from_millis(10));
            }
        })();
        if result.is_err() {
            snapshot_worker
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .status = experiment::Status::Failed;
            stop.store(true, Ordering::Relaxed);
        }
        result
    });
    Ok((ui, worker))
}
