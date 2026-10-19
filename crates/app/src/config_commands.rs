use app::cli::{self, Cli, ConfigAction};
use core_types::{Config, Result};
use std::fs;
use tui::settings::Settings;

pub fn run(command: &ConfigAction, cli: &Cli) -> Result<()> {
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
                .map_err(|error| {
                    format!(
                        "{}: {error}; выберите новый путь, существующие файлы не перезаписываются",
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
