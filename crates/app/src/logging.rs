use app::cli::LogLevel;
use core_types::Result;
use std::{fs, io::Write, path::Path};

pub struct Logger {
    level: LogLevel,
    file: Option<fs::File>,
    headless: bool,
}

impl Logger {
    pub fn new(level: LogLevel, headless: bool, settings_path: &Path) -> Result<Self> {
        let file = if !headless && level != LogLevel::Off {
            let parent = settings_path
                .parent()
                .filter(|path| !path.as_os_str().is_empty())
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

    pub fn write(&mut self, level: LogLevel, text: &str) {
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
