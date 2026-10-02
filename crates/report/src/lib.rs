//! TUI-independent renderer for an `analysis` AnalysisBundle.
mod dataset;
pub mod environment;
mod packaging;
use analysis::AnalysisBundle;
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
    time::{SystemTime, UNIX_EPOCH},
};

pub type ReportError = String;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionMode {
    Auto,
    Execute,
    NoExecute,
}
#[derive(Debug, Clone)]
pub struct ReportOptions {
    pub output: Option<PathBuf>,
    pub execution: ExecutionMode,
    pub python: Option<PathBuf>,
    pub overwrite: bool,
    pub open: bool,
    pub portable: bool,
    pub colab: bool,
    pub include_raw: bool,
    pub maximum_embedded_report_mb: u64,
}
#[derive(Debug, Clone, Serialize)]
pub struct ReportArtifacts {
    pub directory: PathBuf,
    pub notebook: PathBuf,
    pub manifest: PathBuf,
    pub execution_status: String,
    pub warning: Option<String>,
    pub archive: Option<PathBuf>,
}

#[derive(Serialize)]
struct Cell {
    id: String,
    cell_type: String,
    metadata: Value,
    source: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    outputs: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    execution_count: Option<Value>,
}
#[derive(Serialize)]
struct Notebook {
    cells: Vec<Cell>,
    metadata: Value,
    nbformat: u32,
    nbformat_minor: u32,
}
fn cell(kind: &str, text: &str) -> Cell {
    Cell {
        id: String::new(),
        cell_type: kind.into(),
        metadata: json!({}),
        source: text.lines().map(|x| format!("{x}\n")).collect(),
        outputs: (kind == "code").then(Vec::new),
        execution_count: (kind == "code").then_some(Value::Null),
    }
}
fn notebook() -> Notebook {
    let mut cells = vec![cell("markdown", "# LEMON Signal Report\n\n**LEMON — Lightweight EEG Monitoring Of Neuroactivity**\n\nThis report presents numerical characteristics and visualizations of the recorded signal. Neurophysiological, cognitive and medical interpretation must be performed by a qualified specialist."), cell("code", include_str!("../python/presentation.py")), cell("code", "initialize()")];
    for (i, (title, call)) in [
        ("Recording overview", "overview()"),
        ("Import and data provenance", "provenance()"),
        ("Processing performed", "processing()"),
        ("Raw signal", "waveform('raw')"),
        ("Filtered signal", "waveform('filtered')"),
        ("Raw and filtered comparison", "comparison()"),
        ("Power spectral density", "spectrum()"),
        ("Frequency-band power", "bands()"),
        ("Signal continuity and quality", "quality()"),
        ("Technical summary", "technical_summary()"),
        ("Warnings and limitations", "warnings()"),
    ]
    .iter()
    .enumerate()
    {
        cells.push(cell("markdown", &format!("## {}. {}", i + 1, title)));
        cells.push(cell("code", call));
    }
    for (i, c) in cells.iter_mut().enumerate() {
        c.id = format!("lemon-{i}");
    }
    Notebook {
        cells,
        metadata: json!({"kernelspec":{"display_name":"Python 3","language":"python","name":"python3"},"language_info":{"name":"python"}}),
        nbformat: 4,
        nbformat_minor: 5,
    }
}

#[derive(Serialize)]
struct ReportManifest {
    report_format_version: u32,
    lemon_version: String,
    notebook_nbformat: u32,
    created_at: String,
    analysis_manifest_path: String,
    analysis_hash: String,
    execution_mode: String,
    execution_status: String,
    python_executable: Option<String>,
    python_version: Option<String>,
    dependency_versions: Vec<String>,
    generated_figures: Vec<String>,
    warnings: Vec<String>,
}
fn hash_manifest(path: &Path) -> Result<String, ReportError> {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    for name in [
        "manifest.json",
        "summary.json",
        "summary.md",
        "processed.csv",
        "spectrum.csv",
        "band-power.csv",
        "quality-events.csv",
    ] {
        h.update(name.as_bytes());
        let mut f =
            fs::File::open(path.join(name)).map_err(|e| format!("report/hash {name}: {e}"))?;
        let mut buf = [0; 65536];
        loop {
            let n = f.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
        }
    }
    Ok(format!("{:x}", h.finalize()))
}
fn copy_analysis(src: &Path, dst: &Path) -> Result<(), ReportError> {
    fs::create_dir_all(dst).map_err(|e| e.to_string())?;
    for name in [
        "manifest.json",
        "summary.json",
        "summary.md",
        "processed.csv",
        "spectrum.csv",
        "band-power.csv",
        "quality-events.csv",
    ] {
        let from = src.join(name);
        fs::copy(&from, dst.join(name))
            .map_err(|e| format!("report/analysis {}: {e}", from.display()))?;
    }
    Ok(())
}
fn python_info(p: &Path) -> Result<(String, Vec<String>), ReportError> {
    let text = environment::run(Command::new(p).args(["-c", r#"import sys; assert sys.version_info >= (3,10), 'Python >= 3.10 required'
import nbformat,nbclient,pandas,numpy,matplotlib,ipykernel,json,importlib.metadata as m
print(json.dumps([sys.version.split()[0]]+[n+"="+m.version(n) for n in ["nbformat","nbclient","pandas","numpy","matplotlib","ipykernel","jupyter"]]))"#]), Duration::from_secs(30))?;
    let info: Vec<String> = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    Ok((
        info.first().cloned().unwrap_or_default(),
        info.into_iter().skip(1).collect(),
    ))
}
fn execute(p: &Path, nb: &Path, cwd: &Path) -> Result<(), ReportError> {
    let runner = r#"import sys,nbformat,os
from nbclient import NotebookClient
from jupyter_client import KernelManager
path=sys.argv[1]
with open(path,encoding='utf-8') as f: book=nbformat.read(f,as_version=4)
nbformat.validate(book)
km=KernelManager(kernel_name='python3')
km.kernel_spec.argv=[sys.executable, '-m', 'ipykernel_launcher', '-f', '{connection_file}']
try:
    NotebookClient(book,timeout=120,km=km,resources={'metadata':{'path':sys.argv[2]}}).execute()
finally:
    if km.has_kernel: km.shutdown_kernel(now=True)
nbformat.validate(book)
tmp=path+'.executed'
with open(tmp,'w',encoding='utf-8') as f: nbformat.write(book,f)
os.replace(tmp,path)"#;
    let log_path = cwd.join("execution.log");
    let log = fs::File::create(&log_path).map_err(|e| e.to_string())?;
    let mut child = Command::new(p)
        .args(["-c", runner, &nb.to_string_lossy(), &cwd.to_string_lossy()])
        .env("MPLBACKEND", "Agg")
        .current_dir(cwd)
        .stdout(Stdio::from(log.try_clone().map_err(|e| e.to_string())?))
        .stderr(Stdio::from(log))
        .spawn()
        .map_err(|e| e.to_string())?;
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if started.elapsed() > Duration::from_secs(1800) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "report/execute: total timeout 1800 s; see {}",
                log_path.display()
            ));
        }
        thread::sleep(Duration::from_millis(50));
    };
    let mut diagnostic = String::new();
    fs::File::open(&log_path)
        .map_err(|e| e.to_string())?
        .take(65536)
        .read_to_string(&mut diagnostic)
        .map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{diagnostic}\nFull log: {}", log_path.display()))
    }
}
#[allow(clippy::too_many_arguments)]
fn write_manifest(
    output: &Path,
    analysis: &AnalysisBundle,
    status: &str,
    mode: &str,
    python: Option<&Path>,
    version: Option<String>,
    deps: Vec<String>,
    warnings: Vec<String>,
) -> Result<PathBuf, ReportError> {
    let path = output.join("report-manifest.json");
    let m = ReportManifest {
        report_format_version: 1,
        lemon_version: env!("CARGO_PKG_VERSION").into(),
        notebook_nbformat: 4,
        created_at: format!(
            "unix-ms:{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |x| x.as_millis())
        ),
        analysis_manifest_path: {
            let settings: Value = serde_json::from_slice(
                &fs::read(output.join("report-data.json")).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            format!(
                "{}/manifest.json",
                settings["analysis_path"].as_str().unwrap_or("analysis")
            )
        },
        analysis_hash: hash_manifest(&analysis.directory)?,
        execution_mode: mode.into(),
        execution_status: status.into(),
        python_executable: python.map(|x| x.display().to_string()),
        python_version: version,
        dependency_versions: deps,
        generated_figures: [
            "raw-overview.png",
            "filtered-overview.png",
            "raw-filtered-comparison.png",
            "spectrum.png",
            "band-power-history.png",
            "quality-timeline.png",
        ]
        .iter()
        .filter(|name| output.join("figures").join(name).is_file())
        .map(|x| (*x).into())
        .collect(),
        warnings,
    };
    let tmp = path.with_extension("json.partial");
    fs::write(
        &tmp,
        serde_json::to_vec_pretty(&m).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    fs::rename(tmp, &path).map_err(|e| e.to_string())?;
    Ok(path)
}
fn report_dir(analysis: &AnalysisBundle, requested: Option<&Path>) -> PathBuf {
    requested.map(Path::to_path_buf).unwrap_or_else(|| {
        PathBuf::from(format!(
            "report-{}",
            analysis
                .directory
                .file_name()
                .and_then(|x| x.to_str())
                .unwrap_or("analysis")
        ))
    })
}

pub fn render_notebook(
    analysis: &AnalysisBundle,
    options: ReportOptions,
) -> Result<ReportArtifacts, ReportError> {
    AnalysisBundle::open(&analysis.directory)?;
    if options.portable && options.colab {
        return Err("report: --portable conflicts with --colab".into());
    }
    if options.include_raw && !options.portable && !options.colab {
        return Err("report: --include-raw requires --portable or --colab".into());
    }
    if options.portable && options.open {
        return Err("report: --open is not applicable to portable ZIP".into());
    }
    let output = report_dir(analysis, options.output.as_deref());
    let archive_target = output.with_extension("lemon-report.zip");
    if options.portable && archive_target.exists() && !options.overwrite {
        return Err("report/archive: archive exists; use --overwrite".into());
    }
    let input = analysis
        .directory
        .canonicalize()
        .map_err(|e| e.to_string())?;
    // Resolve the nearest existing ancestor before creating any directory.
    let absolute = if output.is_absolute() {
        output.clone()
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(&output)
    };
    let mut ancestor = absolute.as_path();
    while !ancestor.exists() {
        ancestor = ancestor
            .parent()
            .ok_or("report/output: no existing ancestor")?;
    }
    if ancestor
        .canonicalize()
        .map_err(|e| e.to_string())?
        .starts_with(&input)
    {
        return Err("report/output: output must be outside the input AnalysisBundle".into());
    }
    if output.exists() {
        let target = output.canonicalize().map_err(|e| e.to_string())?;
        if input.starts_with(&target)
            || target.starts_with(&input)
            || !target.join("report-manifest.json").is_file()
        {
            return Err(
                "report/output: refusing to overwrite input or an unrelated directory".into(),
            );
        }
        let allowed = [
            "report.ipynb",
            "report-manifest.json",
            "report-requirements.txt",
            "report-data.json",
            "analysis",
            "figures",
            "execution.log",
            "report-colab.ipynb",
            "report-dataset.json",
            "colab-data.zip",
            "full-raw.zip",
            "processed.csv",
            "README.md",
            "SHA256SUMS.json",
        ];
        for entry in fs::read_dir(&target).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            let name = entry.file_name();
            let is_directory = matches!(name.to_str(), Some("analysis" | "figures"));
            if kind.is_symlink()
                || (is_directory && !kind.is_dir())
                || (!is_directory && !kind.is_file())
                || !allowed.contains(&name.to_string_lossy().as_ref())
            {
                return Err("report/output: unrelated files; choose a new output directory".into());
            }
        }
    }
    if output.exists() {
        if !options.overwrite {
            return Err(format!(
                "report/output: {} exists; use --overwrite",
                output.display()
            ));
        }
        for sub in ["analysis", "figures"] {
            let dir = output.join(sub);
            if dir.exists() {
                if fs::symlink_metadata(&dir)
                    .map_err(|e| e.to_string())?
                    .file_type()
                    .is_symlink()
                {
                    return Err("report/output: refusing symlink".into());
                }
                for e in fs::read_dir(&dir).map_err(|e| e.to_string())? {
                    let e = e.map_err(|e| e.to_string())?;
                    let allowed = if sub == "analysis" {
                        vec![
                            "manifest.json",
                            "summary.json",
                            "summary.md",
                            "processed.csv",
                            "spectrum.csv",
                            "band-power.csv",
                            "quality-events.csv",
                        ]
                    } else {
                        vec![
                            "raw-overview.png",
                            "filtered-overview.png",
                            "raw-filtered-comparison.png",
                            "spectrum.png",
                            "band-power-history.png",
                            "quality-timeline.png",
                        ]
                    };
                    if !e.file_type().map_err(|e| e.to_string())?.is_file()
                        || !allowed.contains(&e.file_name().to_string_lossy().as_ref())
                    {
                        return Err("report/output: unrelated files inside existing report".into());
                    }
                }
            }
        }
    }
    let destination = output.clone();
    let staging = ReportStaging::new(&destination)?;
    let output = staging.0.clone();
    fs::create_dir_all(output.join("figures")).map_err(|e| e.to_string())?;
    let output = output.canonicalize().map_err(|e| e.to_string())?;
    // Local and portable reports always contain their complete AnalysisBundle.
    if !options.colab {
        copy_analysis(&analysis.directory, &output.join("analysis"))?;
    }
    fs::write(
        output.join("report-data.json"),
        b"{\"analysis_path\":\"analysis\"}",
    )
    .map_err(|e| e.to_string())?;
    let mut book = notebook();
    let colab_metadata = if options.colab {
        Some(packaging::colab(
            &output,
            &analysis.directory,
            options.include_raw,
            options
                .maximum_embedded_report_mb
                .saturating_mul(1024 * 1024),
            &mut book,
        )?)
    } else {
        None
    };
    let notebook_path = output.join(if options.colab {
        "report-colab.ipynb"
    } else {
        "report.ipynb"
    });
    fs::write(
        &notebook_path,
        serde_json::to_vec_pretty(&book).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    fs::write(
        output.join("report-requirements.txt"),
        include_str!("../../../report-requirements.txt"),
    )
    .map_err(|e| e.to_string())?;
    let python = options
        .python
        .clone()
        .map(Ok)
        .unwrap_or_else(environment::managed_python)?;
    let python = interpreter_path(&python)?;
    let mode = match options.execution {
        ExecutionMode::Auto => "auto",
        ExecutionMode::Execute => "execute",
        ExecutionMode::NoExecute => "no-execute",
    };
    let mut status = "not_executed";
    let mut warning = None;
    let mut version = None;
    let mut deps = Vec::new();
    if !options.colab && !matches!(options.execution, ExecutionMode::NoExecute) {
        match python_info(&python) {
            Ok((v, d)) => {
                version = Some(v);
                deps = d;
                match execute(&python, &notebook_path, &output) {
                    Ok(()) => status = "executed",
                    Err(e) => {
                        status = "execution_failed";
                        warning = Some(e)
                    }
                }
            }
            Err(e) => {
                status = if matches!(options.execution, ExecutionMode::Auto) {
                    "not_executed"
                } else {
                    "execution_failed"
                };
                warning=Some(format!("Notebook generated but not executed. {e}\nLEMON report environment is not configured.\nRun: lemon report setup\nOr generate an unexecuted notebook: lemon report INPUT --no-execute\nFor an external Python install report-requirements.txt yourself."))
            }
        }
    }
    if matches!(options.execution, ExecutionMode::NoExecute) {
        warning = Some("Notebook generated but not executed. From the report directory: python -m pip install -r report-requirements.txt; jupyter nbconvert --execute --inplace report.ipynb".into());
    }
    if options.colab {
        let mut message = "Open report-colab.ipynb in Colab and select Runtime > Run all. No local Python is required.".to_string();
        if let Some(data) = &colab_metadata {
            for field in ["external_data_archive", "external_raw_archive"] {
                if let Some(name) = data[field].as_str() {
                    message.push_str(&format!(
                        "\nAdditional archive: {name} (in the report directory)."
                    ));
                }
            }
        }
        warning = Some(message);
    }
    let warnings = warning.clone().into_iter().collect::<Vec<_>>();
    let manifest = write_manifest(
        &output,
        analysis,
        status,
        mode,
        Some(&python),
        version,
        deps,
        warnings,
    )?;
    let mut manifest_data: Value =
        serde_json::from_slice(&fs::read(&manifest).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    manifest_data["report_mode"] = json!(if options.colab {
        "colab"
    } else if options.portable {
        "portable"
    } else {
        "local"
    });
    manifest_data["portable"] = json!(options.portable);
    manifest_data["colab"] = json!(options.colab);
    manifest_data["full_raw_included"] = json!(!options.colab);
    manifest_data["report_dataset_embedded"] = json!(false);
    manifest_data["external_data_archive"] = Value::Null;
    manifest_data["embedded_compressed_bytes"] = json!(0);
    manifest_data["embedded_uncompressed_bytes"] = json!(0);
    manifest_data["embedded_sha256"] = Value::Null;
    manifest_data["python_environment"] = json!(if options.python.is_some() {
        "external"
    } else {
        "managed"
    });
    if options.colab {
        manifest_data["analysis_manifest_path"] = json!("report-dataset.json#manifest");
    }
    if options.colab || matches!(options.execution, ExecutionMode::NoExecute) {
        manifest_data["python_environment"] = json!("not_used");
        manifest_data["python_executable"] = Value::Null;
    }
    manifest_data["notebook_hash"] = json!(packaging::hash(&notebook_path)?);
    if let Some(v) = colab_metadata {
        if let Some(fields) = v.as_object() {
            for (k, v) in fields {
                manifest_data[k] = v.clone();
            }
        }
    }
    fs::write(
        &manifest,
        serde_json::to_vec_pretty(&manifest_data).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let old_path = output.to_string_lossy().into_owned();
    staging.publish(&destination)?;
    let output = destination.canonicalize().map_err(|e| e.to_string())?;
    let notebook_path = output.join(if options.colab {
        "report-colab.ipynb"
    } else {
        "report.ipynb"
    });
    let manifest = output.join("report-manifest.json");
    if let Some(message) = &mut warning {
        *message = message.replace(&old_path, &output.to_string_lossy());
    }
    let archive = if options.portable {
        packaging::portable(&output, &archive_target, options.overwrite)?;
        Some(archive_target)
    } else {
        None
    };
    if !options.colab
        && !matches!(options.execution, ExecutionMode::NoExecute)
        && status != "executed"
    {
        return Err(format!(
            "report execution failed: {}\nGenerated notebook: {}",
            warning.unwrap_or_default(),
            notebook_path.display()
        ));
    }
    Ok(ReportArtifacts {
        directory: output.clone(),
        notebook: notebook_path,
        manifest,
        execution_status: status.into(),
        warning,
        archive,
    })
}

struct ReportStaging(PathBuf);
impl ReportStaging {
    fn new(target: &Path) -> Result<Self, String> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let parent = target
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        for _ in 0..1000 {
            let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = parent.join(format!(".lemon-report-{}-{n}.partial", std::process::id()));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.to_string()),
            }
        }
        Err("report/output: cannot allocate staging directory".into())
    }
    fn publish(&self, destination: &Path) -> Result<(), String> {
        let backup = if destination.exists() {
            let container = Self::new(destination)?;
            fs::rename(destination, container.0.join("previous")).map_err(|e| e.to_string())?;
            Some(container)
        } else {
            None
        };
        if let Err(e) = fs::rename(&self.0, destination) {
            if let Some(backup) = backup {
                if fs::rename(backup.0.join("previous"), destination).is_err() {
                    let path = backup.0.display().to_string();
                    // Preserve the only copy if restoration itself fails.
                    std::mem::forget(backup);
                    return Err(format!(
                        "report/output: {e}; previous report retained in {path}"
                    ));
                }
            }
            return Err(e.to_string());
        }
        Ok(())
    }
}
impl Drop for ReportStaging {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

// Resolving symlinks here would turn venv/bin/python into the system interpreter
// and lose the virtual environment's site-packages.
fn interpreter_path(path: &Path) -> Result<PathBuf, String> {
    if path.is_relative() && path.components().count() > 1 {
        Ok(std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(path))
    } else {
        Ok(path.to_path_buf())
    }
}
#[cfg(test)]
mod interpreter_tests {
    use super::*;
    #[test]
    fn temporary_venv_keeps_its_own_prefix_without_installing_packages() {
        let temp =
            ReportStaging::new(&std::env::temp_dir().join("lemon-venv-check")).expect("temp");
        let venv = temp.0.join("env with spaces");
        if environment::run(
            Command::new("python3")
                .args(["-m", "venv", "--without-pip"])
                .arg(&venv),
            Duration::from_secs(60),
        )
        .is_err()
        {
            eprintln!("SKIP temporary venv: Python venv module unavailable");
            return;
        }
        let python = interpreter_path(&environment::interpreter(&venv)).expect("path");
        let prefix = environment::run(
            Command::new(python).args(["-c", "import sys; print(sys.prefix)"]),
            Duration::from_secs(15),
        )
        .expect("prefix");
        assert_eq!(
            Path::new(prefix.trim())
                .canonicalize()
                .expect("prefix path"),
            venv.canonicalize().expect("venv path")
        );
    }
}
