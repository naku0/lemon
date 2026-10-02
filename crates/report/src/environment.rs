//! Explicit setup is the only operation allowed to install packages.
use super::ReportError;
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

pub fn directory() -> Result<PathBuf, ReportError> {
    let dirs =
        directories::BaseDirs::new().ok_or("report/environment: home directory unavailable")?;
    #[cfg(target_os = "macos")]
    let root = dirs.data_dir().join("LEMON");
    #[cfg(target_os = "windows")]
    let root = dirs.data_local_dir().join("LEMON");
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let root = dirs.data_local_dir().join("lemon");
    Ok(root.join("report-python"))
}
pub fn interpreter(dir: &Path) -> PathBuf {
    dir.join(if cfg!(windows) {
        "Scripts/python.exe"
    } else {
        "bin/python"
    })
}
pub fn managed_python() -> Result<PathBuf, ReportError> {
    Ok(interpreter(&directory()?))
}

// No shell; output is drained concurrently and capped, so a verbose subprocess cannot deadlock.
pub(crate) fn run(command: &mut Command, timeout: Duration) -> Result<String, ReportError> {
    use std::{io::Read, process::Stdio, thread, time::Instant};
    fn drain(mut stream: impl Read) -> Vec<u8> {
        let mut saved = Vec::new();
        let mut chunk = [0; 8192];
        while let Ok(n) = stream.read(&mut chunk) {
            if n == 0 {
                break;
            }
            let keep = n.min(65536usize.saturating_sub(saved.len()));
            saved.extend_from_slice(&chunk[..keep]);
        }
        saved
    }
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("report/environment: {e}"))?;
    let out = child
        .stdout
        .take()
        .ok_or("report/environment: missing stdout")?;
    let err = child
        .stderr
        .take()
        .ok_or("report/environment: missing stderr")?;
    let out = thread::spawn(move || drain(out));
    let err = thread::spawn(move || drain(err));
    let start = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().map_err(|e| e.to_string())? {
            break s;
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "report/environment: timeout after {} s",
                timeout.as_secs()
            ));
        }
        thread::sleep(Duration::from_millis(25));
    };
    let stdout =
        String::from_utf8_lossy(&out.join().map_err(|_| "stdout reader failed")?).into_owned();
    let stderr =
        String::from_utf8_lossy(&err.join().map_err(|_| "stderr reader failed")?).into_owned();
    if !status.success() {
        return Err(format!("report/environment: {status}\n{stdout}\n{stderr}"));
    }
    Ok(stdout)
}
fn version(
    python: &Path,
    runner: &impl Fn(&mut Command, Duration) -> Result<String, String>,
) -> Result<(), ReportError> {
    runner(Command::new(python).args(["-c", "import sys; assert sys.version_info >= (3,10), 'LEMON requires Python >= 3.10'; print(sys.version)"]), Duration::from_secs(15))?;
    Ok(())
}
pub fn doctor() -> Result<Value, ReportError> {
    let dir = directory()?;
    let python = interpreter(&dir);
    let candidate = if cfg!(windows) { "python" } else { "python3" };
    let system_python = run(
        Command::new(candidate).arg("--version"),
        Duration::from_secs(15),
    );
    let system_info = match system_python {
        Ok(version) => json!({"found":true,"command":candidate,"version":version.trim()}),
        Err(e) => json!({"found":false,"command":candidate,"problem":e}),
    };
    let result = probe(&python);
    Ok(match result {
        Ok(info) => {
            json!({"system_python":system_info,"ready":true,"venv":dir,"python":python,"details":info,"test_notebook":"passed"})
        }
        Err(e) => {
            json!({"system_python":system_info,"ready":false,"venv":dir,"python":python,"problem":e,"fix":"lemon report setup (or lemon report setup --recreate)"})
        }
    })
}
fn probe(python: &Path) -> Result<Value, ReportError> {
    probe_with(python, &run)
}
fn probe_with(
    python: &Path,
    runner: &impl Fn(&mut Command, Duration) -> Result<String, String>,
) -> Result<Value, ReportError> {
    version(python, runner)?;
    let info = runner(Command::new(python).args(["-c", "import nbformat,nbclient,numpy,pandas,matplotlib,ipykernel,sys,json,importlib.metadata as m; print(json.dumps({'python_version':sys.version.split()[0],'dependency_versions':{n:m.version(n) for n in ['numpy','pandas','matplotlib','nbformat','nbclient','ipykernel','jupyter']}}))"]), Duration::from_secs(30))?;
    let info: Value = serde_json::from_str(&info).map_err(|e| e.to_string())?;
    let code = r#"import sys,nbformat
from nbclient import NotebookClient
from jupyter_client import KernelManager
book=nbformat.v4.new_notebook(cells=[nbformat.v4.new_code_cell('assert 2+2 == 4')])
km=KernelManager(kernel_name='python3')
km.kernel_spec.argv=[sys.executable,'-m','ipykernel_launcher','-f','{connection_file}']
try:
    NotebookClient(book,km=km,timeout=20).execute()
finally:
    if km.has_kernel: km.shutdown_kernel(now=True)
print('ok')"#;
    runner(
        Command::new(python).args(["-c", code]),
        Duration::from_secs(60),
    )?;
    Ok(info)
}
fn checked_target(dir: &Path) -> Result<(), ReportError> {
    if dir.file_name().and_then(|s| s.to_str()) != Some("report-python") {
        return Err("report/setup: unexpected environment path".into());
    }
    for component in dir.ancestors() {
        if fs::symlink_metadata(component).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(format!(
                "report/setup: refusing symlink {}",
                component.display()
            ));
        }
    }
    Ok(())
}
pub fn setup(recreate: bool, selected: Option<&Path>) -> Result<Value, ReportError> {
    setup_at(&directory()?, recreate, selected, &run)
}
fn setup_at(
    dir: &Path,
    recreate: bool,
    selected: Option<&Path>,
    runner: &impl Fn(&mut Command, Duration) -> Result<String, String>,
) -> Result<Value, ReportError> {
    checked_target(dir)?;
    if dir.exists() && !recreate {
        return probe_with(&interpreter(dir), runner)
            .map_err(|e| format!("{e}\nRun lemon report setup --recreate"));
    }
    let python = if let Some(selected) = selected {
        version(selected, runner)?;
        selected.to_path_buf()
    } else {
        let mut found = None;
        for candidate in [
            "python3",
            "python",
            "python3.13",
            "python3.12",
            "python3.11",
            "python3.10",
        ] {
            if version(Path::new(candidate), runner).is_ok() {
                found = Some(PathBuf::from(candidate));
                break;
            }
        }
        found.ok_or("report/setup: Python >= 3.10 not found; use --python /path/to/python")?
    };
    if dir.exists() {
        if !dir.join("lemon-environment.json").is_file() || !dir.join("pyvenv.cfg").is_file() {
            return Err(
                "report/setup: refusing to delete an environment without LEMON ownership marker"
                    .into(),
            );
        }
        let marker: Value = serde_json::from_slice(
            &fs::read(dir.join("lemon-environment.json")).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        if marker["owner"] != "LEMON" {
            return Err("report/setup: invalid ownership marker".into());
        }
        fs::remove_dir_all(dir).map_err(|e| e.to_string())?;
    }
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    fs::write(
        dir.join("lemon-environment.json"),
        "{\"owner\":\"LEMON\",\"status\":\"setting_up\"}",
    )
    .map_err(|e| e.to_string())?;
    runner(
        Command::new(&python).arg("-m").arg("venv").arg(dir),
        Duration::from_secs(120),
    )?;
    let requirements = dir.join("report-requirements.txt");
    fs::write(
        &requirements,
        include_str!("../../../report-requirements.txt"),
    )
    .map_err(|e| e.to_string())?;
    let python = interpreter(dir);
    runner(
        Command::new(&python)
            .args([
                "-m",
                "pip",
                "--isolated",
                "--require-virtualenv",
                "install",
                "--disable-pip-version-check",
                "-r",
            ])
            .arg(&requirements),
        Duration::from_secs(900),
    )?;
    let info = probe_with(&python, runner)?;
    fs::write(
        dir.join("lemon-environment.json"),
        serde_json::to_vec_pretty(&json!({"owner":"LEMON","status":"ready","details":info}))
            .map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    fn temp() -> PathBuf {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        std::env::temp_dir()
            .canonicalize()
            .expect("temp")
            .join(format!(
                "lemon-env-test-{}-{}",
                std::process::id(),
                N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ))
    }
    #[test]
    fn setup_and_doctor_use_injected_runner_without_real_install() {
        let root = temp();
        let dir = root.join("report-python");
        let calls = RefCell::new(Vec::new());
        let fake = |command: &mut Command, _: Duration| {
            let args: Vec<_> = command
                .get_args()
                .map(|s| s.to_string_lossy().into_owned())
                .collect();
            calls
                .borrow_mut()
                .push((command.get_program().to_os_string(), args.clone()));
            if args.get(1).is_some_and(|s| s == "venv") {
                fs::write(dir.join("pyvenv.cfg"), "home = fake").expect("cfg");
            }
            if args.iter().any(|a| a.contains("dependency_versions")) {
                Ok("{\"python_version\":\"3.12\",\"dependency_versions\":{}}".into())
            } else {
                Ok("ok".into())
            }
        };
        setup_at(&dir, false, Some(Path::new("python ' Юникод")), &fake).expect("setup");
        assert_eq!(
            calls.borrow()[0].0,
            std::ffi::OsString::from("python ' Юникод")
        );
        assert!(calls
            .borrow()
            .iter()
            .any(|(_, a)| a.iter().any(|a| a == "pip")));
        calls.borrow_mut().clear();
        probe_with(&interpreter(&dir), &fake).expect("doctor");
        assert!(!calls
            .borrow()
            .iter()
            .any(|(_, a)| a.iter().any(|a| a == "pip")));
        setup_at(&dir, true, None, &fake).expect("recreate owned environment");
        fs::remove_dir_all(root).expect("cleanup");
    }
    #[test]
    fn rejected_python_and_unowned_recreate_preserve_files() {
        let root = temp();
        let dir = root.join("report-python");
        fs::create_dir_all(&dir).expect("mkdir");
        fs::write(dir.join("keep"), "safe").expect("file");
        assert!(setup_at(&dir, true, None, &|_, _| Ok("ok".into()))
            .expect_err("unowned")
            .contains("ownership"));
        assert!(dir.join("keep").exists());
        let absent = root.join("other/report-python");
        assert!(setup_at(&absent, false, None, &|_, _| Err(
            "unsupported Python".into()
        ))
        .is_err());
        assert!(!absent.exists());
        fs::remove_dir_all(root).expect("cleanup");
    }
    #[cfg(unix)]
    #[test]
    fn runner_timeout_and_literal_arguments() {
        let text = run(
            Command::new("/bin/echo").arg("literal ; $(nothing) ' Юникод"),
            Duration::from_secs(2),
        )
        .expect("echo");
        assert_eq!(text.trim(), "literal ; $(nothing) ' Юникод");
        assert!(run(
            Command::new("/bin/sleep").arg("1"),
            Duration::from_millis(10)
        )
        .expect_err("timeout")
        .contains("timeout"));
    }
    #[cfg(unix)]
    #[test]
    fn recreate_refuses_symlink() {
        let root = temp();
        fs::create_dir_all(&root).expect("mkdir");
        let dir = root.join("report-python");
        std::os::unix::fs::symlink("missing", &dir).expect("link");
        assert!(
            setup_at(&dir, true, None, &|_, _| panic!("must not run processes"))
                .expect_err("symlink")
                .contains("symlink")
        );
        fs::remove_dir_all(root).expect("cleanup");
    }
}
