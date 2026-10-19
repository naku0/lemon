//! Atomic offline-study preparation over the existing import/analyze pipeline.

use analysis::{analyze, AnalysisInput, AnalysisOptions};
use core_types::Config;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::Read,
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub const STUDY_SCHEMA_VERSION: u32 = 1;
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StudySpec {
    pub schema_version: u32,
    pub id: String,
    pub display_name: String,
    pub description: String,
    pub baseline: StudySource,
    pub conditions: Vec<StudySource>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StudySource {
    pub label: String,
    pub input: PathBuf,
}

#[derive(Debug, Clone)]
pub struct StudyRequest {
    pub manifest: PathBuf,
    pub output: PathBuf,
    pub config: Config,
    pub overwrite: bool,
}

#[derive(Debug, Clone)]
pub struct StudyResult {
    pub directory: PathBuf,
    pub manifest: StudyManifest,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StudyManifest {
    pub schema_version: u32,
    pub id: String,
    pub display_name: String,
    pub description: String,
    pub created_at: String,
    pub lemon_version: String,
    pub dsp_config_hash: String,
    pub baseline: PreparedSource,
    pub conditions: Vec<PreparedSource>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreparedSource {
    pub label: String,
    pub input: PathBuf,
    pub source_hash: String,
    pub analysis_bundle: PathBuf,
    pub dsp_config_hash: String,
    pub import_warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
struct MuConfig {
    mode: &'static str,
    channels: Vec<String>,
    bands: MuBands,
    window_seconds: f64,
    hop_seconds: f64,
    baseline: MuBaseline,
    conditions: Vec<MuCondition>,
}

#[derive(Debug, Serialize)]
struct MuBands {
    mu_hz: [f64; 2],
    beta_hz: [f64; 2],
}

#[derive(Debug, Serialize)]
struct MuBaseline {
    label: String,
}

#[derive(Debug, Serialize)]
struct MuCondition {
    label: String,
    analysis_bundle: PathBuf,
}

#[derive(Debug, Deserialize)]
struct AnalysisProvenance {
    channels: Vec<String>,
    units: Option<String>,
    sample_rate_hz: f64,
    processing_config: core_types::ProcessingConfig,
    #[serde(default)]
    import_warnings: Vec<String>,
    #[serde(default)]
    analysis_warnings: Vec<String>,
}

pub fn load(path: &Path) -> Result<StudySpec, String> {
    let file =
        File::open(path).map_err(|error| format!("study/manifest {}: {error}", path.display()))?;
    let size = file.metadata().map_err(|error| error.to_string())?.len();
    if size > MAX_MANIFEST_BYTES {
        return Err("study/manifest: JSON exceeds 1 MiB".into());
    }
    let spec: StudySpec = serde_json::from_reader(file)
        .map_err(|error| format!("study/manifest JSON {}: {error}", path.display()))?;
    validate(&spec)?;
    Ok(spec)
}

pub fn validate(spec: &StudySpec) -> Result<(), String> {
    if spec.schema_version != STUDY_SCHEMA_VERSION {
        return Err(format!(
            "study/manifest: unsupported schema_version {}; expected {}",
            spec.schema_version, STUDY_SCHEMA_VERSION
        ));
    }
    if spec.id.is_empty()
        || !spec
            .id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(
            "study/manifest: id must use lowercase ASCII letters, digits and hyphens".into(),
        );
    }
    if spec.display_name.trim().is_empty() {
        return Err("study/manifest: display_name must not be empty".into());
    }
    if spec.conditions.is_empty() {
        return Err("study/manifest: at least one condition is required".into());
    }
    let mut labels = BTreeSet::new();
    for source in std::iter::once(&spec.baseline).chain(&spec.conditions) {
        validate_source(source)?;
        if !labels.insert(source.label.clone()) {
            return Err(format!(
                "study/manifest: duplicate label {:?}",
                source.label
            ));
        }
    }
    Ok(())
}

fn validate_source(source: &StudySource) -> Result<(), String> {
    if source.label.trim().is_empty() {
        return Err("study/manifest: labels must not be empty".into());
    }
    if source.label == "."
        || source.label == ".."
        || source.label.contains('/')
        || source.label.contains('\\')
        || source.label.contains('\0')
    {
        return Err(format!(
            "study/manifest: label {:?} is not a safe output directory name",
            source.label
        ));
    }
    if source.input.as_os_str().is_empty() || source.input.is_absolute() {
        return Err(format!(
            "study/manifest: input for {:?} must be a relative path",
            source.label
        ));
    }
    Ok(())
}

pub fn run(request: StudyRequest) -> Result<StudyResult, String> {
    request
        .config
        .validate()
        .map_err(|error| format!("study/DSP config: {error}"))?;
    let spec = load(&request.manifest)?;
    let manifest_path = fs::canonicalize(&request.manifest)
        .map_err(|error| format!("study/manifest {}: {error}", request.manifest.display()))?;
    let manifest_dir = manifest_path
        .parent()
        .ok_or("study/manifest: path has no parent directory")?;
    let resolved = resolve_sources(&spec, manifest_dir)?;
    let output = absolute_output(&request.output)?;
    validate_output(&output, &manifest_path, &resolved, request.overwrite)?;
    let staging = Staging::new(&output)?;
    let dsp_config_hash = hash_json(&request.config.processing)?;
    let mut prepared = Vec::with_capacity(resolved.len());
    let mut bundle_channels = Vec::with_capacity(resolved.len());
    let mut bundle_units = Vec::with_capacity(resolved.len());
    let mut warnings = Vec::new();

    for (source, input) in &resolved {
        let bundle_relative = PathBuf::from(&source.label);
        let bundle = analyze(
            AnalysisInput {
                path: input.clone(),
            },
            AnalysisOptions {
                output: Some(staging.path.join(&bundle_relative)),
                config: Some(request.config.clone()),
                sample_rate: Some(request.config.source.sample_rate_hz),
                units: request.config.source.units.clone(),
                overwrite: false,
                ..AnalysisOptions::default()
            },
        )
        .map_err(|error| format!("study/analyze {:?}: {error}", source.label))?;
        let provenance = read_analysis_provenance(&bundle.directory)?;
        let actual_hash = hash_json(&provenance.processing_config)?;
        if actual_hash != dsp_config_hash {
            return Err(format!(
                "study/analyze {:?}: effective DSP config differs from the study config",
                source.label
            ));
        }
        if (provenance.sample_rate_hz - request.config.source.sample_rate_hz).abs() > 1e-9 {
            return Err(format!(
                "study/analyze {:?}: effective sample rate differs from the study config",
                source.label
            ));
        }
        for warning in provenance
            .import_warnings
            .iter()
            .chain(&provenance.analysis_warnings)
        {
            warnings.push(format!("{}: {warning}", source.label));
        }
        bundle_channels.push(provenance.channels);
        bundle_units.push(provenance.units.clone());
        prepared.push(PreparedSource {
            label: source.label.clone(),
            input: source.input.clone(),
            source_hash: hash_path(input)?,
            analysis_bundle: bundle_relative,
            dsp_config_hash: dsp_config_hash.clone(),
            import_warnings: provenance.import_warnings,
        });
    }

    let channels = common_channels(&bundle_channels)?;
    if bundle_units
        .iter()
        .skip(1)
        .any(|units| units != &bundle_units[0])
    {
        return Err(
            "study: input units differ; set one explicit source.units value in --config".into(),
        );
    }
    let baseline = prepared
        .first()
        .cloned()
        .ok_or("study: baseline was not prepared")?;
    let conditions = prepared.into_iter().skip(1).collect::<Vec<_>>();
    let study_manifest = StudyManifest {
        schema_version: STUDY_SCHEMA_VERSION,
        id: spec.id,
        display_name: spec.display_name,
        description: spec.description,
        created_at: format!(
            "unix-ms:{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |duration| duration.as_millis())
        ),
        lemon_version: env!("CARGO_PKG_VERSION").into(),
        dsp_config_hash: dsp_config_hash.clone(),
        baseline: baseline.clone(),
        conditions: conditions.clone(),
        warnings,
    };
    write_json(&staging.path.join("study-manifest.json"), &study_manifest)?;
    write_json(
        &staging.path.join("mu-condition-analysis.json"),
        &MuConfig {
            mode: "condition-comparison",
            channels,
            bands: MuBands {
                mu_hz: [8.0, 13.0],
                beta_hz: [13.0, 30.0],
            },
            window_seconds: 2.0,
            hop_seconds: 0.5,
            baseline: MuBaseline {
                label: baseline.label,
            },
            conditions: conditions
                .into_iter()
                .map(|condition| MuCondition {
                    label: condition.label,
                    analysis_bundle: condition.analysis_bundle,
                })
                .collect(),
        },
    )?;
    publish(&staging.path, &output, request.overwrite)?;
    Ok(StudyResult {
        directory: output,
        manifest: study_manifest,
    })
}

fn resolve_sources<'a>(
    spec: &'a StudySpec,
    manifest_dir: &Path,
) -> Result<Vec<(&'a StudySource, PathBuf)>, String> {
    let mut seen = BTreeSet::new();
    std::iter::once(&spec.baseline)
        .chain(&spec.conditions)
        .map(|source| {
            let input = fs::canonicalize(manifest_dir.join(&source.input)).map_err(|error| {
                format!(
                    "study/input {:?} ({}): {error}",
                    source.label,
                    source.input.display()
                )
            })?;
            if !seen.insert(input.clone()) {
                return Err(format!(
                    "study/manifest: input {} is used by more than one role",
                    source.input.display()
                ));
            }
            Ok((source, input))
        })
        .collect()
}

fn read_analysis_provenance(directory: &Path) -> Result<AnalysisProvenance, String> {
    serde_json::from_reader(
        File::open(directory.join("manifest.json"))
            .map_err(|error| format!("study/analysis manifest: {error}"))?,
    )
    .map_err(|error| format!("study/analysis manifest JSON: {error}"))
}

fn common_channels(all: &[Vec<String>]) -> Result<Vec<String>, String> {
    let baseline = all.first().ok_or("study: no analyzed bundles")?;
    let channels = baseline
        .iter()
        .filter(|channel| all.iter().skip(1).all(|set| set.contains(channel)))
        .cloned()
        .collect::<Vec<_>>();
    if channels.is_empty() {
        return Err("study: analyzed recordings have no common signal channels".into());
    }
    Ok(channels)
}

fn hash_json(value: &impl Serialize) -> Result<String, String> {
    let bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

fn hash_path(path: &Path) -> Result<String, String> {
    let mut hasher = Sha256::new();
    hash_entry(path, Path::new(""), &mut hasher)?;
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn hash_entry(path: &Path, relative: &Path, hasher: &mut Sha256) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("study/hash {}: {error}", path.display()))?;
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "study/hash {}: symlinks inside inputs are not supported",
            path.display()
        ));
    }
    if metadata.is_dir() {
        let mut entries = fs::read_dir(path)
            .map_err(|error| format!("study/hash {}: {error}", path.display()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let child_relative = relative.join(entry.file_name());
            hash_entry(&entry.path(), &child_relative, hasher)?;
        }
        return Ok(());
    }
    if !metadata.is_file() {
        return Err(format!(
            "study/hash {}: unsupported file type",
            path.display()
        ));
    }
    hasher.update(relative.to_string_lossy().as_bytes());
    hasher.update([0]);
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(())
}

fn absolute_output(output: &Path) -> Result<PathBuf, String> {
    if output.file_name().is_none() {
        return Err("study/output: choose a directory other than a filesystem root".into());
    }
    let absolute = if output.is_absolute() {
        output.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| error.to_string())?
            .join(output)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    Ok(normalized)
}

fn validate_output(
    output: &Path,
    manifest: &Path,
    sources: &[(&StudySource, PathBuf)],
    overwrite: bool,
) -> Result<(), String> {
    if output == manifest || manifest.starts_with(output) {
        return Err("study/output: output overlaps the study manifest".into());
    }
    for (_, source) in sources {
        if output == source || output.starts_with(source) || source.starts_with(output) {
            return Err(format!(
                "study/output: output overlaps input {}",
                source.display()
            ));
        }
    }
    if !output.exists() {
        return Ok(());
    }
    let metadata = fs::symlink_metadata(output).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("study/output: existing output must be a regular directory".into());
    }
    if !overwrite {
        return Err("study/output: directory exists; use --overwrite to replace it".into());
    }
    Ok(())
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let file =
        File::create(path).map_err(|error| format!("study/write {}: {error}", path.display()))?;
    serde_json::to_writer_pretty(file, value)
        .map_err(|error| format!("study/write {}: {error}", path.display()))
}

fn publish(staging: &Path, output: &Path, overwrite: bool) -> Result<(), String> {
    let backup = output.with_extension(format!("lemon-study-{}-previous", std::process::id()));
    let replacing = output.exists();
    if replacing {
        if !overwrite {
            return Err("study/output: directory exists; use --overwrite to replace it".into());
        }
        if backup.exists() {
            return Err(format!(
                "study/output: backup path already exists: {}",
                backup.display()
            ));
        }
        fs::rename(output, &backup).map_err(|error| format!("study/output backup: {error}"))?;
    }
    if let Err(error) = fs::rename(staging, output) {
        if replacing {
            fs::rename(&backup, output).map_err(|restore| {
                format!(
                    "study/output publish: {error}; restoring previous output failed: {restore}"
                )
            })?;
        }
        return Err(format!("study/output publish: {error}"));
    }
    if replacing {
        fs::remove_dir_all(&backup)
            .map_err(|error| format!("study/output backup cleanup: {error}"))?;
    }
    Ok(())
}

struct Staging {
    path: PathBuf,
}

impl Staging {
    fn new(output: &Path) -> Result<Self, String> {
        let parent = output
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .ok_or("study/output: path has no parent directory")?;
        fs::create_dir_all(parent)
            .map_err(|error| format!("study/output {}: {error}", parent.display()))?;
        for attempt in 0..1000u32 {
            let path = parent.join(format!(
                ".lemon-study-{}-{attempt}.partial",
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(format!("study/output staging: {error}")),
            }
        }
        Err("study/output: cannot allocate staging directory".into())
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
