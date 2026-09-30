use clap::Parser;
use eeg_app::cli::{resolve, Action, Cli, Environment, LogLevel};
use eeg_core::Config;
use eeg_tui::settings::{Settings, Theme};
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static ID: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "lemon-cli-{}-{}",
            std::process::id(),
            ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("temp");
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn parse(args: &[&str]) -> Cli {
    Cli::try_parse_from(args).expect("CLI")
}
#[test]
fn subcommands_and_short_arguments() {
    let c = parse(&["lemon", "demo", "-n", "2", "-r", "-t", "1", "--headless"]);
    assert!(matches!(c.command, Some(Action::Demo)));
    assert_eq!(c.channels, Some(2));
    assert!(c.record);
    for args in [
        vec!["lemon", "live", "--port", "a", "--port", "b"],
        vec!["lemon", "replay", "session", "--speed", "2"],
        vec!["lemon", "ports"],
        vec!["lemon", "config", "init"],
        vec!["lemon", "config", "check", "config.json"],
    ] {
        Cli::try_parse_from(args).expect("subcommand");
    }
    assert!(parse(&["lemon"]).command.is_none());
    assert_eq!(
        Cli::try_parse_from(["lemon", "demo", "-V"])
            .expect_err("version")
            .kind(),
        clap::error::ErrorKind::DisplayVersion
    );
}
#[test]
fn conflicts_and_invalid_numbers_are_rejected() {
    for args in [
        vec!["lemon", "demo", "--port", "a"],
        vec!["lemon", "demo", "--speed", "2"],
        vec!["lemon", "demo", "-n", "3"],
        vec!["lemon", "replay", "s", "--speed", "NaN"],
        vec!["lemon", "demo", "-t", "inf"],
        vec!["lemon", "demo", "-v", "--log-level", "info"],
    ] {
        assert!(Cli::try_parse_from(args).is_err());
    }
    let c = parse(&["lemon", "live", "--port", "a", "--port", "b", "-n", "1"]);
    assert!(resolve(&c, &Environment::default(), None, None).is_err());
}
#[test]
fn cli_environment_user_file_and_defaults_precedence() {
    let temp = Temp::new();
    let path = temp.0.join("experiment.json");
    let mut file = Config {
        recording_dir: "file-dir".into(),
        ..Config::default()
    };
    file.source.channels = 1;
    file.tui_hz = 10;
    fs::write(&path, serde_json::to_vec(&file).expect("json")).expect("write");
    let mut user = Settings::default();
    user.storage.directory = Some("user-dir".into());
    user.general.fps = 30;
    let env = Environment {
        config: Some(path.display().to_string()),
        data_dir: Some("env-dir".into()),
        log: Some("info".into()),
    };
    let cli = parse(&["lemon", "demo", "--data-dir", "cli-dir", "-n", "2", "-v"]);
    let r = resolve(&cli, &env, Some(user.clone()), None).expect("resolve");
    assert_eq!(r.experiment.recording_dir, "cli-dir");
    assert_eq!(r.experiment.source.channels, 2);
    assert_eq!(r.settings.general.fps, 30);
    assert_eq!(r.log, LogLevel::Debug);
    let cli = parse(&["lemon", "demo"]);
    assert_eq!(
        resolve(&cli, &env, Some(user.clone()), None)
            .expect("env")
            .experiment
            .recording_dir,
        "env-dir"
    );
    let no_data = Environment {
        data_dir: None,
        ..env.clone()
    };
    assert_eq!(
        resolve(&cli, &no_data, Some(user), None)
            .expect("user")
            .experiment
            .recording_dir,
        "user-dir"
    );
    let r = resolve(&cli, &no_data, None, None).expect("file");
    assert_eq!(r.experiment.recording_dir, "file-dir");
    assert_eq!(r.settings.general.fps, 10);
    let r = resolve(&cli, &Environment::default(), None, None).expect("defaults");
    assert_eq!(r.experiment.source.channels, 2);
    let cli =
        Cli::try_parse_from(["lemon", "-f", path.to_str().expect("path"), "demo"]).expect("cli");
    let env = Environment {
        config: Some("missing-file".into()),
        ..Environment::default()
    };
    assert!(resolve(&cli, &env, None, None).is_ok());
}
#[test]
fn invalid_environment_is_actionable() {
    for env in [
        Environment {
            config: Some(" ".into()),
            ..Environment::default()
        },
        Environment {
            data_dir: Some("".into()),
            ..Environment::default()
        },
        Environment {
            log: Some("loud".into()),
            ..Environment::default()
        },
    ] {
        let error = resolve(&parse(&["lemon", "demo"]), &env, None, None)
            .err()
            .expect("invalid");
        assert!(error.contains("LEMON_"), "{error}");
    }
}
#[test]
fn theme_does_not_change_experiment_metadata() {
    let cli = parse(&["lemon", "demo"]);
    let mut a = Settings::default();
    a.appearance.theme = Theme::Light;
    let mut b = a.clone();
    b.appearance.theme = Theme::Monochrome;
    b.general.fps = 60;
    let x = resolve(&cli, &Environment::default(), Some(a), None).expect("light");
    let y = resolve(&cli, &Environment::default(), Some(b), None).expect("mono");
    assert_eq!(
        serde_json::to_value(x.experiment).expect("json"),
        serde_json::to_value(y.experiment).expect("json")
    );
}
#[test]
fn lemon_binary_help_version_and_config_commands() {
    let bin = env!("CARGO_BIN_EXE_lemon");
    for arg in ["--help", "--version"] {
        let output = Command::new(bin).arg(arg).output().expect("lemon");
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout)
            .to_lowercase()
            .contains("lemon"));
    }
    let temp = Temp::new();
    let path = temp.0.join("config.json");
    let output = Command::new(bin)
        .args(["config", "init"])
        .arg(&path)
        .env_remove("LEMON_CONFIG")
        .env_remove("LEMON_DATA_DIR")
        .env_remove("LEMON_LOG")
        .output()
        .expect("init");
    assert!(output.status.success(), "{:?}", output);
    let output = Command::new(bin)
        .args(["config", "check"])
        .arg(&path)
        .output()
        .expect("check");
    assert!(output.status.success());
    let duplicate = Command::new(bin)
        .args(["config", "init"])
        .arg(&path)
        .output()
        .expect("duplicate");
    assert!(!duplicate.status.success());
    let output = Command::new(bin)
        .args(["demo", "--headless", "-t", "0.05", "--settings"])
        .arg(temp.0.join("settings.json"))
        .env_remove("LEMON_CONFIG")
        .env_remove("LEMON_DATA_DIR")
        .env_remove("LEMON_LOG")
        .output()
        .expect("demo");
    assert!(output.status.success(), "{:?}", output);
    assert!(String::from_utf8_lossy(&output.stdout).contains("samples="));
}
#[test]
fn two_live_ports_infer_two_channels() {
    let c = parse(&["lemon", "live", "--port", "a", "--port", "b"]);
    let r = resolve(&c, &Environment::default(), None, None).expect("ports");
    assert_eq!(r.experiment.source.channels, 2);
    assert_eq!(r.experiment.source.mode, "serial-test");
}
