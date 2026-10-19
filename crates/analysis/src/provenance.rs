use std::{
    collections::BTreeMap,
    fs::{self, File},
    hash::{Hash, Hasher},
    io::Read,
    path::{Path, PathBuf},
};

use storage::Metadata;

use crate::AnalysisError;

pub(crate) fn source_hashes(
    input: &Path,
    metadata: &Metadata,
) -> Result<BTreeMap<String, String>, AnalysisError> {
    let files = if input.is_dir() && input.join("metadata.json").is_file() {
        ["metadata.json", "raw.csv", "events.csv", "experiment.json"]
            .iter()
            .map(|name| input.join(name))
            .filter(|path| path.is_file())
            .collect::<Vec<_>>()
    } else if let Some(provenance) = &metadata.provenance {
        provenance.files_used.iter().map(PathBuf::from).collect()
    } else {
        vec![input.to_path_buf()]
    };
    files
        .iter()
        .map(|path| Ok((path.display().to_string(), hash_path(path)?)))
        .collect()
}

pub(crate) fn source_format(metadata: &Metadata) -> String {
    metadata
        .provenance
        .as_ref()
        .map(|provenance| provenance.original_format.clone())
        .unwrap_or_else(|| "lemon-session".into())
}

pub(crate) fn hash_path(path: &Path) -> Result<String, AnalysisError> {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let mut files = Vec::new();
    if path.is_dir() {
        for entry in fs::read_dir(path)
            .map_err(|error| format!("analyze/hash {}: {error}", path.display()))?
        {
            let path = entry.map_err(|error| error.to_string())?.path();
            if path.is_file() {
                files.push(path);
            }
        }
        files.sort();
    } else {
        files.push(path.to_path_buf());
    }
    for file_path in files {
        file_path.file_name().hash(&mut hasher);
        let mut file = File::open(&file_path)
            .map_err(|error| format!("analyze/hash {}: {error}", file_path.display()))?;
        let mut buffer = [0_u8; 8192];
        loop {
            let bytes_read = file
                .read(&mut buffer)
                .map_err(|error| format!("analyze/hash {}: {error}", file_path.display()))?;
            if bytes_read == 0 {
                break;
            }
            hasher.write(&buffer[..bytes_read]);
        }
    }
    Ok(format!("{:016x}", hasher.finish()))
}
