//! Local registry, validation and out-of-process execution for LEMON plugins.
use lemon_plugin_sdk::{Artifact, PluginResult, PluginStatus, RESULT_SCHEMA_VERSION};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    error::Error,
    fmt,
    fs::{self, File, OpenOptions},
    hash::{DefaultHasher, Hash, Hasher},
    io::Read,
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub const MANIFEST_SCHEMA_VERSION: u32 = 1;
pub const REGISTRY_SCHEMA_VERSION: u32 = 1;
pub const PLUGIN_API_VERSION: u32 = lemon_plugin_sdk::PLUGIN_API_VERSION;
const MAX_JSON_BYTES: u64 = 1024 * 1024;
const MAX_ARTIFACTS: usize = 1024;

#[derive(Debug)]
pub enum PlatformError {
    Registry(String),
    Manifest(String),
    Bundle(String),
    Config(String),
    Output(String),
    Execution(String),
    Result(String),
    Io {
        context: String,
        source: std::io::Error,
    },
}
impl fmt::Display for PlatformError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Registry(s) => write!(f, "plugin registry: {s}"),
            Self::Manifest(s) => write!(f, "plugin manifest: {s}"),
            Self::Bundle(s) => write!(f, "plugin input: {s}"),
            Self::Config(s) => write!(f, "plugin config: {s}"),
            Self::Output(s) => write!(f, "plugin output: {s}"),
            Self::Execution(s) => write!(f, "plugin execution: {s}"),
            Self::Result(s) => write!(f, "plugin result: {s}"),
            Self::Io { context, source } => write!(f, "{context}: {source}"),
        }
    }
}
impl Error for PlatformError {}
pub type Result<T> = std::result::Result<T, PlatformError>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entrypoint {
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginManifest {
    pub schema_version: u32,
    pub id: String,
    pub display_name: String,
    pub version: String,
    pub api_version: u32,
    pub description: String,
    pub entrypoint: Entrypoint,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PluginState {
    Ready,
    Disabled,
    Unavailable,
    Incompatible,
}
impl fmt::Display for PluginState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Ready => "ready",
            Self::Disabled => "disabled",
            Self::Unavailable => "unavailable",
            Self::Incompatible => "incompatible",
        })
    }
}
#[derive(Debug, Clone)]
pub struct PluginInfo {
    pub id: String,
    pub path: PathBuf,
    pub enabled: bool,
    pub state: PluginState,
    pub manifest: Option<PluginManifest>,
    pub reason: Option<String>,
    pub checked_at: SystemTime,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistryEntry {
    pub id: String,
    pub path: PathBuf,
    pub enabled: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistryFile {
    schema_version: u32,
    plugins: Vec<RegistryEntry>,
}
impl Default for RegistryFile {
    fn default() -> Self {
        Self {
            schema_version: REGISTRY_SCHEMA_VERSION,
            plugins: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Platform {
    registry_path: PathBuf,
    registry: RegistryFile,
}
#[derive(Debug, Clone)]
pub struct RunOptions {
    pub plugin: String,
    pub input: PathBuf,
    pub config: PathBuf,
    pub output: PathBuf,
    pub overwrite: bool,
    pub timeout: Duration,
}
#[derive(Debug, Clone)]
pub struct RunReport {
    pub manifest: PluginManifest,
    pub result: PluginResult,
    pub output: PathBuf,
}

impl Platform {
    pub fn open_default() -> Result<Self> {
        Self::open(default_registry_path()?)
    }
    pub fn open(registry_path: impl Into<PathBuf>) -> Result<Self> {
        let registry_path = registry_path.into();
        let registry = if registry_path.is_file() {
            read_limited_json(&registry_path).map_err(PlatformError::Registry)?
        } else {
            RegistryFile::default()
        };
        if registry.schema_version != REGISTRY_SCHEMA_VERSION {
            return Err(PlatformError::Registry(format!(
                "unsupported schema_version {}; expected {REGISTRY_SCHEMA_VERSION}",
                registry.schema_version
            )));
        }
        let mut ids = HashSet::new();
        for entry in &registry.plugins {
            validate_id(&entry.id).map_err(PlatformError::Registry)?;
            if !ids.insert(entry.id.clone()) {
                return Err(PlatformError::Registry(format!(
                    "duplicate plugin ID {:?}",
                    entry.id
                )));
            }
            if !entry.path.is_absolute() {
                return Err(PlatformError::Registry(format!(
                    "path for {:?} is not absolute",
                    entry.id
                )));
            }
        }
        Ok(Self {
            registry_path,
            registry,
        })
    }
    pub fn registry_path(&self) -> &Path {
        &self.registry_path
    }
    pub fn add(&mut self, directory: impl AsRef<Path>) -> Result<PluginInfo> {
        let directory = canonical_directory(directory.as_ref())?;
        let manifest = validate_plugin_directory(&directory)?;
        if self.registry.plugins.iter().any(|p| p.id == manifest.id) {
            return Err(PlatformError::Registry(format!(
                "plugin ID {:?} is already registered",
                manifest.id
            )));
        }
        self.registry.plugins.push(RegistryEntry {
            id: manifest.id.clone(),
            path: directory,
            enabled: true,
        });
        self.registry.plugins.sort_by(|a, b| a.id.cmp(&b.id));
        self.save()?;
        self.inspect(&manifest.id)
    }
    pub fn remove(&mut self, id: &str) -> Result<()> {
        let before = self.registry.plugins.len();
        self.registry.plugins.retain(|p| p.id != id);
        if before == self.registry.plugins.len() {
            return Err(PlatformError::Registry(format!("unknown plugin ID {id:?}")));
        }
        self.save()
    }
    pub fn set_enabled(&mut self, id: &str, enabled: bool) -> Result<PluginInfo> {
        let entry = self
            .registry
            .plugins
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or_else(|| PlatformError::Registry(format!("unknown plugin ID {id:?}")))?;
        entry.enabled = enabled;
        self.save()?;
        self.inspect(id)
    }
    pub fn list(&self) -> Vec<PluginInfo> {
        self.registry.plugins.iter().map(check_entry).collect()
    }
    pub fn inspect(&self, id_or_dir: &str) -> Result<PluginInfo> {
        if let Some(entry) = self.registry.plugins.iter().find(|p| p.id == id_or_dir) {
            return Ok(check_entry(entry));
        }
        let path = Path::new(id_or_dir);
        if path.components().count() > 1 || path.is_dir() {
            let canonical = canonical_directory(path)?;
            let manifest = validate_plugin_directory(&canonical)?;
            return Ok(PluginInfo {
                id: manifest.id.clone(),
                path: canonical,
                enabled: true,
                state: PluginState::Ready,
                manifest: Some(manifest),
                reason: None,
                checked_at: SystemTime::now(),
            });
        }
        Err(PlatformError::Registry(format!(
            "unknown plugin ID or directory {id_or_dir:?}"
        )))
    }
    pub fn run(&self, options: &RunOptions) -> Result<RunReport> {
        if options.timeout.is_zero() {
            return Err(PlatformError::Execution(
                "timeout must be greater than zero".into(),
            ));
        }
        let info = self.inspect(&options.plugin)?;
        if !info.enabled {
            return Err(PlatformError::Execution(format!(
                "plugin {:?} is disabled",
                info.id
            )));
        }
        if info.state != PluginState::Ready {
            return Err(PlatformError::Execution(
                info.reason
                    .unwrap_or_else(|| format!("plugin is {}", info.state)),
            ));
        }
        let manifest = info
            .manifest
            .ok_or_else(|| PlatformError::Manifest("manifest unavailable".into()))?;
        let input = fs::canonicalize(&options.input)
            .map_err(|e| PlatformError::Bundle(format!("{}: {e}", options.input.display())))?;
        let bundle = lemon_plugin_sdk::Bundle::open(&input)
            .map_err(|e| PlatformError::Bundle(e.to_string()))?;
        let before = snapshot_bundle(bundle.directory())?;
        let config = validate_config(&options.config)?;
        let output_target = resolved_target(&options.output)?;
        for (protected, label) in [
            (input.as_path(), "AnalysisBundle"),
            (config.as_path(), "configuration file"),
            (info.path.as_path(), "plugin directory"),
        ] {
            if output_target.starts_with(protected) || protected.starts_with(&output_target) {
                return Err(PlatformError::Output(format!(
                    "output must not overlap the {label}: {}",
                    protected.display()
                )));
            }
        }
        let output = prepare_output(&options.output, options.overwrite)?;
        let program = resolve_program(&info.path, &manifest.entrypoint.program)?;
        let stdout_path = output.join(".lemon-plugin-stdout.log");
        let stderr_path = output.join(".lemon-plugin-stderr.log");
        let stdout = File::create(&stdout_path).map_err(|source| PlatformError::Io {
            context: format!("create {}", stdout_path.display()),
            source,
        })?;
        let stderr = File::create(&stderr_path).map_err(|source| PlatformError::Io {
            context: format!("create {}", stderr_path.display()),
            source,
        })?;
        let mut command = Command::new(program);
        command
            .current_dir(&info.path)
            .env_clear()
            .env("LEMON_PLUGIN_API", PLUGIN_API_VERSION.to_string())
            .args(&manifest.entrypoint.args)
            .arg("--input")
            .arg(&input)
            .arg("--config")
            .arg(&config)
            .arg("--output")
            .arg(&output)
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr));
        let mut child = command.spawn().map_err(|e| {
            PlatformError::Execution(format!("could not start {:?}: {e}", manifest.id))
        })?;
        let started = Instant::now();
        let status = loop {
            if let Some(status) = child
                .try_wait()
                .map_err(|e| PlatformError::Execution(format!("wait for {:?}: {e}", manifest.id)))?
            {
                break status;
            }
            if started.elapsed() >= options.timeout {
                let _ = child.kill();
                let _ = child.wait();
                return Err(PlatformError::Execution(format!(
                    "plugin {:?} exceeded timeout of {} seconds",
                    manifest.id,
                    options.timeout.as_secs_f64()
                )));
            }
            thread::sleep(Duration::from_millis(20));
        };
        if snapshot_bundle(bundle.directory())? != before {
            return Err(PlatformError::Bundle(
                "plugin modified the AnalysisBundle".into(),
            ));
        }
        if !status.success() {
            return Err(PlatformError::Execution(format!(
                "plugin {:?} exited with {status}; diagnostics: {} and {}",
                manifest.id,
                stdout_path.display(),
                stderr_path.display()
            )));
        }
        let result = validate_result(&output, &manifest)?;
        Ok(RunReport {
            manifest,
            result,
            output,
        })
    }
    fn save(&self) -> Result<()> {
        let parent = self
            .registry_path
            .parent()
            .ok_or_else(|| PlatformError::Registry("registry path has no parent".into()))?;
        fs::create_dir_all(parent).map_err(|source| PlatformError::Io {
            context: format!("create registry directory {}", parent.display()),
            source,
        })?;
        let temp = parent.join(".plugins.json.tmp");
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&temp)
            .map_err(|source| PlatformError::Io {
                context: format!("write registry {}", temp.display()),
                source,
            })?;
        serde_json::to_writer_pretty(&file, &self.registry)
            .map_err(|e| PlatformError::Registry(e.to_string()))?;
        file.sync_all().map_err(|source| PlatformError::Io {
            context: "sync plugin registry".into(),
            source,
        })?;
        fs::rename(&temp, &self.registry_path).map_err(|source| PlatformError::Io {
            context: format!("publish registry {}", self.registry_path.display()),
            source,
        })
    }
}

pub fn default_registry_path() -> Result<PathBuf> {
    directories::ProjectDirs::from("", "", "LEMON")
        .map(|p| p.data_local_dir().join("plugins.json"))
        .ok_or_else(|| {
            PlatformError::Registry("cannot determine the LEMON application data directory".into())
        })
}
fn canonical_directory(path: &Path) -> Result<PathBuf> {
    let p = fs::canonicalize(path)
        .map_err(|e| PlatformError::Manifest(format!("{}: {e}", path.display())))?;
    if !p.is_dir() {
        return Err(PlatformError::Manifest(format!(
            "{} is not a directory",
            p.display()
        )));
    }
    Ok(p)
}
pub fn validate_plugin_directory(directory: &Path) -> Result<PluginManifest> {
    let manifest_path = directory.join("plugin.json");
    let manifest: PluginManifest =
        read_limited_json(&manifest_path).map_err(PlatformError::Manifest)?;
    validate_manifest(&manifest)?;
    let _ = resolve_program(directory, &manifest.entrypoint.program)?;
    Ok(manifest)
}
fn validate_manifest(manifest: &PluginManifest) -> Result<()> {
    if manifest.schema_version != MANIFEST_SCHEMA_VERSION {
        return Err(PlatformError::Manifest(format!(
            "unsupported schema_version {}; expected {MANIFEST_SCHEMA_VERSION}",
            manifest.schema_version
        )));
    }
    if manifest.api_version != PLUGIN_API_VERSION {
        return Err(PlatformError::Manifest(format!(
            "incompatible api_version {}; LEMON supports {PLUGIN_API_VERSION}",
            manifest.api_version
        )));
    }
    validate_id(&manifest.id).map_err(PlatformError::Manifest)?;
    if manifest.display_name.trim().is_empty() || manifest.display_name.len() > 256 {
        return Err(PlatformError::Manifest(
            "display_name must contain 1..256 bytes".into(),
        ));
    }
    if manifest.version.trim().is_empty() || manifest.version.len() > 64 {
        return Err(PlatformError::Manifest(
            "version must contain 1..64 bytes".into(),
        ));
    }
    if manifest.description.len() > 4096 {
        return Err(PlatformError::Manifest(
            "description exceeds 4096 bytes".into(),
        ));
    }
    if manifest.entrypoint.args.len() > 64
        || manifest
            .entrypoint
            .args
            .iter()
            .any(|a| a.len() > 4096 || a.contains('\0'))
    {
        return Err(PlatformError::Manifest(
            "entrypoint args exceed safety limits".into(),
        ));
    }
    Ok(())
}
fn validate_id(id: &str) -> std::result::Result<(), String> {
    if id.is_empty()
        || id.len() > 64
        || id.starts_with('-')
        || id.ends_with('-')
        || !id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(format!(
            "invalid plugin ID {id:?}; use lowercase ASCII letters, digits and hyphens"
        ));
    }
    Ok(())
}
fn resolve_program(root: &Path, value: &str) -> Result<PathBuf> {
    let relative = Path::new(value);
    if value.is_empty()
        || relative.is_absolute()
        || relative
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(PlatformError::Manifest(
            "entrypoint program must be a relative path inside the plugin directory".into(),
        ));
    }
    let canonical_root = fs::canonicalize(root)
        .map_err(|e| PlatformError::Manifest(format!("plugin directory: {e}")))?;
    let program = fs::canonicalize(root.join(relative))
        .map_err(|e| PlatformError::Manifest(format!("entrypoint {value:?}: {e}")))?;
    if !program.starts_with(&canonical_root) || !program.is_file() {
        return Err(PlatformError::Manifest(
            "entrypoint resolves outside the plugin directory or is not a file".into(),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if program
            .metadata()
            .map_err(|e| PlatformError::Manifest(e.to_string()))?
            .permissions()
            .mode()
            & 0o111
            == 0
        {
            return Err(PlatformError::Manifest(
                "entrypoint is not executable".into(),
            ));
        }
    }
    Ok(program)
}
fn check_entry(entry: &RegistryEntry) -> PluginInfo {
    let checked_at = SystemTime::now();
    if !entry.enabled {
        return PluginInfo {
            id: entry.id.clone(),
            path: entry.path.clone(),
            enabled: false,
            state: PluginState::Disabled,
            manifest: read_manifest_without_entrypoint(&entry.path).ok(),
            reason: None,
            checked_at,
        };
    }
    match validate_plugin_directory(&entry.path) {
        Ok(manifest) if manifest.id != entry.id => PluginInfo {
            id: entry.id.clone(),
            path: entry.path.clone(),
            enabled: true,
            state: PluginState::Unavailable,
            manifest: Some(manifest),
            reason: Some("plugin.json ID differs from the registered ID".into()),
            checked_at,
        },
        Ok(manifest) => PluginInfo {
            id: entry.id.clone(),
            path: entry.path.clone(),
            enabled: true,
            state: PluginState::Ready,
            manifest: Some(manifest),
            reason: None,
            checked_at,
        },
        Err(PlatformError::Manifest(reason)) if reason.contains("incompatible api_version") => {
            PluginInfo {
                id: entry.id.clone(),
                path: entry.path.clone(),
                enabled: true,
                state: PluginState::Incompatible,
                manifest: read_manifest_without_entrypoint(&entry.path).ok(),
                reason: Some(reason),
                checked_at,
            }
        }
        Err(e) => PluginInfo {
            id: entry.id.clone(),
            path: entry.path.clone(),
            enabled: true,
            state: PluginState::Unavailable,
            manifest: read_manifest_without_entrypoint(&entry.path).ok(),
            reason: Some(e.to_string()),
            checked_at,
        },
    }
}
fn read_manifest_without_entrypoint(root: &Path) -> std::result::Result<PluginManifest, String> {
    read_limited_json(&root.join("plugin.json"))
}
fn read_limited_json<T: for<'de> Deserialize<'de>>(path: &Path) -> std::result::Result<T, String> {
    let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let len = file.metadata().map_err(|e| e.to_string())?.len();
    if len > MAX_JSON_BYTES {
        return Err(format!("{} exceeds 1 MiB", path.display()));
    }
    serde_json::from_reader(file).map_err(|e| format!("JSON {}: {e}", path.display()))
}
fn validate_config(path: &Path) -> Result<PathBuf> {
    let path = fs::canonicalize(path)
        .map_err(|e| PlatformError::Config(format!("{}: {e}", path.display())))?;
    let _: serde_json::Value = read_limited_json(&path).map_err(PlatformError::Config)?;
    Ok(path)
}
fn prepare_output(path: &Path, overwrite: bool) -> Result<PathBuf> {
    let absolute = resolved_target(path)?;
    if absolute.parent().is_none() || absolute == Path::new("/") {
        return Err(PlatformError::Output("refusing unsafe output path".into()));
    }
    let current = fs::canonicalize(
        std::env::current_dir().map_err(|e| PlatformError::Output(e.to_string()))?,
    )
    .map_err(|e| PlatformError::Output(e.to_string()))?;
    if let Ok(existing) = fs::canonicalize(&absolute) {
        let home = directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf());
        if current.starts_with(&existing) || home.as_ref().is_some_and(|home| home == &existing) {
            return Err(PlatformError::Output(format!(
                "refusing to clear broad output directory {}",
                existing.display()
            )));
        }
    }
    if absolute.symlink_metadata().is_ok() {
        if absolute
            .symlink_metadata()
            .map_err(|e| PlatformError::Output(e.to_string()))?
            .file_type()
            .is_symlink()
        {
            return Err(PlatformError::Output("output must not be a symlink".into()));
        }
        if !absolute.is_dir() {
            return Err(PlatformError::Output(
                "output exists and is not a directory".into(),
            ));
        }
        let nonempty = fs::read_dir(&absolute)
            .map_err(|e| PlatformError::Output(e.to_string()))?
            .next()
            .is_some();
        if nonempty && !overwrite {
            return Err(PlatformError::Output("directory exists and is not empty; pass --overwrite to clear this exact output directory".into()));
        }
        if nonempty {
            fs::remove_dir_all(&absolute)
                .map_err(|e| PlatformError::Output(format!("clear {}: {e}", absolute.display())))?;
        }
    }
    fs::create_dir_all(&absolute)
        .map_err(|e| PlatformError::Output(format!("create {}: {e}", absolute.display())))?;
    fs::canonicalize(&absolute).map_err(|e| PlatformError::Output(e.to_string()))
}
fn resolved_target(path: &Path) -> Result<PathBuf> {
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(PlatformError::Output(
            "output path must not contain '..'".into(),
        ));
    }
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| PlatformError::Output(e.to_string()))?
            .join(path)
    };
    if absolute.exists() {
        return fs::canonicalize(&absolute).map_err(|e| PlatformError::Output(e.to_string()));
    }
    let mut ancestor = absolute.as_path();
    let mut suffix = Vec::new();
    while !ancestor.exists() {
        let name = ancestor
            .file_name()
            .ok_or_else(|| PlatformError::Output("invalid output path".into()))?;
        suffix.push(name.to_os_string());
        ancestor = ancestor
            .parent()
            .ok_or_else(|| PlatformError::Output("invalid output path".into()))?;
    }
    let mut resolved =
        fs::canonicalize(ancestor).map_err(|e| PlatformError::Output(e.to_string()))?;
    for name in suffix.into_iter().rev() {
        resolved.push(name);
    }
    Ok(resolved)
}
fn validate_result(output: &Path, manifest: &PluginManifest) -> Result<PluginResult> {
    let path = output.join("plugin-result.json");
    let result: PluginResult = read_limited_json(&path).map_err(PlatformError::Result)?;
    if result.schema_version != RESULT_SCHEMA_VERSION {
        return Err(PlatformError::Result(format!(
            "unsupported schema_version {}",
            result.schema_version
        )));
    }
    if result.plugin.id != manifest.id || result.plugin.version != manifest.version {
        return Err(PlatformError::Result(
            "plugin ID or version does not match plugin.json".into(),
        ));
    }
    if result.artifacts.len() > MAX_ARTIFACTS {
        return Err(PlatformError::Result(format!(
            "more than {MAX_ARTIFACTS} artifacts"
        )));
    }
    let canonical_output =
        fs::canonicalize(output).map_err(|e| PlatformError::Result(e.to_string()))?;
    for artifact in &result.artifacts {
        validate_artifact(&canonical_output, artifact)?;
    }
    if result.status == PluginStatus::Failed
        && result.error.as_deref().unwrap_or("").trim().is_empty()
    {
        return Err(PlatformError::Result(
            "failed status requires a non-empty error".into(),
        ));
    }
    Ok(result)
}
fn validate_artifact(output: &Path, artifact: &Artifact) -> Result<()> {
    let relative = Path::new(&artifact.path);
    if artifact.path.is_empty()
        || relative.is_absolute()
        || relative
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(PlatformError::Result(format!(
            "invalid artifact path {:?}",
            artifact.path
        )));
    }
    let path = fs::canonicalize(output.join(relative))
        .map_err(|e| PlatformError::Result(format!("artifact {:?}: {e}", artifact.path)))?;
    if !path.starts_with(output) || !path.is_file() {
        return Err(PlatformError::Result(format!(
            "artifact {:?} is outside output or is not a file",
            artifact.path
        )));
    }
    Ok(())
}
fn snapshot_bundle(root: &Path) -> Result<BTreeMap<String, (u64, u64)>> {
    let mut map = BTreeMap::new();
    for name in [
        "manifest.json",
        "summary.json",
        "summary.md",
        "processed.csv",
        "spectrum.csv",
        "band-power.csv",
        "quality-events.csv",
    ] {
        let path = root.join(name);
        let mut file = File::open(&path)
            .map_err(|e| PlatformError::Bundle(format!("{}: {e}", path.display())))?;
        let mut hasher = DefaultHasher::new();
        let mut size = 0;
        let mut buf = [0u8; 65536];
        loop {
            let n = file
                .read(&mut buf)
                .map_err(|e| PlatformError::Bundle(e.to_string()))?;
            if n == 0 {
                break;
            }
            size += n as u64;
            buf[..n].hash(&mut hasher);
        }
        map.insert(name.into(), (size, hasher.finish()));
    }
    Ok(map)
}
pub fn checked_at_text(time: SystemTime) -> String {
    format!(
        "{} Unix seconds",
        time.duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn temp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("lemon-plugins-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).expect("temp");
        p
    }
    fn executable(root: &Path, body: &str) -> PathBuf {
        let path = root.join("plugin");
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("script");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&path).expect("meta").permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&path, permissions).expect("chmod");
        }
        path
    }
    fn manifest(root: &Path, id: &str, api: u32) {
        fs::write(root.join("plugin.json"),format!(r#"{{"schema_version":1,"id":"{id}","display_name":"Fixture","version":"0.1.0","api_version":{api},"description":"test","entrypoint":{{"program":"plugin","args":[]}}}}"#)).expect("manifest");
    }
    #[test]
    fn registry_lifecycle_and_inspection() {
        let root = temp("registry");
        let plugin = root.join("p");
        fs::create_dir(&plugin).expect("dir");
        executable(&plugin, "exit 0");
        manifest(&plugin, "fixture", 1);
        let mut p = Platform::open(root.join("plugins.json")).expect("open");
        assert_eq!(p.add(&plugin).expect("add").state, PluginState::Ready);
        assert!(p.add(&plugin).is_err());
        assert_eq!(p.list().len(), 1);
        assert_eq!(
            p.set_enabled("fixture", false).expect("disable").state,
            PluginState::Disabled
        );
        assert_eq!(
            p.set_enabled("fixture", true).expect("enable").state,
            PluginState::Ready
        );
        p.remove("fixture").expect("remove");
        assert!(p.list().is_empty());
        let _ = fs::remove_dir_all(root);
    }
    #[test]
    fn rejects_bad_manifest_and_external_entrypoint() {
        let root = temp("bad");
        let p = root.join("p");
        fs::create_dir(&p).expect("dir");
        executable(&p, "exit 0");
        manifest(&p, "Bad_ID", 1);
        assert!(validate_plugin_directory(&p).is_err());
        manifest(&p, "fixture", 99);
        assert!(validate_plugin_directory(&p).is_err());
        manifest(&p, "fixture", 1);
        let value = fs::read_to_string(p.join("plugin.json"))
            .expect("read")
            .replace("\"plugin\"", "\"../outside\"");
        fs::write(p.join("plugin.json"), value).expect("write");
        assert!(validate_plugin_directory(&p).is_err());
        let _ = fs::remove_dir_all(root);
    }
    #[cfg(unix)]
    #[test]
    fn rejects_symlink_entrypoint_outside() {
        use std::os::unix::fs::symlink;
        let root = temp("link");
        let p = root.join("p");
        fs::create_dir(&p).expect("dir");
        let outside = executable(&root, "exit 0");
        symlink(outside, p.join("plugin")).expect("link");
        manifest(&p, "fixture", 1);
        assert!(validate_plugin_directory(&p).is_err());
        let _ = fs::remove_dir_all(root);
    }
    #[test]
    fn unavailable_registry_does_not_prevent_loading() {
        let root = temp("missing");
        fs::write(root.join("plugins.json"),format!(r#"{{"schema_version":1,"plugins":[{{"id":"missing","path":"{}","enabled":true}}]}}"#,root.join("gone").display())).expect("registry");
        let p = Platform::open(root.join("plugins.json")).expect("open");
        assert_eq!(p.list()[0].state, PluginState::Unavailable);
        let _ = fs::remove_dir_all(root);
    }
    #[test]
    fn incompatible_registry_entry_has_distinct_state() {
        let root = temp("incompatible");
        let plugin = root.join("plugin");
        fs::create_dir(&plugin).expect("plugin");
        executable(&plugin, "exit 0");
        manifest(&plugin, "future", 99);
        fs::write(root.join("plugins.json"),format!(r#"{{"schema_version":1,"plugins":[{{"id":"future","path":"{}","enabled":true}}]}}"#,plugin.display())).expect("registry");
        let platform = Platform::open(root.join("plugins.json")).expect("open");
        assert_eq!(platform.list()[0].state, PluginState::Incompatible);
        let _ = fs::remove_dir_all(root);
    }
    #[test]
    fn malformed_registry_is_reported() {
        let root = temp("json");
        fs::write(root.join("plugins.json"), "{").expect("write");
        assert!(Platform::open(root.join("plugins.json")).is_err());
        let _ = fs::remove_dir_all(root);
    }
    fn bundle(root: &Path) -> PathBuf {
        let p = root.join("analysis");
        fs::create_dir(&p).expect("bundle");
        fs::write(p.join("manifest.json"),r#"{"analysis_format_version":1,"lemon_version":"0.2.0","created_at":"0","source":"fixture","source_format":"csv","source_path":"fixture.csv","channels":["A0"],"units":"a.u.","sample_rate_hz":250.0,"sample_rate_source":"confirmed","duration_seconds":1.0,"processing_config":{},"spectral_config":{},"configured_bands":[]}"#).expect("manifest");
        fs::write(p.join("summary.json"),r#"{"duration_seconds":1.0,"channel_count":1,"sample_count_by_channel":{"A0":1},"valid_filtered_count_by_channel":{"A0":1},"missing_count_by_channel":{"A0":0},"lost_sample_count":0,"invalid_packet_count":0,"outlier_count":0,"saturation_count":0,"disconnection_count":0,"continuous_segment_count":1,"sample_rate_hz":250.0,"sample_rate_source":"confirmed","channels":{},"warnings":[],"dominant_band_by_channel":{}}"#).expect("summary");
        fs::write(p.join("summary.md"), "fixture").expect("md");
        for (name,head) in [("processed.csv","timestamp_ns,sequence,channel,raw,filtered,quality,flags\n"),("spectrum.csv","window_index,window_start_ns,window_end_ns,channel,frequency_hz,raw_psd,filtered_psd\n"),("band-power.csv","window_index,window_start_ns,window_end_ns,channel,band,low_hz,high_hz,absolute,relative,smoothed,baseline_change_pct\n"),("quality-events.csv","timestamp_ns,channel,kind,severity,details\n")]{fs::write(p.join(name),head).expect("csv");}
        p
    }
    #[cfg(unix)]
    #[test]
    fn runs_process_validates_result_and_preserves_input() {
        let root = temp("run");
        let plugin = root.join("plugin-dir");
        fs::create_dir(&plugin).expect("plugin");
        executable(
            &plugin,
            r#"while [ "$#" -gt 0 ]; do case "$1" in --output) out="$2"; shift 2;; *) shift 2;; esac; done
printf '{"ok":true}' > "$out/summary.json"
printf '{"schema_version":1,"plugin":{"id":"fixture","version":"0.1.0"},"status":"completed","artifacts":[{"path":"summary.json","kind":"data","description":"fixture"}],"warnings":[]}' > "$out/plugin-result.json""#,
        );
        manifest(&plugin, "fixture", 1);
        let input = bundle(&root);
        let config = root.join("config.json");
        fs::write(&config, "{}").expect("config");
        let mut platform = Platform::open(root.join("registry.json")).expect("open");
        platform.add(&plugin).expect("add");
        let report = platform
            .run(&RunOptions {
                plugin: "fixture".into(),
                input: input.clone(),
                config,
                output: root.join("output"),
                overwrite: false,
                timeout: Duration::from_secs(2),
            })
            .expect("run");
        assert_eq!(report.result.status, PluginStatus::Completed);
        assert!(input.join("manifest.json").is_file());
        let _ = fs::remove_dir_all(root);
    }
    #[cfg(unix)]
    #[test]
    fn reports_timeout_and_missing_result() {
        let root = temp("failures");
        let input = bundle(&root);
        let config = root.join("config.json");
        fs::write(&config, "{}").expect("config");
        let slow = root.join("slow");
        fs::create_dir(&slow).expect("slow");
        executable(&slow, "sleep 2");
        manifest(&slow, "slow", 1);
        let platform = Platform::open(root.join("registry.json")).expect("open");
        assert!(platform
            .run(&RunOptions {
                plugin: slow.to_string_lossy().into(),
                input: input.clone(),
                config: config.clone(),
                output: root.join("timeout"),
                overwrite: false,
                timeout: Duration::from_millis(20)
            })
            .expect_err("timeout")
            .to_string()
            .contains("timeout"));
        let empty = root.join("empty");
        fs::create_dir(&empty).expect("empty");
        executable(&empty, "exit 0");
        manifest(&empty, "empty", 1);
        assert!(platform
            .run(&RunOptions {
                plugin: empty.to_string_lossy().into(),
                input,
                config,
                output: root.join("empty-output"),
                overwrite: false,
                timeout: Duration::from_secs(1)
            })
            .expect_err("result")
            .to_string()
            .contains("plugin-result.json"));
        let _ = fs::remove_dir_all(root);
    }
    #[cfg(unix)]
    #[test]
    fn rejects_crash_mismatched_identity_and_artifacts() {
        let root = temp("invalid-results");
        let input = bundle(&root);
        let config = root.join("config.json");
        fs::write(&config, "{}").expect("config");
        let platform = Platform::open(root.join("registry.json")).expect("open");

        let crash = root.join("crash");
        fs::create_dir(&crash).expect("crash");
        executable(&crash, "exit 7");
        manifest(&crash, "crash", 1);
        let error = platform
            .run(&RunOptions {
                plugin: crash.to_string_lossy().into(),
                input: input.clone(),
                config: config.clone(),
                output: root.join("crash-output"),
                overwrite: false,
                timeout: Duration::from_secs(1),
            })
            .expect_err("crash");
        assert!(error.to_string().contains("exited"));

        for (name, result, expected) in [
            (
                "identity",
                r#"{"schema_version":1,"plugin":{"id":"other","version":"0.1.0"},"status":"completed","artifacts":[],"warnings":[]}"#,
                "ID or version",
            ),
            (
                "traversal",
                r#"{"schema_version":1,"plugin":{"id":"traversal","version":"0.1.0"},"status":"completed","artifacts":[{"path":"../outside","kind":"data","description":"bad"}],"warnings":[]}"#,
                "invalid artifact path",
            ),
            (
                "missing-artifact",
                r#"{"schema_version":1,"plugin":{"id":"missing-artifact","version":"0.1.0"},"status":"completed","artifacts":[{"path":"missing.json","kind":"data","description":"bad"}],"warnings":[]}"#,
                "artifact",
            ),
        ] {
            let plugin = root.join(name);
            fs::create_dir(&plugin).expect("plugin");
            executable(
                &plugin,
                &format!(
                    r#"while [ "$#" -gt 0 ]; do case "$1" in --output) out="$2"; shift 2;; *) shift 2;; esac; done
printf '%s' '{}' > "$out/plugin-result.json""#,
                    result
                ),
            );
            manifest(&plugin, name, 1);
            let error = platform
                .run(&RunOptions {
                    plugin: plugin.to_string_lossy().into(),
                    input: input.clone(),
                    config: config.clone(),
                    output: root.join(format!("{name}-output")),
                    overwrite: false,
                    timeout: Duration::from_secs(1),
                })
                .expect_err("invalid result");
            assert!(error.to_string().contains(expected), "{error}");
        }
        let _ = fs::remove_dir_all(root);
    }
    #[test]
    fn output_requires_overwrite_and_only_clears_selected_directory() {
        let root = temp("overwrite");
        let output = root.join("output");
        fs::create_dir(&output).expect("output");
        fs::write(output.join("old"), "old").expect("old");
        fs::write(root.join("keep"), "keep").expect("keep");
        assert!(prepare_output(&output, false).is_err());
        let prepared = prepare_output(&output, true).expect("overwrite");
        assert!(prepared.is_dir());
        assert!(!prepared.join("old").exists());
        assert!(root.join("keep").is_file());
        let _ = fs::remove_dir_all(root);
    }
    #[test]
    fn resolved_output_target_rejects_parent_traversal() {
        let root = temp("target");
        assert!(resolved_target(&root.join("new/../result")).is_err());
        let _ = fs::remove_dir_all(root);
    }
}
