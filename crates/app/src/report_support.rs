use core_types::Result;
use std::{
    fs,
    path::{Path, PathBuf},
};

pub fn open(path: &Path) {
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(path).status();
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("explorer.exe")
        .arg(path)
        .status();
    #[cfg(all(unix, not(target_os = "macos")))]
    let result = std::process::Command::new("xdg-open").arg(path).status();
    match result {
        Ok(status) if !status.success() => eprintln!(
            "LEMON: не удалось открыть notebook ({status}); путь: {}",
            path.display()
        ),
        Err(error) => eprintln!(
            "LEMON: не удалось открыть notebook: {error}; путь: {}",
            path.display()
        ),
        _ => {}
    }
}

pub struct TempAnalysis(PathBuf);

impl TempAnalysis {
    pub fn new() -> Result<Self> {
        for index in 0..10_000 {
            let path =
                std::env::temp_dir().join(format!("lemon-report-{}-{index}", std::process::id()));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.to_string()),
            }
        }
        Err("report: cannot allocate temporary directory".into())
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempAnalysis {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
