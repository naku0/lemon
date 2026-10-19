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
    long_about = "LEMON — локальная запись, воспроизведение и технический анализ ЭЭГ.\n\n`lemon run …` запускает рабочий режим. `lemon plugins …` управляет реестром плагинов, `lemon config …` — настройками. `import`, `export`, `ports` и `probe` — отдельные служебные утилиты.",
    after_help = "Примеры:\n  lemon run demo --record\n  lemon run live --port /dev/cu.usbmodem123 --baud 115200\n  lemon run replay recordings/session --speed 2\n  lemon run analyze recording.dmov --output reports/analysis\n  lemon run study --manifest study.json --output reports/study\n  lemon run report create reports/analysis --no-execute\n  lemon config init\n\nОкружение: LEMON_CONFIG, LEMON_DATA_DIR, LEMON_LOG. Справка TUI: ?"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Action>,
    /// Конфигурация обработки; для `run plugin` — JSON-конфигурация плагина
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
    /// Запустить live, demo, replay, анализ, study, эксперимент, плагин или отчёт
    Run {
        #[command(subcommand)]
        command: RunAction,
    },
    /// Воспроизводимый синтетический сигнал без оборудования
    #[command(hide = true)]
    Demo,
    /// Последовательный порт с протоколом csv или tagged-u8
    #[command(hide = true)]
    Live {
        /// Один порт с 1/2 каналами или два порта по одному каналу
        #[arg(long, value_name = "PORT")]
        port: Vec<String>,
        /// Скорость порта; по умолчанию из конфигурации (115200)
        #[arg(long,value_parser=clap::value_parser!(u32).range(1..))]
        baud: Option<u32>,
        #[arg(long, value_parser = ["csv", "tagged-u8"])]
        protocol: Option<String>,
        /// Номинальная частота кадров, не измеренная устройством
        #[arg(long)]
        sample_rate: Option<f64>,
        #[arg(long)]
        units: Option<String>,
        /// Дополнительное время ожидания после открытия/reset UNO
        #[arg(long)]
        startup_delay_ms: Option<u64>,
    },
    /// Сохранить ограниченный дамп Serial без DSP и без отправки команд
    Probe {
        #[arg(long)]
        port: String,
        #[arg(long, default_value_t = 115200, value_parser=clap::value_parser!(u32).range(1..))]
        baud: u32,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        overwrite: bool,
        #[arg(long, default_value_t = 2000, value_parser=clap::value_parser!(u64).range(0..=60000))]
        startup_delay_ms: u64,
        #[arg(long, default_value_t = 67108864, value_parser=clap::value_parser!(u64).range(1..=1073741824))]
        max_bytes: u64,
    },
    /// Воспроизвести сеанс LEMON, CSV, DAT, DMOV или каталог BiTronics
    #[command(hide = true)]
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
    #[command(hide = true)]
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
    #[command(args_conflicts_with_subcommands = true, hide = true)]
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
    /// Управлять реестром локальных аналитических плагинов
    Plugins {
        #[command(subcommand)]
        command: PluginsAction,
    },
    /// Выполнить аналитический плагин в отдельном процессе
    #[command(hide = true)]
    Plugin {
        #[command(subcommand)]
        command: PluginAction,
    },
    /// Выполнить протокол записи со структурированными markers
    #[command(hide = true)]
    Experiment {
        #[command(subcommand)]
        command: ExperimentAction,
    },
    /// Перечислить доступные последовательные порты
    Ports,
    /// Создать или проверить конфигурацию
    Config {
        #[command(subcommand)]
        command: ConfigAction,
    },
}

/// Рабочие режимы LEMON.
#[derive(Debug, Subcommand)]
pub enum RunAction {
    /// Принимать EEG из последовательного порта
    Live {
        /// Один порт с 1/2 каналами или два порта по одному каналу
        #[arg(long, value_name = "PORT")]
        port: Vec<String>,
        /// Скорость порта; по умолчанию из конфигурации
        #[arg(long,value_parser=clap::value_parser!(u32).range(1..))]
        baud: Option<u32>,
        #[arg(long, value_parser = ["csv", "tagged-u8"])]
        protocol: Option<String>,
        #[arg(long)]
        sample_rate: Option<f64>,
        #[arg(long)]
        units: Option<String>,
        #[arg(long)]
        startup_delay_ms: Option<u64>,
    },
    /// Запустить воспроизводимый синтетический источник
    Demo,
    /// Воспроизвести сеанс или внешнюю запись
    Replay {
        #[arg(value_name = "INPUT")]
        directory: PathBuf,
        #[arg(long, value_parser = speed)]
        speed: Option<f64>,
        #[command(flatten)]
        import: ImportArgs,
    },
    /// Создать технический AnalysisBundle одной записи
    Analyze {
        #[arg(value_name = "INPUT")]
        input: PathBuf,
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(long)]
        overwrite: bool,
        #[command(flatten)]
        options: ImportArgs,
    },
    /// Пакетно подготовить AnalysisBundle для фоновой и condition-записей
    Study {
        /// JSON-manifest с явными ролями и относительными путями источников
        #[arg(long, value_name = "STUDY_JSON")]
        manifest: PathBuf,
        /// Новый каталог study; существующий заменяется только с --overwrite
        #[arg(long, value_name = "OUTPUT_DIR")]
        output: PathBuf,
        #[arg(long)]
        overwrite: bool,
    },
    /// Записать EEG по протоколу фаз и trials
    Experiment {
        #[arg(long)]
        protocol: PathBuf,
        #[arg(long, conflicts_with_all = ["port", "replay"])]
        demo: bool,
        #[arg(long, value_name = "PORT", conflicts_with = "replay")]
        port: Vec<String>,
        #[arg(long, value_name = "SESSION", conflicts_with_all = ["demo", "port"])]
        replay: Option<PathBuf>,
        #[arg(long, value_parser=clap::value_parser!(u32).range(1..))]
        baud: Option<u32>,
        #[arg(long, default_value_t = 1.0)]
        time_scale: f64,
    },
    /// Выполнить локальный аналитический плагин
    Plugin {
        plugin_id_or_dir: String,
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        overwrite: bool,
        #[arg(long, default_value_t = 300, value_parser=clap::value_parser!(u64).range(1..=86400))]
        timeout: u64,
    },
    /// Создать отчёт или управлять его Python-окружением
    Report {
        #[command(subcommand)]
        command: RunReportAction,
    },
}

#[derive(Debug, Subcommand)]
pub enum RunReportAction {
    /// Создать local, portable или Colab notebook
    Create {
        #[arg(value_name = "INPUT", conflicts_with = "analysis")]
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
        options: Box<ImportArgs>,
    },
    /// Создать или восстановить управляемое Python-окружение
    Setup {
        #[arg(long)]
        recreate: bool,
        #[arg(long)]
        python: Option<PathBuf>,
    },
    /// Проверить Python, venv, kernel и зависимости отчётов
    Healthcheck,
}

impl Cli {
    /// Collapse the canonical `run …` grammar into the legacy-neutral request
    /// variants consumed by configuration resolution and command dispatch.
    /// Legacy commands already use those variants, so both syntaxes cross the
    /// same seam and cannot acquire different runtime behaviour.
    pub fn normalize_run(&mut self) {
        let command = match self.command.take() {
            Some(Action::Run { command }) => command,
            legacy => {
                self.command = legacy;
                return;
            }
        };
        self.command = Some(match command {
            RunAction::Demo => Action::Demo,
            RunAction::Live {
                port,
                baud,
                protocol,
                sample_rate,
                units,
                startup_delay_ms,
            } => Action::Live {
                port,
                baud,
                protocol,
                sample_rate,
                units,
                startup_delay_ms,
            },
            RunAction::Replay {
                directory,
                speed,
                import,
            } => Action::Replay {
                directory,
                speed,
                import,
            },
            RunAction::Analyze {
                input,
                output,
                overwrite,
                options,
            } => Action::Analyze {
                input,
                output,
                overwrite,
                options,
            },
            RunAction::Study {
                manifest,
                output,
                overwrite,
            } => Action::Run {
                command: RunAction::Study {
                    manifest,
                    output,
                    overwrite,
                },
            },
            RunAction::Experiment {
                protocol,
                demo,
                port,
                replay,
                baud,
                time_scale,
            } => Action::Experiment {
                command: ExperimentAction::Run {
                    protocol,
                    demo,
                    port,
                    replay,
                    baud,
                    time_scale,
                },
            },
            RunAction::Plugin {
                plugin_id_or_dir,
                input,
                output,
                overwrite,
                timeout,
            } => Action::Plugin {
                command: PluginAction::Run {
                    plugin_id_or_dir,
                    input,
                    output,
                    overwrite,
                    timeout,
                },
            },
            RunAction::Report { command } => match command {
                RunReportAction::Create {
                    input,
                    analysis,
                    output,
                    execute,
                    no_execute,
                    python,
                    portable,
                    colab,
                    include_raw,
                    maximum_embedded_report_mb,
                    open,
                    overwrite,
                    options,
                } => Action::Report {
                    command: None,
                    input,
                    analysis,
                    output,
                    execute,
                    no_execute,
                    python,
                    portable,
                    colab,
                    include_raw,
                    maximum_embedded_report_mb,
                    open,
                    overwrite,
                    options: *options,
                },
                RunReportAction::Setup { recreate, python } => Action::Report {
                    command: Some(ReportAction::Setup { recreate, python }),
                    input: None,
                    analysis: None,
                    output: None,
                    execute: false,
                    no_execute: false,
                    python: None,
                    portable: false,
                    colab: false,
                    include_raw: false,
                    maximum_embedded_report_mb: 25,
                    open: false,
                    overwrite: false,
                    options: ImportArgs::default(),
                },
                RunReportAction::Healthcheck => Action::Report {
                    command: Some(ReportAction::Doctor),
                    input: None,
                    analysis: None,
                    output: None,
                    execute: false,
                    no_execute: false,
                    python: None,
                    portable: false,
                    colab: false,
                    include_raw: false,
                    maximum_embedded_report_mb: 25,
                    open: false,
                    overwrite: false,
                    options: ImportArgs::default(),
                },
            },
        });
    }
}

#[derive(Debug, Subcommand)]
pub enum ExperimentAction {
    /// Record EEG while executing a validated phase/trial plan
    Run {
        #[arg(long)]
        protocol: PathBuf,
        /// Use the deterministic synthetic source
        #[arg(long, conflicts_with_all = ["port", "replay"])]
        demo: bool,
        /// Serial port; repeat for a dual-port source
        #[arg(long, value_name = "PORT", conflicts_with = "replay")]
        port: Vec<String>,
        /// Use the source timeline from an existing native LEMON session
        #[arg(long, value_name = "SESSION", conflicts_with_all = ["demo", "port"])]
        replay: Option<PathBuf>,
        #[arg(long, value_parser=clap::value_parser!(u32).range(1..))]
        baud: Option<u32>,
        /// Test-only phase duration multiplier for demo/headless
        #[arg(long, default_value_t = 1.0)]
        time_scale: f64,
    },
}

#[derive(Debug, Subcommand)]
pub enum PluginsAction {
    /// Проверить и подключить локальный каталог плагина без запуска
    Add { plugin_dir: PathBuf },
    /// Показать подключённые плагины и их состояние
    List,
    /// Показать manifest и состояние плагина
    Inspect { plugin_id_or_dir: String },
    /// Удалить ID из реестра, сохранив файлы плагина
    Remove { plugin_id: String },
    /// Включить подключённый плагин
    Enable { plugin_id: String },
    /// Выключить подключённый плагин
    Disable { plugin_id: String },
}

#[derive(Debug, Subcommand)]
pub enum PluginAction {
    /// Run a plugin against an existing AnalysisBundle
    Run {
        plugin_id_or_dir: String,
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        overwrite: bool,
        #[arg(long, default_value_t = 300, value_parser=clap::value_parser!(u64).range(1..=86400))]
        timeout: u64,
    },
}
#[derive(Debug, Clone, Args)]
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
impl Default for ImportArgs {
    fn default() -> Self {
        Self {
            sample_rate: None,
            time_column: None,
            channel_column: Vec::new(),
            delimiter: "auto".into(),
            units: None,
            channel_name: Vec::new(),
        }
    }
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
        Some(Action::Live {
            port,
            baud,
            protocol,
            sample_rate,
            units,
            startup_delay_ms,
        }) => {
            experiment.source.mode = "serial".into();
            if let Some(protocol) = protocol {
                experiment.source.serial_protocol = protocol.clone();
            }
            if let Some(rate) = sample_rate {
                experiment.source.sample_rate_hz = *rate;
            }
            if let Some(delay) = startup_delay_ms {
                experiment.source.serial_startup_delay_ms = *delay;
            }
            if let Some(units) = units {
                experiment.source.units = Some(units.clone());
            }
            if experiment.source.serial_protocol == "tagged-u8" {
                if sample_rate.is_none() && config_path.is_none() {
                    return Err("tagged-u8: nominal rate is unknown; pass --sample-rate 333.333 (for a 3000 us timer)".into());
                }
                if experiment.source.units.is_none() {
                    experiment.source.units = Some("adc_counts_u8".into());
                }
            }
            if !port.is_empty() {
                experiment.source.ports = port.clone();
            }
            if let Some(baud) = baud {
                experiment.source.baud_rate = *baud;
            }
            if cli.channels.is_none() && config_path.is_none() {
                experiment.source.channels = if experiment.source.serial_protocol == "tagged-u8" {
                    2
                } else {
                    experiment.source.ports.len().max(1)
                };
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
        Some(Action::Experiment {
            command:
                ExperimentAction::Run {
                    demo,
                    port,
                    replay,
                    baud,
                    ..
                },
        }) => {
            if *demo {
                experiment.source.mode = "synthetic".into();
            } else if let Some(directory) = replay {
                let metadata = storage::read_metadata(directory)?;
                experiment.source.mode = "replay".into();
                experiment.source.replay_path = directory.to_string_lossy().into_owned();
                experiment.source.channels = metadata.config.source.channels;
                experiment.source.sample_rate_hz = metadata.config.source.sample_rate_hz;
                experiment.source.units = metadata.units;
            } else {
                experiment.source.mode = "serial".into();
                if !port.is_empty() {
                    experiment.source.ports = port.clone();
                }
                if let Some(baud) = baud {
                    experiment.source.baud_rate = *baud;
                }
                if experiment.source.ports.is_empty() {
                    return Err("experiment run: provide --demo or --port PORT".into());
                }
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
                    "Используйте lemon run demo или lemon run live --port PORT; прежний --source удалён.",
                )
            }
            "--replay" => {
                return Some("Используйте lemon run replay SESSION_DIRECTORY [--speed 2].")
            }
            "--list-ports" => return Some("Используйте lemon ports."),
            "--write-config" => return Some("Используйте lemon config init FILE."),
            _ => {}
        }
    }
    if args
        .iter()
        .any(|a| a == "--port" || a.starts_with("--port="))
        && !args
            .iter()
            .any(|a| matches!(a.as_str(), "live" | "probe" | "experiment"))
    {
        return Some("Используйте lemon run live --port PORT [--baud 115200].");
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

#[cfg(test)]
mod plugin_cli_tests {
    use super::*;
    fn normalized(args: &[&str]) -> String {
        let mut cli = Cli::try_parse_from(args).expect("CLI");
        cli.normalize_run();
        format!(
            "{:?}|{:?}|{:?}|{}|{:?}|{}",
            cli.command, cli.config, cli.channels, cli.record, cli.duration, cli.headless
        )
    }
    #[test]
    fn parses_registry_commands() {
        for args in [
            vec!["lemon", "plugins", "add", "./plugin"],
            vec!["lemon", "plugins", "list"],
            vec!["lemon", "plugins", "inspect", "example-summary"],
            vec!["lemon", "plugins", "disable", "example-summary"],
        ] {
            assert!(Cli::try_parse_from(args).is_ok());
        }
    }
    #[test]
    fn parses_plugin_run_contract() {
        let cli = Cli::try_parse_from([
            "lemon",
            "plugin",
            "run",
            "example-summary",
            "--input",
            "analysis",
            "--config",
            "plugin-config.json",
            "--output",
            "result",
            "--timeout",
            "7",
        ])
        .expect("CLI");
        assert_eq!(cli.config, Some(PathBuf::from("plugin-config.json")));
        assert!(matches!(
            cli.command,
            Some(Action::Plugin {
                command: PluginAction::Run { timeout: 7, .. }
            })
        ));
    }
    #[test]
    fn parses_experiment_run_contract() {
        let cli = Cli::try_parse_from([
            "lemon",
            "experiment",
            "run",
            "--protocol",
            "examples/motor-imagery-demo.json",
            "--demo",
            "--headless",
            "--record",
            "--time-scale",
            "0.01",
        ])
        .expect("CLI");
        assert!(cli.record && cli.headless);
        assert!(matches!(
            cli.command,
            Some(Action::Experiment {
                command: ExperimentAction::Run { demo: true, .. }
            })
        ));
    }

    #[test]
    fn canonical_modes_parse_including_offline_study() {
        for args in [
            vec!["lemon", "run", "demo"],
            vec!["lemon", "run", "live", "--port", "serial"],
            vec!["lemon", "run", "replay", "recording"],
            vec!["lemon", "run", "analyze", "recording"],
            vec![
                "lemon",
                "run",
                "study",
                "--manifest",
                "study.json",
                "--output",
                "reports/study",
                "--config",
                "dsp.json",
                "--overwrite",
            ],
            vec![
                "lemon",
                "run",
                "experiment",
                "--protocol",
                "protocol.json",
                "--demo",
            ],
            vec![
                "lemon",
                "run",
                "plugin",
                "example",
                "--input",
                "analysis",
                "--config",
                "plugin.json",
                "--output",
                "result",
            ],
            vec!["lemon", "run", "report", "create", "analysis"],
            vec!["lemon", "run", "report", "setup", "--recreate"],
            vec!["lemon", "run", "report", "healthcheck"],
        ] {
            Cli::try_parse_from(args).expect("canonical command");
        }
        let error = Cli::try_parse_from(["lemon", "run", "study"])
            .expect_err("study requires manifest and output");
        assert_eq!(
            error.kind(),
            clap::error::ErrorKind::MissingRequiredArgument
        );
    }

    #[test]
    fn canonical_and_legacy_modes_normalize_to_identical_requests() {
        for (canonical, legacy) in [
            (
                vec![
                    "lemon",
                    "run",
                    "live",
                    "--port",
                    "a",
                    "--baud",
                    "9600",
                    "--protocol",
                    "csv",
                    "--sample-rate",
                    "250",
                    "--units",
                    "counts",
                    "--startup-delay-ms",
                    "20",
                    "--channels",
                    "2",
                    "--record",
                ],
                vec![
                    "lemon",
                    "live",
                    "--port",
                    "a",
                    "--baud",
                    "9600",
                    "--protocol",
                    "csv",
                    "--sample-rate",
                    "250",
                    "--units",
                    "counts",
                    "--startup-delay-ms",
                    "20",
                    "--channels",
                    "2",
                    "--record",
                ],
            ),
            (
                vec![
                    "lemon",
                    "run",
                    "replay",
                    "input.dat",
                    "--speed",
                    "2",
                    "--sample-rate",
                    "250",
                    "--channel-name",
                    "A0",
                    "--headless",
                ],
                vec![
                    "lemon",
                    "replay",
                    "input.dat",
                    "--speed",
                    "2",
                    "--sample-rate",
                    "250",
                    "--channel-name",
                    "A0",
                    "--headless",
                ],
            ),
            (
                vec![
                    "lemon",
                    "run",
                    "analyze",
                    "input.dmov",
                    "--output",
                    "analysis",
                    "--overwrite",
                    "--sample-rate",
                    "250",
                    "--units",
                    "counts",
                ],
                vec![
                    "lemon",
                    "analyze",
                    "input.dmov",
                    "--output",
                    "analysis",
                    "--overwrite",
                    "--sample-rate",
                    "250",
                    "--units",
                    "counts",
                ],
            ),
            (
                vec![
                    "lemon",
                    "run",
                    "experiment",
                    "--protocol",
                    "protocol.json",
                    "--demo",
                    "--time-scale",
                    "0.1",
                    "--headless",
                    "--record",
                ],
                vec![
                    "lemon",
                    "experiment",
                    "run",
                    "--protocol",
                    "protocol.json",
                    "--demo",
                    "--time-scale",
                    "0.1",
                    "--headless",
                    "--record",
                ],
            ),
            (
                vec![
                    "lemon",
                    "run",
                    "plugin",
                    "example",
                    "--input",
                    "analysis",
                    "--config",
                    "plugin.json",
                    "--output",
                    "result",
                    "--overwrite",
                    "--timeout",
                    "7",
                ],
                vec![
                    "lemon",
                    "plugin",
                    "run",
                    "example",
                    "--input",
                    "analysis",
                    "--config",
                    "plugin.json",
                    "--output",
                    "result",
                    "--overwrite",
                    "--timeout",
                    "7",
                ],
            ),
            (
                vec![
                    "lemon",
                    "run",
                    "report",
                    "create",
                    "recording",
                    "--output",
                    "report",
                    "--no-execute",
                    "--portable",
                    "--include-raw",
                    "--overwrite",
                ],
                vec![
                    "lemon",
                    "report",
                    "recording",
                    "--output",
                    "report",
                    "--no-execute",
                    "--portable",
                    "--include-raw",
                    "--overwrite",
                ],
            ),
            (
                vec!["lemon", "run", "report", "setup", "--recreate"],
                vec!["lemon", "report", "setup", "--recreate"],
            ),
            (
                vec!["lemon", "run", "report", "healthcheck"],
                vec!["lemon", "report", "doctor"],
            ),
        ] {
            assert_eq!(normalized(&canonical), normalized(&legacy));
        }
        assert_eq!(
            normalized(&["lemon", "run", "demo", "--duration", "1"]),
            normalized(&["lemon", "demo", "--duration", "1"])
        );
    }
}
