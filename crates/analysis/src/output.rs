use std::{
    fs,
    path::{Path, PathBuf},
};

use storage::Metadata;

use crate::{AnalysisError, AnalysisOptions};

/// Owns the staging directory and publishes it atomically when analysis succeeds.
pub(crate) struct OutputTransaction {
    input: PathBuf,
    target: PathBuf,
    staging: PathBuf,
    overwrite: bool,
}

impl OutputTransaction {
    pub(crate) fn new(
        input: &Path,
        options: &AnalysisOptions,
        metadata: &Metadata,
    ) -> Result<Self, AnalysisError> {
        let target = output_path(options, metadata)?;
        validate_output(input, &target, options.overwrite)?;
        let staging = staging_directory(&target)?;
        Ok(Self {
            input: input.to_path_buf(),
            target,
            staging,
            overwrite: options.overwrite,
        })
    }

    pub(crate) fn staging_directory(&self) -> &Path {
        &self.staging
    }

    pub(crate) fn commit(self) -> Result<PathBuf, AnalysisError> {
        let backup = self.staging.with_extension("previous");
        let replacing = self.target.exists();
        if replacing {
            validate_output(&self.input, &self.target, self.overwrite)?;
            if backup.exists() {
                return Err("analyze/output: backup path already exists".into());
            }
            fs::rename(&self.target, &backup)
                .map_err(|error| format!("analyze/output backup: {error}"))?;
        }
        if let Err(error) = fs::rename(&self.staging, &self.target) {
            if replacing {
                fs::rename(&backup, &self.target).map_err(|restore| {
                    format!(
                        "analyze/output: {error}; previous report retained at {}: {restore}",
                        backup.display()
                    )
                })?;
            }
            return Err(format!("analyze/output publish: {error}"));
        }
        if replacing {
            fs::remove_dir_all(backup)
                .map_err(|error| format!("analyze/output backup cleanup: {error}"))?;
        }
        Ok(self.target.clone())
    }
}

impl Drop for OutputTransaction {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.staging);
    }
}

// Only a complete, recognizable bundle may be replaced. Never remove an input/ancestor.
fn validate_output(input: &Path, output: &Path, overwrite: bool) -> Result<(), AnalysisError> {
    if !output.exists() {
        return Ok(());
    }
    let target = output.canonicalize().map_err(|e| e.to_string())?;
    let input = input.canonicalize().map_err(|e| e.to_string())?;
    if input.starts_with(&target) || target.starts_with(&input) {
        return Err("analyze/output: output overlaps input; choose a separate directory".into());
    }
    if !overwrite {
        return Err(
            "analyze/output: directory exists; choose another path or use --overwrite".into(),
        );
    }
    let allowed = [
        "manifest.json",
        "summary.json",
        "summary.md",
        "processed.csv",
        "spectrum.csv",
        "band-power.csv",
        "quality-events.csv",
        "excluded-windows.csv",
        "experiment.json",
    ];
    if !target.join("manifest.json").is_file() || !target.join("summary.json").is_file() {
        return Err("analyze/output: --overwrite requires an existing AnalysisBundle".into());
    }
    for entry in fs::read_dir(&target).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if !entry.file_type().map_err(|e| e.to_string())?.is_file()
            || !allowed.contains(&entry.file_name().to_string_lossy().as_ref())
        {
            return Err("analyze/output: refusing to overwrite unrelated files".into());
        }
    }
    Ok(())
}

fn staging_directory(output: &Path) -> Result<PathBuf, AnalysisError> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    for _ in 0..1_000 {
        let index = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = parent.join(format!(
            ".lemon-analysis-{}-{index}.partial",
            std::process::id()
        ));
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.to_string()),
        }
    }
    Err("analyze/output: cannot allocate staging directory".into())
}

fn output_path(options: &AnalysisOptions, metadata: &Metadata) -> Result<PathBuf, AnalysisError> {
    if let Some(path) = &options.output {
        if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
            fs::create_dir_all(parent)
                .map_err(|error| format!("analyze/output {}: {error}", parent.display()))?;
        }
        return Ok(path.clone());
    }
    let parent = Path::new(&metadata.config.recording_dir);
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    for index in 0..1_000 {
        let path = parent.join(format!(
            "analysis-{}-{index}",
            metadata
                .config
                .session
                .name
                .replace(|character: char| !character.is_ascii_alphanumeric(), "_")
        ));
        if !path.exists() {
            return Ok(path);
        }
    }
    Err("analyze/output: cannot create unique directory".into())
}
