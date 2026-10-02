use clap::{Args, Parser, Subcommand, ValueEnum};
use core_types::{Config, Result};
use std::{
    env, fs,
    path::{Path, PathBuf},
};
use tui::settings::Settings;

#[derive(Debug, Parser)]
#[command(
    name = "lemon",
    version,
    propagate_version = true,
    about = "LEMON — Lightweight EEG Monitoring Of Neuroactivity",
    long_about = "LEMON — Lightweight EEG Monitoring Of Neuroactivity\nЛокальный мониторинг, предварительная обработка и запись ЭЭГ. Без подкоманды: demo.",
    after_help = "Примеры:\n  lemon demo -n 2 -r\n  lemon live --port /dev/cu.usbmodem123 --baud 115200\n  lemon replay recordings/session --speed 2\n  lemon report recordings/session --no-execute\n  lemon config init\n  lemon demo --headless -t 10 -r\n\nОкружение: LEMON_CONFIG, LEMON_DATA_DIR, LEMON_LOG. Справка TUI: ?"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Action>,
    /// Конфигурация эксперимента (выше LEMON_CONFIG)
    #[arg(short = 'f', long, global = true, value_name = "FILE")]
    pub config: Option<PathBuf>,
    /// Количество каналов: 1 или 2
    #[arg(short='n',long,global=true,value_parser=clap::value_parser!(u8).range(1..=2))]
    pub channels: Option<u8>,
    /// Начать запись сразу
    #[arg(short = 'r', long, global = true)]
    pub record: bool,
    /// Ограничить время работы: 0.01..86400 секунд
    #[arg(short='t',long,visible_alias="seconds",global=true,value_parser=duration)]
    pub duration: Option<f64>,
    /// Работа без терминального интерфейса
    #[arg(long, global = true)]
    pub headless: bool,
    /// Подробный журнал (debug)
    #[arg(short = 'v', long, global = true, conflicts_with = "log_level")]
    pub verbose: bool,
    /// Уровень журнала (выше LEMON_LOG)
    #[arg(long, global = true, value_enum)]
    pub log_level: Option<LogLevel>,
    /// Каталог записей (выше LEMON_DATA_DIR)
    #[arg(long, global = true)]
    pub data_dir: Option<PathBuf>,
    /// Отдельный файл пользовательских настроек вместо стандартного
    #[arg(long, global = true, value_name = "FILE")]
    pub settings: Option<PathBuf>,
}
#[derive(Debug, Subcommand)]
pub enum Action {
    /// Воспроизводимый синтетический сигнал без оборудования
    Demo,
    /// Последовательный порт: ТЕСТОВЫЙ протокол, не подтверждённый Bitronics
    Live {
        /// Один порт с 1/2 каналами или два порта по одному каналу
        #[arg(long, value_name = "PORT")]
        port: Vec<String>,
        /// Скорость порта; по умолчанию из конфигурации (115200)
        #[arg(long,value_parser=clap::value_parser!(u32).range(1..))]
        baud: Option<u32>,
    },
    /// Воспроизвести сеанс LEMON, CSV, DAT, DMOV или каталог BiTronics
    Replay {
        #[arg(value_name = "INPUT")]
        directory: PathBuf,
        /// Скорость воспроизведения: 0.01..100
        #[arg(long,value_parser=speed)]
        speed: Option<f64>,
        #[command(flatten)]
        import: ImportArgs,
    },
    /// Нормализовать внешнюю запись в metadata.json + raw.csv + events.csv
    Import {
        input: PathBuf,
        /// Новый каталог результата (существующий не перезаписывается)
        #[arg(long)]
        output: Option<PathBuf>,
        #[command(flatten)]
        options: ImportArgs,
    },
    /// Экспорт native-сеанса в long-form CSV: timestamp,channel,value
    Export {
        session: PathBuf,
        #[arg(long, default_value = "csv", value_parser = ["csv"])]
        format: String,
        #[arg(long)]
        output: PathBuf,
    },
    /// Построить потоковый технический пакет анализа без TUI
    Analyze {
        input: PathBuf,
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(long)]
        overwrite: bool,
        #[command(flatten)]
        options: ImportArgs,
    },
    /// Создать воспроизводимый Jupyter Notebook из AnalysisBundle
    #[command(args_conflicts_with_subcommands = true)]
    Report {
        #[command(subcommand)]
        command: Option<ReportAction>,
        #[arg(conflicts_with = "analysis")]
        input: Option<PathBuf>,
        #[arg(long)]
        analysis: Option<PathBuf>,
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(long, conflicts_with = "no_execute")]
        execute: bool,
        #[arg(long, conflicts_with = "execute")]
        no_execute: bool,
        #[arg(long)]
        python: Option<PathBuf>,
        /// Create a complete portable ZIP
        #[arg(long, conflicts_with_all = ["colab", "open"], group = "raw_container")]
        portable: bool,
        #[arg(long, conflicts_with_all = ["execute", "open"], group = "raw_container")]
        colab: bool,
        #[arg(long, requires = "raw_container")]
        include_raw: bool,
        #[arg(long, default_value = "25", value_parser = clap::value_parser!(u64).range(1..=1024))]
        maximum_embedded_report_mb: u64,
        #[arg(long)]
        open: bool,
        #[arg(long)]
        overwrite: bool,
        #[command(flatten)]
        options: ImportArgs,
    },
    /// Перечислить доступные последовательные порты
    Ports,
    /// Создать или проверить конфигурацию
    Config {
        #[command(subcommand)]
        command: ConfigAction,
    },
}
#[derive(Debug, Clone, Default, Args)]
pub struct ImportArgs {
    /// Подтверждённая номинальная частота, Hz; иначе оценка по времени
    #[arg(long, value_parser = sample_rate)]
    pub sample_rate: Option<f64>,
    /// Имя колонки времени или индекс с нуля
    #[arg(long)]
    pub time_column: Option<String>,
    /// Имя/индекс колонки сигнала; можно повторить для второго канала
    #[arg(long)]
    pub channel_column: Vec<String>,
    #[arg(long, default_value = "auto", value_parser = ["auto", "comma", "semicolon", "tab", "whitespace"])]
    pub delimiter: String,
    #[arg(long)]
    pub units: Option<String>,
    #[arg(long)]
    pub channel_name: Vec<String>,
}
impl ImportArgs {
    pub fn options(&self) -> storage::external::Options {
        storage::external::Options {
            sample_rate: self.sample_rate,
            time_column: self.time_column.clone(),
            channel_columns: self.channel_column.clone(),
            delimiter: self.delimiter.clone(),
            units: self.units.clone(),
            channel_names: self.channel_name.clone(),
        }
    }
}
fn sample_rate(s: &str) -> Result<f64> {
    number(s, 2., 100_000., "sample-rate")
}
#[derive(Debug, Subcommand)]
pub enum ConfigAction {
    /// Создать стандартную конфигурацию; существующий файл не перезаписывается
    Init {
        #[arg(value_name = "FILE")]
        path: Option<PathBuf>,
        #[arg(long, help = "Создать пользовательские настройки вместо эксперимента")]
        user: bool,
    },
    /// Проверить JSON и допустимость значений (включая Найквист)
    Check {
        path: PathBuf,
        #[arg(long, help = "Проверить пользовательские настройки")]
        user: bool,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
pub enum LogLevel {
    Off,
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}
impl LogLevel {
    pub fn parse(s: &str) -> Result<Self> {
        Self::from_str(s, true).map_err(|_| {
            format!("LEMON_LOG: {s:?}; допустимы off, error, warn, info, debug, trace")
        })
    }
}
fn number(s: &str, min: f64, max: f64, name: &str) -> Result<f64> {
    let n = s
        .parse::<f64>()
        .map_err(|_| format!("{name}: требуется число {min}..{max}"))?;
    if !n.is_finite() || !(min..=max).contains(&n) {
        return Err(format!("{name}: требуется конечное число {min}..{max}"));
    }
    Ok(n)
}
fn duration(s: &str) -> Result<f64> {
    number(s, 0.01, 86400., "duration")
}
fn speed(s: &str) -> Result<f64> {
    number(s, 0.01, 100., "speed")
}

#[derive(Default, Clone, Debug)]
pub struct Environment {
    pub config: Option<String>,
    pub data_dir: Option<String>,
    pub log: Option<String>,
}
impl Environment {
    pub fn read() -> Result<Self> {
        fn get(key: &str) -> Result<Option<String>> {
            match env::var(key) {
                Ok(s) => Ok(Some(s)),
                Err(env::VarError::NotPresent) => Ok(None),
                Err(_) => Err(format!("{key}: значение должно быть UTF-8")),
            }
        }
        Ok(Self {
            config: get("LEMON_CONFIG")?,
            data_dir: get("LEMON_DATA_DIR")?,
            log: get("LEMON_LOG")?,
        })
    }
    pub fn validate(&self) -> Result<()> {
        for (key, value) in [
            ("LEMON_CONFIG", &self.config),
            ("LEMON_DATA_DIR", &self.data_dir),
        ] {
            if value
                .as_ref()
                .is_some_and(|s| s.trim().is_empty() || s.contains('\0'))
            {
                return Err(format!(
                    "{key}: укажите непустой путь или удалите переменную"
                ));
            }
        }
        if let Some(level) = &self.log {
            LogLevel::parse(level)?;
        }
        Ok(())
    }
}
pub fn user_directory() -> Result<PathBuf> {
    let base = directories::BaseDirs::new()
        .ok_or("Не удалось определить каталог настроек; задайте --settings FILE")?;
    Ok(base.config_dir().join(if cfg!(target_os = "linux") {
        "lemon"
    } else {
        "LEMON"
    }))
}
pub fn settings_path(cli: &Cli) -> Result<PathBuf> {
    cli.settings
        .clone()
        .map_or_else(|| Ok(user_directory()?.join("settings.json")), Ok)
}
pub fn read_config(path: &Path) -> Result<Config> {
    let file = fs::File::open(path).map_err(|e| {
        format!(
            "Конфигурация {}: {e}; исправьте --config/LEMON_CONFIG или создайте lemon config init",
            path.display()
        )
    })?;
    if file.metadata().map_err(|e| e.to_string())?.len() > 1_048_576 {
        return Err("Конфигурация превышает 1 MiB".into());
    }
    serde_json::from_reader(file).map_err(|e| format!("JSON {}: {e}", path.display()))
}
pub struct Resolved {
    pub experiment: Config,
    pub settings: Settings,
    pub log: LogLevel,
    pub experiment_directory: String,
    pub experiment_session_name: String,
}
pub fn resolve(
    cli: &Cli,
    environment: &Environment,
    user: Option<Settings>,
    default_config: Option<&Path>,
) -> Result<Resolved> {
    environment.validate()?;
    let config_path = cli
        .config
        .clone()
        .or_else(|| environment.config.as_ref().map(PathBuf::from))
        .or_else(|| {
            default_config
                .filter(|p| p.is_file())
                .map(Path::to_path_buf)
        });
    let mut experiment = if let Some(path) = &config_path {
        read_config(path)?
    } else {
        Config::default()
    };
    let mut settings = user.unwrap_or_else(|| {
        let mut s = Settings::default();
        s.general.fps = experiment.tui_hz;
        s
    });
    if let Some(ch) = cli.channels {
        experiment.source.channels = ch as usize;
    }
    match &cli.command {
        None | Some(Action::Demo) => experiment.source.mode = "synthetic".into(),
        Some(Action::Live { port, baud }) => {
            experiment.source.mode = "serial-test".into();
            if !port.is_empty() {
                experiment.source.ports = port.clone();
            }
            if let Some(baud) = baud {
                experiment.source.baud_rate = *baud;
            }
            if cli.channels.is_none() && config_path.is_none() {
                experiment.source.channels = experiment.source.ports.len().max(1);
            }
            if experiment.source.ports.len() == 2 {
                if cli.channels == Some(1) {
                    return Err("Два порта требуют --channels 2; удалите -n 1".into());
                }
                experiment.source.channels = 2;
            }
        }
        Some(Action::Replay {
            directory, speed, ..
        }) => {
            let metadata = storage::read_metadata(directory)?;
            if cli
                .channels
                .is_some_and(|c| c as usize != metadata.config.source.channels)
            {
                return Err("Число каналов replay определяется записью; удалите --channels".into());
            }
            experiment.source.mode = "replay".into();
            experiment.source.replay_path = directory.to_string_lossy().into_owned();
            experiment.source.channels = metadata.config.source.channels;
            experiment.source.sample_rate_hz = metadata.config.source.sample_rate_hz;
            experiment.source.units = metadata.units;
            if config_path.is_none() {
                experiment.processing = metadata.config.processing;
                experiment.session = metadata.config.session;
            }
            if let Some(speed) = speed {
                experiment.source.replay_speed = *speed;
            }
        }
        _ => {}
    }
    let experiment_directory = experiment.recording_dir.clone();
    let experiment_session_name = experiment.session.name.clone();
    let data = cli
        .data_dir
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned())
        .or_else(|| environment.data_dir.clone())
        .or_else(|| settings.storage.directory.clone())
        .unwrap_or_else(|| experiment.recording_dir.clone());
    if data.trim().is_empty() || data.contains('\0') {
        return Err("Каталог записей пуст; задайте --data-dir DIR".into());
    }
    experiment.recording_dir = data;
    if let Some(name) = &settings.storage.session_name {
        experiment.session.name = name.clone();
    }
    if settings.waveform.channels > experiment.source.channels {
        settings.waveform.channels = 0;
    }
    settings.validate()?;
    experiment.validate()?;
    let log = if cli.verbose {
        LogLevel::Debug
    } else {
        cli.log_level
            .or(environment
                .log
                .as_deref()
                .map(LogLevel::parse)
                .transpose()?)
            .unwrap_or(LogLevel::Warn)
    };
    Ok(Resolved {
        experiment,
        settings,
        log,
        experiment_directory,
        experiment_session_name,
    })
}
pub fn legacy_hint(args: &[String]) -> Option<&'static str> {
    for arg in args {
        match arg.split('=').next().unwrap_or("") {
            "--source" => {
                return Some(
                    "Используйте lemon demo или lemon live --port PORT; прежний --source удалён.",
                )
            }
            "--replay" => return Some("Используйте lemon replay SESSION_DIRECTORY [--speed 2]."),
            "--list-ports" => return Some("Используйте lemon ports."),
            "--write-config" => return Some("Используйте lemon config init FILE."),
            _ => {}
        }
    }
    if args
        .iter()
        .any(|a| a == "--port" || a.starts_with("--port="))
        && !args.iter().any(|a| a == "live")
    {
        return Some("Используйте lemon live --port PORT [--baud 115200].");
    }
    None
}

#[derive(Debug, Clone, Subcommand)]
pub enum ReportAction {
    /// Create the managed Python environment and install report dependencies
    Setup {
        #[arg(long)]
        recreate: bool,
        #[arg(long)]
        python: Option<PathBuf>,
    },
    /// Diagnose the managed Python environment without installing packages
    Doctor,
}
