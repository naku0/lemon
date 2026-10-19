use analysis::{analyze, AnalysisBundle, AnalysisInput, AnalysisOptions};
use app::{
    cli::{self, Action, Cli, Environment, LogLevel},
    Runtime,
};
use clap::Parser;
use core_types::Result;
use report::{render_notebook, ExecutionMode, ReportOptions};
use std::{
    io::{IsTerminal, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};
use tui::{settings::Settings, UiOptions};

mod config_commands;
mod experiment_controller;
mod logging;
mod plugin_commands;
mod report_support;

use logging::Logger;
use report_support::TempAnalysis;

fn main() {
    if let Err(e) = run() {
        eprintln!("LEMON: {e}");
        std::process::exit(1);
    }
}
fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if let Some(hint) = cli::legacy_hint(&args) {
        return Err(hint.into());
    }
    let mut cli = Cli::parse();
    cli.normalize_run();
    let experiment_request = experiment_controller::Request::from_cli(&mut cli)?;
    if matches!(
        cli.command,
        Some(Action::Plugins { .. }) | Some(Action::Plugin { .. })
    ) {
        return plugin_commands::run(&cli);
    }
    if let Some(Action::Report {
        command: Some(command),
        ..
    }) = &cli.command
    {
        let result = match command {
            cli::ReportAction::Setup { recreate, python } => {
                report::environment::setup(*recreate, python.as_deref())?
            }
            cli::ReportAction::Doctor => report::environment::doctor()?,
        };
        println!(
            "{}",
            serde_json::to_string_pretty(&result).map_err(|e| e.to_string())?
        );
        return Ok(());
    }
    let environment = Environment::read()?;
    environment.validate()?;
    if let Some(Action::Probe {
        port,
        baud,
        output,
        overwrite,
        startup_delay_ms,
        max_bytes,
    }) = &cli.command
    {
        let seconds = cli.duration.unwrap_or(10.);
        if seconds > 3600. {
            return Err("probe: --duration must not exceed 3600 seconds".into());
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let signal = cancelled.clone();
        ctrlc::set_handler(move || signal.store(true, Ordering::Relaxed))
            .map_err(|e| format!("probe/Ctrl+C: {e}"))?;
        let result = source::probe(
            &source::ProbeOptions {
                port: port.clone(),
                baud: *baud,
                duration: Duration::from_secs_f64(seconds),
                startup_delay: Duration::from_millis(*startup_delay_ms),
                output: output.clone(),
                overwrite: *overwrite,
                max_bytes: *max_bytes,
            },
            &cancelled,
        )?;
        println!("Saved: {}\nStartup delay: {:.3} s; discarded startup bytes: {}\nMeasurement duration: {:.3} s (requested {:.3})\nCaptured bytes: {} ({:.1} bytes/s); LF: {}\nASCII: {}\nHEX: {}", output.display(), result.startup_seconds, result.startup_bytes, result.seconds, seconds, result.bytes, result.bytes as f64 / result.seconds.max(1e-9), result.newlines, result.ascii_preview, result.hex_preview);
        if let Some(c) = &result.candidate {
            println!("Candidate tagged-u8 frame: {}\nFrame size: {} bytes\nComplete frames: {}\nDiscarded prefix/suffix bytes: {}/{}; internal discarded: {}\nEstimated full frame rate: {:.3} frames/s (diagnostic only; nominal rate unchanged)",c.tags.join(","),c.frame_size,c.complete_frames,c.prefix_bytes,c.suffix_bytes,c.internal_discarded_bytes,c.complete_frames as f64 / result.seconds.max(1e-9));
            for counter in &c.counters {
                println!("Candidate sequence tag: {}; sequence discontinuities: {}; minimum missing (modulo 256): {}; repeated: {}",counter.tag,counter.discontinuities,counter.minimum_missing,counter.duplicates);
            }
            println!("Suggested signal tags: must be selected explicitly. Configuration template (replace placeholders):\n{}", serde_json::json!({"source": {"mode":"serial", "serial_protocol":"tagged-u8", "serial_frame_tags":c.tags,"serial_channel_tags":["SELECT_SIGNAL_TAG"],"serial_sequence_tag":if c.counters.len()==1 {Some(c.counters[0].tag.clone())}else{None},"channels":1,"sample_rate_hz":"SET_NOMINAL_RATE","units":"adc_counts_u8"}}));
        }
        if result.limit_reached {
            println!("Capture stopped at --max-bytes limit.");
        }
        println!(
            "{}",
            if result.suggests_tagged_u8() {
                "Repeated structure suggests --protocol tagged-u8; this is not proof of framing or hardware sample rate."
            } else {
                "Insufficient consistent framing to recommend a protocol."
            }
        );
        return Ok(());
    }
    if matches!(
        cli.command,
        Some(Action::Ports) | Some(Action::Config { .. })
    ) {
        if cli.record
            || cli.channels.is_some()
            || cli.duration.is_some()
            || cli.headless
            || cli.data_dir.is_some()
        {
            return Err("Параметры записи/каналов/длительности применимы только к demo, live и replay; удалите их из команды".into());
        }
        if let Some(Action::Ports) = cli.command {
            let ports = source::describe_ports()?;
            if ports.is_empty() {
                println!("No serial ports detected");
            }
            for port in ports {
                println!("{port}");
            }
            return Ok(());
        }
        if let Some(Action::Config { command }) = &cli.command {
            return config_commands::run(command, &cli);
        }
    }
    let settings_path = cli::settings_path(&cli)?;
    let default_config = settings_path.parent().map(|p| p.join("config.json"));
    let user_settings = Settings::load(&settings_path)?;
    // External replay is normalized in an owned temporary directory, then uses the existing Replay.
    let mut prepared = None;
    if matches!(
        cli.command,
        Some(Action::Import { .. })
            | Some(Action::Export { .. })
            | Some(Action::Analyze { .. })
            | Some(Action::Report { .. })
            | Some(Action::Replay { .. })
            | Some(Action::Run {
                command: cli::RunAction::Study { .. }
            })
    ) {
        let config_path = cli
            .config
            .clone()
            .or_else(|| environment.config.as_ref().map(std::path::PathBuf::from))
            .or_else(|| default_config.clone().filter(|p| p.is_file()));
        let base = config_path
            .as_deref()
            .map(cli::read_config)
            .transpose()?
            .unwrap_or_default();
        match &cli.command {
            Some(Action::Run {
                command:
                    cli::RunAction::Study {
                        manifest,
                        output,
                        overwrite,
                    },
            }) => {
                if cli.record || cli.headless || cli.duration.is_some() || cli.channels.is_some() {
                    return Err(
                        "study: --record/--headless/--duration/--channels are not applicable"
                            .into(),
                    );
                }
                let result = study::run(study::StudyRequest {
                    manifest: manifest.clone(),
                    output: output.clone(),
                    config: base.clone(),
                    overwrite: *overwrite,
                })?;
                println!("Study completed: {}", result.directory.display());
                println!(
                    "Mu condition config: {}",
                    result
                        .directory
                        .join("mu-condition-analysis.json")
                        .display()
                );
                return Ok(());
            }
            Some(Action::Analyze {
                input,
                output,
                overwrite,
                options,
            }) => {
                if cli.record || cli.headless || cli.duration.is_some() || cli.channels.is_some() {
                    return Err(
                        "analyze: --record/--headless/--duration/--channels are not applicable"
                            .into(),
                    );
                }
                let bundle = analyze(
                    AnalysisInput {
                        path: input.clone(),
                    },
                    AnalysisOptions {
                        output: output.clone(),
                        config: config_path.as_deref().map(cli::read_config).transpose()?,
                        sample_rate: options.sample_rate,
                        units: options.units.clone(),
                        overwrite: *overwrite,
                        import: options.options(),
                    },
                )?;
                println!("Analysis bundle: {}", bundle.directory.display());
                return Ok(());
            }
            Some(Action::Report {
                command: _,
                colab,
                include_raw,
                maximum_embedded_report_mb,
                input,
                analysis,
                output,
                execute,
                no_execute,
                portable,
                python,
                open,
                overwrite,
                options,
            }) => {
                if cli.record || cli.headless || cli.duration.is_some() || cli.channels.is_some() {
                    return Err(
                        "report: --record/--headless/--duration/--channels are not applicable"
                            .into(),
                    );
                }
                if analysis.is_some() && input.is_some() {
                    return Err("report: укажите INPUT или --analysis, но не оба".into());
                }
                let report_output = output.clone().unwrap_or_else(|| {
                    PathBuf::from(format!(
                        "report-{}",
                        input
                            .as_ref()
                            .and_then(|p| p.file_name())
                            .and_then(|p| p.to_str())
                            .unwrap_or("analysis")
                    ))
                });
                let mut report_temp = None;
                let bundle = if let Some(path) = analysis {
                    AnalysisBundle::open(path)?
                } else if let Some(path) = input {
                    let temp = TempAnalysis::new()?;
                    let temp_path = temp.path().join("analysis");
                    report_temp = Some(temp);
                    analyze(
                        AnalysisInput { path: path.clone() },
                        AnalysisOptions {
                            output: Some(temp_path),
                            config: config_path.as_deref().map(cli::read_config).transpose()?,
                            sample_rate: options.sample_rate,
                            units: options.units.clone(),
                            overwrite: true,
                            import: options.options(),
                        },
                    )?
                } else {
                    return Err("report: укажите INPUT или --analysis DIRECTORY".into());
                };
                if *include_raw && !*colab && !*portable {
                    return Err("report: --include-raw requires --colab or --portable".into());
                }
                let mut declined_setup = false;
                if !*colab
                    && !*no_execute
                    && python.is_none()
                    && !report::environment::managed_python()?.is_file()
                    && std::io::stdin().is_terminal()
                    && std::io::stdout().is_terminal()
                {
                    eprint!("LEMON report environment is not configured.\nCreate it now? [Y/n] ");
                    std::io::stderr().flush().map_err(|e| e.to_string())?;
                    let mut answer = String::new();
                    let bytes = std::io::stdin()
                        .read_line(&mut answer)
                        .map_err(|e| e.to_string())?;
                    if bytes > 0
                        && matches!(answer.trim().to_lowercase().as_str(), "y" | "yes" | "")
                    {
                        report::environment::setup(false, None)?;
                    } else {
                        declined_setup = true;
                    }
                }
                let mode = if *colab || declined_setup {
                    ExecutionMode::NoExecute
                } else if *execute {
                    ExecutionMode::Execute
                } else if *no_execute {
                    ExecutionMode::NoExecute
                } else {
                    ExecutionMode::Auto
                };
                let artifacts = render_notebook(
                    &bundle,
                    ReportOptions {
                        output: Some(report_output),
                        execution: mode,
                        python: python.clone(),
                        overwrite: *overwrite,
                        open: *open,
                        portable: *portable,
                        colab: *colab,
                        include_raw: *include_raw,
                        maximum_embedded_report_mb: *maximum_embedded_report_mb,
                    },
                )?;
                drop(report_temp);
                println!(
                    "{}: {}",
                    if artifacts.execution_status == "executed" {
                        "Report completed"
                    } else {
                        "Report generated (not executed)"
                    },
                    artifacts.notebook.display()
                );
                if let Some(archive) = &artifacts.archive {
                    println!("Portable report: {}", archive.display());
                }
                println!("Report status: {}", artifacts.execution_status);
                if let Some(warning) = artifacts.warning {
                    eprintln!("LEMON: {warning}");
                }
                if *open {
                    report_support::open(&artifacts.notebook);
                }
                return Ok(());
            }
            Some(Action::Import {
                input,
                output,
                options,
            }) => {
                if cli.record || cli.headless || cli.duration.is_some() || cli.channels.is_some() {
                    return Err("import: use --channel-column/--channel-name; --record/--headless/--duration/--channels apply to playback only".into());
                }
                let parent = cli
                    .data_dir
                    .clone()
                    .or_else(|| environment.data_dir.as_ref().map(std::path::PathBuf::from))
                    .or_else(|| {
                        user_settings.as_ref().and_then(|s| {
                            s.storage.directory.as_ref().map(std::path::PathBuf::from)
                        })
                    })
                    .unwrap_or_else(|| base.recording_dir.clone().into());
                let path = storage::external::import(
                    input,
                    output.as_deref(),
                    &parent,
                    &options.options(),
                    &base,
                )?;
                print_import(&path)?;
                println!("Imported session: {}", path.display());
                return Ok(());
            }
            Some(Action::Export {
                session, output, ..
            }) => {
                if cli.record || cli.headless || cli.duration.is_some() || cli.channels.is_some() {
                    return Err("export: recording/playback options are not applicable".into());
                }
                storage::external::export_csv(session, output)?;
                println!(
                    "Exported long-form CSV (timestamp in seconds): {}",
                    output.display()
                );
                return Ok(());
            }
            Some(Action::Replay {
                directory, import, ..
            }) => {
                let p = storage::external::prepare(directory, &import.options(), &base)?;
                print_import(&p.path)?;
                prepared = Some(p);
            }
            _ => {}
        }
    }
    if let (Some(Action::Replay { directory, .. }), Some(p)) = (&mut cli.command, &prepared) {
        *directory = p.path.clone();
    }
    let resolved = cli::resolve(&cli, &environment, user_settings, default_config.as_deref())?;
    let config = resolved.experiment;
    let mut settings = resolved.settings;
    let mut logger = Logger::new(resolved.log, cli.headless, &settings_path)?;
    logger.write(
        LogLevel::Info,
        &format!(
            "start mode={} channels={} fs={} Hz",
            config.source.mode, config.source.channels, config.source.sample_rate_hz
        ),
    );
    logger.write(
        LogLevel::Debug,
        &format!(
            "experiment={:?}; data_dir={}; settings={}; HP={} LP={} notch={} FFT={}/{}",
            cli.config,
            config.recording_dir,
            settings_path.display(),
            config.processing.high_pass_hz,
            config.processing.low_pass_hz,
            config.processing.notch,
            config.processing.window_samples,
            config.processing.hop_samples
        ),
    );
    if matches!(config.source.mode.as_str(), "serial" | "serial-test") {
        let warning="LIVE: nominal sample rate; reconstructed host-side timestamps, no hardware synchronization. Close Arduino Serial Monitor; opening the port may reset UNO.";
        eprintln!("{warning}");
        logger.write(LogLevel::Warn, warning);
    }
    if cli.record
        && !settings.storage.create_directory
        && !Path::new(&config.recording_dir).is_dir()
    {
        return Err("Каталог записей не существует; создайте его или включите Storage / Auto create directory".into());
    }
    let source = source::open(&config.source)?;
    let mut runtime = Runtime::start(config.clone(), source, cli.record)?;
    let (experiment_ui, experiment_worker) = if let Some(request) = experiment_request {
        let (ui, worker) = experiment_controller::start(
            request,
            runtime.status.clone(),
            runtime.commands.clone(),
            runtime.stop.clone(),
        )?;
        (Some(ui), Some(worker))
    } else {
        (None, None)
    };
    let experiment_control = experiment_ui
        .as_ref()
        .map(|experiment| experiment.commands.clone());
    let experiment_snapshot = experiment_ui
        .as_ref()
        .map(|experiment| experiment.snapshot.clone());
    let interrupt = Arc::new(AtomicBool::new(false));
    let signal = if cli.headless {
        runtime.stop.clone()
    } else {
        interrupt.clone()
    };
    if let Err(e) = ctrlc::set_handler(move || signal.store(true, Ordering::Relaxed)) {
        runtime.shutdown()?;
        return Err(format!("Ctrl+C handler: {e}"));
    }
    // A bounded snapshot observer logs diagnostics without writing over the TUI.
    let log_stop = Arc::new(AtomicBool::new(false));
    let logging_stop = log_stop.clone();
    let logging_status = runtime.status.clone();
    let logging = thread::spawn(move || {
        let mut last = None;
        let mut last_count = 0;
        while !logging_stop.load(Ordering::Relaxed) {
            let s = logging_status
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone();
            if let Some(event) = s.events.iter().last() {
                let key = (event.timestamp_ns, event.kind.clone(), event.text.clone());
                if last.as_ref() != Some(&key) {
                    logger.write(
                        if event.kind.contains("Error") {
                            LogLevel::Error
                        } else {
                            LogLevel::Info
                        },
                        &format!("{}: {}", event.kind, event.text),
                    );
                    last = Some(key);
                }
            }
            if s.processed_blocks / 50 != last_count {
                last_count = s.processed_blocks / 50;
                logger.write(
                    LogLevel::Debug,
                    &format!(
                        "blocks={} samples={} lost={} errors={}",
                        s.processed_blocks, s.received_samples, s.lost_samples, s.errors
                    ),
                );
            }
            thread::sleep(Duration::from_millis(50));
        }
        logger
    });
    let started = Instant::now();
    // Serial reset grace is setup time, not part of the requested recording duration.
    let duration_limit = cli.duration.map(|d| {
        d + if matches!(config.source.mode.as_str(), "serial" | "serial-test") {
            config.source.serial_startup_delay_ms as f64 / 1000.
        } else {
            0.
        }
    });
    let result = if cli.headless {
        loop {
            let experiment_finished = experiment_ui.as_ref().is_some_and(|ui| {
                matches!(
                    ui.snapshot
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner())
                        .status,
                    experiment::Status::Completed
                        | experiment::Status::Cancelled
                        | experiment::Status::Failed
                )
            });
            if runtime
                .status
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .finished
                || runtime.stop.load(Ordering::Relaxed)
                || duration_limit.is_some_and(|d| started.elapsed().as_secs_f64() >= d)
                || experiment_finished
            {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    } else {
        if let Some(duration) = duration_limit {
            let stop = runtime.stop.clone();
            thread::spawn(move || {
                let start = Instant::now();
                while !stop.load(Ordering::Relaxed) && start.elapsed().as_secs_f64() < duration {
                    thread::sleep(Duration::from_millis(10));
                }
                stop.store(true, Ordering::Relaxed);
            });
        }
        let ports = source::list_ports().unwrap_or_else(|e| vec![e]);
        match runtime.subscription.take() {
            Some(subscription) => match tui::run(
                &config,
                &ports,
                subscription,
                runtime.commands.clone(),
                runtime.status.clone(),
                runtime.stop.clone(),
                UiOptions {
                    settings: settings.clone(),
                    settings_path,
                    interrupt,
                    data_directory_locked: cli.data_dir.is_some() || environment.data_dir.is_some(),
                    experiment_directory: resolved.experiment_directory,
                    experiment_session_name: resolved.experiment_session_name,
                    plugin_registry_path: plugins::default_registry_path()
                        .map_err(|e| e.to_string())?,
                    experiment: experiment_ui,
                },
            ) {
                Ok(updated) => {
                    settings = updated;
                    Ok(())
                }
                Err(e) => Err(e),
            },
            None => Err("TUI subscription missing".into()),
        }
    };
    if experiment_snapshot.as_ref().is_some_and(|snapshot| {
        !matches!(
            snapshot
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .status,
            experiment::Status::Completed
                | experiment::Status::Cancelled
                | experiment::Status::Failed
        )
    }) {
        if let Some(control) = &experiment_control {
            let _ = control.send(experiment::Control::Cancel(
                "LEMON stopped before protocol completion".into(),
            ));
        }
    }
    let experiment_result = experiment_worker
        .map(|worker| {
            worker
                .join()
                .map_err(|_| "Experiment worker panicked".to_string())
        })
        .transpose()?
        .transpose()?;
    let final_status = runtime.shutdown();
    log_stop.store(true, Ordering::Relaxed);
    let mut logger = logging.join().map_err(|_| "Logger thread failed")?;
    let final_status = final_status?;
    result?;
    if let Some(path) = experiment_result {
        println!("Experiment sidecar: {}", path.display());
    }
    let totals = format!(
        "blocks={} samples={} lost={} errors={} outliers={} consumer_blocks={} consumer_dropped={}",
        final_status.processed_blocks,
        final_status.received_samples,
        final_status.lost_samples,
        final_status.errors,
        final_status.outliers,
        final_status.consumer_blocks,
        final_status.consumer_dropped
    );
    println!("{totals}");
    logger.write(LogLevel::Info, &totals);
    if settings.storage.show_summary {
        if let Some(s) = &final_status.recording_summary {
            println!(
                "Recording complete\nDuration: {:.3} s\nSamples: {}\nLost: {}\nErrors: {}\nraw: {}",
                s.duration_ns as f64 / 1e9,
                s.samples,
                s.lost,
                s.errors,
                s.path
            );
        }
    }
    if let Some(e) = final_status.fatal_error {
        logger.write(LogLevel::Error, &e);
        return Err(e);
    }
    Ok(())
}

fn print_import(path: &Path) -> Result<()> {
    if let Some(provenance) = storage::read_metadata(path)?.provenance {
        for file in provenance.files_used {
            eprintln!("Import file: {file}");
        }
        for file in provenance.duplicate_files_ignored {
            eprintln!("Exact duplicate ignored: {file}");
        }
        for warning in provenance.import_warnings {
            eprintln!("Import warning: {warning}");
        }
    }
    Ok(())
}
