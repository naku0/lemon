use clap::Parser;
use eeg_app::{
    cli::{self, Action, Cli, ConfigAction, Environment, LogLevel},
    Runtime,
};
use eeg_core::{Config, Result};
use eeg_tui::{settings::Settings, UiOptions};
use std::{
    fs,
    io::Write,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

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
    let cli = Cli::parse();
    let environment = Environment::read()?;
    environment.validate()?;
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
            let ports = eeg_source::list_ports()?;
            if ports.is_empty() {
                println!("No serial ports detected");
            }
            for port in ports {
                println!("{port}");
            }
            return Ok(());
        }
        if let Some(Action::Config { command }) = &cli.command {
            return config_command(command, &cli);
        }
    }
    let settings_path = cli::settings_path(&cli)?;
    let default_config = settings_path.parent().map(|p| p.join("config.json"));
    let resolved = cli::resolve(
        &cli,
        &environment,
        Settings::load(&settings_path)?,
        default_config.as_deref(),
    )?;
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
    if config.source.mode == "serial-test" {
        let warning="LIVE uses serial-test, a TEST text protocol. It is NOT a confirmed Bitronics protocol.";
        eprintln!("{warning}");
        logger.write(LogLevel::Warn, warning);
    }
    if cli.record
        && !settings.storage.create_directory
        && !Path::new(&config.recording_dir).is_dir()
    {
        return Err("Каталог записей не существует; создайте его или включите Storage / Auto create directory".into());
    }
    let source = eeg_source::open(&config.source)?;
    let mut runtime = Runtime::start(config.clone(), source, cli.record)?;
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
    let result = if cli.headless {
        loop {
            if runtime
                .status
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .finished
                || runtime.stop.load(Ordering::Relaxed)
                || cli
                    .duration
                    .is_some_and(|d| started.elapsed().as_secs_f64() >= d)
            {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    } else {
        if let Some(duration) = cli.duration {
            let stop = runtime.stop.clone();
            thread::spawn(move || {
                let start = Instant::now();
                while !stop.load(Ordering::Relaxed) && start.elapsed().as_secs_f64() < duration {
                    thread::sleep(Duration::from_millis(10));
                }
                stop.store(true, Ordering::Relaxed);
            });
        }
        let ports = eeg_source::list_ports().unwrap_or_else(|e| vec![e]);
        match runtime.subscription.take() {
            Some(subscription) => match eeg_tui::run(
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
    let final_status = runtime.shutdown();
    log_stop.store(true, Ordering::Relaxed);
    let mut logger = logging.join().map_err(|_| "Logger thread failed")?;
    let final_status = final_status?;
    result?;
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
fn config_command(command: &ConfigAction, cli: &Cli) -> Result<()> {
    match command {
        ConfigAction::Init { path, user } => {
            let path = match path {
                Some(path) => path.clone(),
                None if *user => cli::settings_path(cli)?,
                None => cli::user_directory()?.join("config.json"),
            };
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() {
                    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
            }
            let file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map_err(|e| {
                    format!(
                        "{}: {e}; выберите новый путь, существующие файлы не перезаписываются",
                        path.display()
                    )
                })?;
            if *user {
                serde_json::to_writer_pretty(&file, &Settings::default())
            } else {
                serde_json::to_writer_pretty(&file, &Config::default())
            }
            .map_err(|e| e.to_string())?;
            file.sync_all().map_err(|e| e.to_string())?;
            println!("Created {}", path.display());
        }
        ConfigAction::Check { path, user } => {
            if *user {
                Settings::load(path)?
                    .ok_or_else(|| format!("Settings {} not found", path.display()))?;
            } else {
                cli::read_config(path)?.validate()?;
            }
            println!("OK: {}", path.display());
        }
    }
    Ok(())
}
struct Logger {
    level: LogLevel,
    file: Option<fs::File>,
    headless: bool,
}
impl Logger {
    fn new(level: LogLevel, headless: bool, settings_path: &Path) -> Result<Self> {
        let file = if !headless && level != LogLevel::Off {
            let parent = settings_path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            fs::create_dir_all(parent).map_err(|e| format!("Log directory: {e}"))?;
            Some(
                fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(parent.join("lemon.log"))
                    .map_err(|e| format!("Log file: {e}"))?,
            )
        } else {
            None
        };
        Ok(Self {
            level,
            file,
            headless,
        })
    }
    fn write(&mut self, level: LogLevel, text: &str) {
        if self.level == LogLevel::Off || level > self.level {
            return;
        }
        if self.headless {
            eprintln!("[{level:?}] {text}");
        } else if let Some(file) = &mut self.file {
            let _ = writeln!(file, "[{level:?}] {text}");
        }
    }
}
