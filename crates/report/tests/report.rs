#![allow(clippy::unwrap_used)]
use analysis::{AnalysisBundle, Summary};
use report::{render_notebook, ExecutionMode, ReportOptions};
use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

fn temp() -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let p = std::env::temp_dir().join(format!(
        "lemon-report-test-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&p).unwrap();
    p
}
fn bundle() -> (PathBuf, AnalysisBundle) {
    let p = temp();
    for name in [
        "manifest.json",
        "processed.csv",
        "spectrum.csv",
        "band-power.csv",
        "quality-events.csv",
    ] {
        fs::write(
            p.join(name),
            if name == "manifest.json" {
                "{\"analysis_format_version\":1}"
            } else {
                "header\n"
            },
        )
        .unwrap();
    }
    fs::write(p.join("summary.md"), "# summary\n").unwrap();
    fs::write(p.join("summary.json"), "{\"duration_seconds\":0,\"channel_count\":0,\"sample_count_by_channel\":{},\"valid_filtered_count_by_channel\":{},\"missing_count_by_channel\":{},\"lost_sample_count\":0,\"invalid_packet_count\":0,\"outlier_count\":0,\"saturation_count\":0,\"disconnection_count\":0,\"continuous_segment_count\":0,\"sample_rate_hz\":1,\"sample_rate_source\":\"test\",\"channels\":{},\"warnings\":[],\"dominant_band_by_channel\":{}}").unwrap();
    let s: Summary =
        serde_json::from_str(&fs::read_to_string(p.join("summary.json")).unwrap()).unwrap();
    (
        p.clone(),
        AnalysisBundle {
            directory: p,
            summary: s,
        },
    )
}
#[test]
fn generated_notebook_is_nbformat_four_and_uses_relative_paths() {
    let (source, bundle) = bundle();
    let out = temp().join("report");
    let result = render_notebook(
        &bundle,
        ReportOptions {
            output: Some(out.clone()),
            execution: ExecutionMode::NoExecute,
            python: None,
            overwrite: false,
            open: false,
            portable: false,
            colab: false,
            include_raw: false,
            maximum_embedded_report_mb: 25,
        },
    )
    .unwrap();
    let book: Value = serde_json::from_slice(&fs::read(&result.notebook).unwrap()).unwrap();
    assert_eq!(book["nbformat"], 4);
    let text = fs::read_to_string(&result.notebook).unwrap();
    assert!(text.contains("report-data.json"));
    assert!(!text.contains(source.to_string_lossy().as_ref()));
    assert_eq!(result.execution_status, "not_executed");
    assert!(out.join("report-requirements.txt").is_file());
    let _ = fs::remove_dir_all(source);
    let _ = fs::remove_dir_all(out);
}

#[test]
fn auto_without_python_and_safe_output() {
    let (source, bundle) = bundle();
    let out = temp().join("отчёт ' unicode");
    let options = ReportOptions {
        output: Some(out.clone()),
        execution: ExecutionMode::Auto,
        python: Some(source.join("missing-python")),
        overwrite: false,
        open: false,
        portable: false,
        colab: false,
        include_raw: false,
        maximum_embedded_report_mb: 25,
    };
    assert!(render_notebook(&bundle, options.clone())
        .expect_err("missing environment")
        .contains("setup"));
    assert!(out.join("analysis").exists());
    assert!(render_notebook(&bundle, options).is_err());
    assert!(render_notebook(
        &bundle,
        ReportOptions {
            output: Some(source.clone()),
            execution: ExecutionMode::NoExecute,
            python: None,
            overwrite: true,
            open: false,
            portable: false,
            colab: false,
            include_raw: false,
            maximum_embedded_report_mb: 25
        }
    )
    .is_err());
    assert!(source.join("summary.json").is_file());
    let _ = fs::remove_dir_all(source);
    let _ = fs::remove_dir_all(out);
}

#[test]
fn unsupported_version_and_missing_files_fail_before_output() {
    let (source, bundle) = bundle();
    let out = temp().join("report");
    fs::write(
        source.join("manifest.json"),
        "{\"analysis_format_version\":999}",
    )
    .expect("manifest");
    let options = ReportOptions {
        output: Some(out.clone()),
        execution: ExecutionMode::NoExecute,
        python: None,
        overwrite: false,
        open: false,
        portable: false,
        colab: false,
        include_raw: false,
        maximum_embedded_report_mb: 25,
    };
    assert!(render_notebook(&bundle, options.clone())
        .expect_err("version")
        .contains("version"));
    assert!(!out.exists());
    fs::remove_file(source.join("summary.json")).expect("remove");
    assert!(render_notebook(&bundle, options).is_err());
    let _ = fs::remove_dir_all(source);
}

#[test]
fn nested_output_does_not_modify_analysis() {
    let (source, bundle) = bundle();
    let output = source.join("new/reports");
    let error = render_notebook(
        &bundle,
        ReportOptions {
            output: Some(output),
            execution: ExecutionMode::NoExecute,
            python: None,
            overwrite: false,
            open: false,
            portable: false,
            colab: false,
            include_raw: false,
            maximum_embedded_report_mb: 25,
        },
    )
    .expect_err("nested output");
    assert!(error.contains("outside"));
    assert!(!source.join("new").exists());
    fs::remove_dir_all(source).expect("cleanup");
}

#[test]
fn failed_overwrite_retains_previous_report() {
    let (source, bundle) = bundle();
    let output = temp().join("report");
    let options = ReportOptions {
        output: Some(output.clone()),
        execution: ExecutionMode::NoExecute,
        python: None,
        overwrite: false,
        open: false,
        portable: false,
        colab: false,
        include_raw: false,
        maximum_embedded_report_mb: 25,
    };
    render_notebook(&bundle, options.clone()).expect("first report");
    let before = fs::read(output.join("report.ipynb")).expect("notebook");
    fs::remove_file(source.join("summary.md")).expect("remove fixture");
    let mut retry = options;
    retry.overwrite = true;
    assert!(render_notebook(&bundle, retry).is_err());
    assert_eq!(
        fs::read(output.join("report.ipynb")).expect("original survives"),
        before
    );
    fs::remove_dir_all(source).expect("cleanup");
    fs::remove_dir_all(output).expect("cleanup");
}

#[test]
fn portable_archive_requires_overwrite_and_is_replaced_safely() {
    let (source, bundle) = bundle();
    let output = temp().join("portable-report");
    let options = ReportOptions {
        output: Some(output.clone()),
        execution: ExecutionMode::NoExecute,
        python: None,
        overwrite: false,
        open: false,
        portable: true,
        colab: false,
        include_raw: false,
        maximum_embedded_report_mb: 25,
    };
    let first = render_notebook(&bundle, options.clone()).expect("first archive");
    let archive = first.archive.expect("archive path");
    assert_eq!(&fs::read(&archive).expect("archive")[..2], b"PK");
    assert!(render_notebook(&bundle, options.clone()).is_err());
    let mut replace = options;
    replace.overwrite = true;
    render_notebook(&bundle, replace).expect("replace archive");
    assert_eq!(&fs::read(&archive).expect("replacement")[..2], b"PK");
    fs::remove_dir_all(source).expect("cleanup source");
    fs::remove_dir_all(output).expect("cleanup report");
    fs::remove_file(archive).expect("cleanup archive");
}
