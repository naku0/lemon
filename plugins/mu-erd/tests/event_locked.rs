use mu_erd::{run, Arguments};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

struct Fixture {
    root: PathBuf,
    bundle: PathBuf,
    config: PathBuf,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::env::var_os("LEMON_KEEP_TEST_OUTPUT").is_none() {
            let _ = fs::remove_dir_all(&self.root);
        } else {
            eprintln!("fixture: {}", self.root.display());
        }
    }
}
fn write_json(p: &Path, j: &Value) {
    fs::write(p, serde_json::to_vec_pretty(j).expect("json")).expect("write");
}
fn read_json(p: &Path) -> Value {
    serde_json::from_slice(&fs::read(p).expect("read")).expect("json")
}
impl Fixture {
    fn new() -> Self {
        static ID: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "lemon-mu-erd-{}-{}",
            std::process::id(),
            ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).expect("temp");
        let bundle = root.join("bundle");
        fs::create_dir(&bundle).expect("bundle");
        write_json(
            &bundle.join("manifest.json"),
            &json!({"analysis_format_version":2,"dsp_algorithm_version":3,"windowing_algorithm_version":1,"psd_algorithm_version":2,"processing_mode":"causal_live_compatible","window_function":"hann_symmetric","filter_warmup_seconds":2.0,"reject_low_quality_windows":true,"lemon_version":"0.2.0","created_at":"0","source":"fixture","source_format":"synthetic","source_path":"fixture","channels":["A0","A2"],"units":"adc_counts_u8","sample_rate_hz":100.,"sample_rate_source":"confirmed","duration_seconds":24.,"processing_config":{"high_pass_hz":1.,"low_pass_hz":40.,"notch":false},"spectral_config":{"window_samples":200,"hop_samples":50,"window":"hann_symmetric"},"configured_bands":[]}),
        );
        write_json(
            &bundle.join("summary.json"),
            &json!({"duration_seconds":24.,"channel_count":2,"sample_count_by_channel":{"A0":2400,"A2":2400},"valid_filtered_count_by_channel":{"A0":2400,"A2":2400},"missing_count_by_channel":{"A0":0,"A2":0},"lost_sample_count":0,"invalid_packet_count":0,"outlier_count":0,"saturation_count":0,"disconnection_count":0,"continuous_segment_count":1,"sample_rate_hz":100.,"sample_rate_source":"confirmed","channels":{},"warnings":[],"dominant_band_by_channel":{}}),
        );
        fs::write(bundle.join("summary.md"), "synthetic").expect("summary");
        for (file,header) in [("spectrum.csv","window_index,window_start_ns,window_end_ns,channel,segment_id,frequency_hz,raw_psd,filtered_psd,window_function,sample_count,frequency_resolution_hz\n"),("band-power.csv","window_index,window_start_ns,window_end_ns,channel,segment_id,band,low_hz,high_hz,absolute,relative,smoothed,baseline_change_pct,quality_status\n"),("quality-events.csv","timestamp_ns,channel,kind,severity,details\n"),("excluded-windows.csv","channel,segment_id,window_index,window_start_ns,window_end_ns,reason_codes,details\n")]{fs::write(bundle.join(file),header).expect("csv");}
        let mut w = csv::Writer::from_path(bundle.join("processed.csv")).expect("csv");
        w.write_record("timestamp_ns,sequence,channel,raw,filtered,segment_id,sample_valid,filter_warmup,artifact_rejected,quality,flags".split(',')).expect("header");
        for n in 0..2400 {
            let t = n as f64 / 100.;
            let amplitude = if (6. ..10.).contains(&t) || (18. ..22.).contains(&t) {
                2.
            } else {
                4.
            };
            let value = amplitude * (std::f64::consts::TAU * 10. * t).sin()
                + 2. * (std::f64::consts::TAU * 20. * t).sin();
            for ch in ["A0", "A2"] {
                w.write_record([
                    (n * 10_000_000u64).to_string(),
                    n.to_string(),
                    ch.into(),
                    value.to_string(),
                    value.to_string(),
                    "0".into(),
                    "true".into(),
                    "false".into(),
                    "false".into(),
                    "Good".into(),
                    "".into(),
                ])
                .expect("row");
            }
        }
        w.flush().expect("flush");
        let mut events = Vec::new();
        let mut plan = Vec::new();
        for n in 1..=2 {
            plan.push(json!({"trial_number":n,"condition":"squeeze"}));
            let offset = (n - 1) * 12;
            for (kind, seconds) in [
                ("TrialStart", 1),
                ("BaselineStart", 1),
                ("BaselineEnd", 5),
                ("Cue", 5),
                ("TaskStart", 6),
                ("TaskEnd", 10),
                ("RestStart", 10),
                ("RestEnd", 12),
                ("TrialEnd", 12),
            ] {
                events.push(json!({"timestamp_ns":(offset+seconds)*1_000_000_000u64,"kind":kind,"text":json!({"protocol_id":"test","trial_number":n,"condition":"squeeze"}).to_string()}));
            }
        }
        write_json(
            &bundle.join("experiment.json"),
            &json!({"schema_version":1,"protocol":{"id":"test","source_file_hash":"sha256:synthetic"},"status":"completed","marker_timestamp_basis":"last_observed_sample","resolved_plan":plan,"events":events}),
        );
        let config = root.join("config.json");
        let mut c: Value =
            serde_json::from_str(include_str!("../../../examples/mu-erd.json")).expect("config");
        c["conditions"] = json!(["squeeze"]);
        write_json(&config, &c);
        Self {
            root,
            bundle,
            config,
        }
    }
    fn args(&self, name: &str) -> Arguments {
        Arguments {
            input: self.bundle.clone(),
            config: self.config.clone(),
            output: self.root.join(name),
        }
    }
    fn run(&self) -> PathBuf {
        let a = self.args("out");
        let p = a.output.clone();
        run(a).expect("analysis");
        p
    }
    fn edit_config(&self, key: &str, value: Value) {
        let mut c = read_json(&self.config);
        c[key] = value;
        write_json(&self.config, &c);
    }
    fn change_samples(&self, mut change: impl FnMut(&mut Vec<String>)) {
        let path = self.bundle.join("processed.csv");
        let mut r = csv::Reader::from_path(&path).expect("reader");
        let headers = r.headers().expect("header").clone();
        let rows = r
            .records()
            .map(|r| {
                r.expect("row")
                    .iter()
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        let mut w = csv::Writer::from_path(path).expect("writer");
        w.write_record(&headers).expect("header");
        for mut row in rows {
            change(&mut row);
            if !row.is_empty() {
                w.write_record(row).expect("row");
            }
        }
        w.flush().expect("flush");
    }
}
fn rows(p: &Path) -> Vec<BTreeMap<String, String>> {
    csv::Reader::from_path(p)
        .expect("csv")
        .deserialize()
        .map(|r| r.expect("row"))
        .collect()
}
fn snapshot(path: &Path) -> BTreeMap<String, Vec<u8>> {
    fs::read_dir(path)
        .expect("dir")
        .map(|e| {
            let p = e.expect("entry").path();
            (
                p.file_name().expect("name").to_string_lossy().into(),
                fs::read(p).expect("bytes"),
            )
        })
        .collect()
}

#[test]
fn mu_reduces_beta_stable_epoch_and_real_figures() {
    let f = Fixture::new();
    let before = snapshot(&f.bundle);
    let out = f.run();
    assert_eq!(before, snapshot(&f.bundle));
    let tr = rows(&out.join("trial-results.csv"));
    assert_eq!(tr.len(), 8);
    for r in &tr {
        assert_eq!(r["included"], "true");
        assert_eq!(r["baseline_window_count"], "3");
        assert_eq!(r["analysis_window_count"], "5");
        let pct = r["relative_change_percent"].parse::<f64>().expect("pct");
        if r["band"] == "Mu" {
            assert!((pct + 75.).abs() < 0.1);
            assert!(r["change_db"].parse::<f64>().expect("db") < -5.9);
        } else {
            assert!(pct.abs() < 0.1);
        }
    }
    let win = rows(&out.join("windowed-power.csv"));
    for r in &win {
        let t = r["relative_time_seconds"].parse::<f64>().expect("t");
        if r["interval"] == "baseline" {
            assert!((-4. ..=-3.).contains(&t));
        } else {
            assert!((0. ..=2.).contains(&t));
        }
    }
    let summary = read_json(&out.join("summary.json"));
    assert_eq!(summary["total_trials"], 2);
    assert_eq!(summary["included_trials"], 2);
    assert!(summary["warnings"]
        .to_string()
        .contains("hardware triggers"));
    let agg = rows(&out.join("timecourse.csv"));
    assert!(agg.iter().all(|r| r["trial_count"] == "2"));
    let result: lemon_plugin_sdk::PluginResult =
        serde_json::from_value(read_json(&out.join("plugin-result.json"))).expect("result");
    let mut figures = 0;
    for a in result.artifacts {
        assert!(out.join(&a.path).is_file());
        if a.path.ends_with(".png") {
            figures += 1;
            let bytes = fs::read(out.join(a.path)).expect("png");
            assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
            assert!(bytes.len() > 10000);
            assert_eq!(
                u32::from_be_bytes(bytes[16..20].try_into().expect("width")),
                1200
            );
        }
    }
    assert_eq!(figures, 6);
}
#[test]
fn artifact_gap_warmup_and_low_quality_are_explained() {
    for flag in ["Outlier", "LostSample", "FilterWarmup", "LowQuality"] {
        let f = Fixture::new();
        f.change_samples(|r| {
            if r[1] == "650" && r[2] == "A0" {
                r[10] = flag.into();
                if flag == "FilterWarmup" {
                    r[7] = "true".into();
                }
            }
        });
        let out = f.run();
        let ex = rows(&out.join("excluded-trials.csv"));
        assert_eq!(ex.len(), 1);
        assert!(ex[0]["reason_codes"].contains(flag));
        let tr = rows(&out.join("trial-results.csv"));
        assert!(tr
            .iter()
            .filter(|r| r["trial_number"] == "1" && r["channel"] == "A0")
            .all(|r| r["relative_change_percent"].is_empty()));
    }
}
#[test]
fn absent_samples_and_segment_changes_exclude_trials() {
    for segment in [false, true] {
        let f = Fixture::new();
        f.change_samples(|r| {
            if r[2] == "A0" {
                if segment && r[1].parse::<u64>().expect("seq") >= 650 {
                    r[5] = "1".into();
                } else if !segment && r[1] == "650" {
                    r.clear();
                }
            }
        });
        let out = f.run();
        let ex = rows(&out.join("excluded-trials.csv"));
        assert!(ex[0]["reason_codes"].contains(if segment {
            "MixedSegment"
        } else {
            "SequenceDiscontinuity"
        }));
    }
}
#[test]
fn not_enough_baseline_or_task_windows_are_not_fabricated() {
    for baseline in [true, false] {
        let f = Fixture::new();
        f.edit_config(
            if baseline {
                "minimum_baseline_windows"
            } else {
                "minimum_analysis_windows"
            },
            json!(20),
        );
        let out = f.run();
        let ex = rows(&out.join("excluded-trials.csv"));
        assert_eq!(ex.len(), 4);
        assert!(ex.iter().all(|r| r["reason_codes"].contains(if baseline {
            "InsufficientBaselineWindows"
        } else {
            "InsufficientAnalysisWindows"
        })));
        assert!(!out.join("figures").exists());
    }
    // The original two-second baseline can contain one two-second window only;
    // it also crosses the cue phase, and must never borrow task samples.
    let f = Fixture::new();
    f.edit_config("baseline_seconds", json!([-2., 0.]));
    let out = f.run();
    assert!(
        rows(&out.join("excluded-trials.csv"))[0]["reason_codes"].contains("BaselineOutsidePhase")
    );
}
#[test]
fn missing_sidecar_unknown_names_and_invalid_config_fail() {
    let f = Fixture::new();
    fs::remove_file(f.bundle.join("experiment.json")).expect("remove");
    assert!(run(f.args("out"))
        .expect_err("missing")
        .contains("experiment.json"));
    for (key, value) in [
        ("conditions", json!(["missing"])),
        ("channels", json!(["C3"])),
        ("anchor_event", json!("Missing")),
        ("mode", json!("other")),
        ("epoch_seconds", json!([-1, 1])),
        ("window_seconds", json!(0.07)),
        ("hop_seconds", json!(0)),
        ("minimum_baseline_windows", json!(0)),
        ("channels", json!(["A0", "A0"])),
    ] {
        let f = Fixture::new();
        f.edit_config(key, value);
        assert!(run(f.args("out")).is_err(), "{key}");
        assert!(!f.root.join("out").exists());
    }
}
#[test]
fn cancelled_requires_opt_in_and_complete_trials() {
    let f = Fixture::new();
    let p = f.bundle.join("experiment.json");
    let mut s = read_json(&p);
    s["status"] = json!("cancelled");
    s["events"]
        .as_array_mut()
        .expect("events")
        .retain(|e| e["kind"] != "TrialEnd" || e["timestamp_ns"] != 24_000_000_000u64);
    write_json(&p, &s);
    assert!(run(f.args("out")).is_err());
    f.edit_config("allow_cancelled", json!(true));
    let out = f.run();
    let s = read_json(&out.join("summary.json"));
    assert_eq!(s["included_trials"], 1);
    assert!(rows(&out.join("excluded-trials.csv"))
        .iter()
        .all(|r| r["reason_codes"].contains("IncompleteTrial")));
}
#[test]
fn pause_duplicate_and_missing_anchor_are_not_silent() {
    for mode in ["pause", "duplicate", "missing"] {
        let f = Fixture::new();
        let p = f.bundle.join("experiment.json");
        let mut s = read_json(&p);
        let events = s["events"].as_array_mut().expect("events");
        let mut e = events[4].clone();
        if mode == "pause" {
            e["kind"] = json!("ExperimentPaused");
            events.insert(5, e);
        } else if mode == "duplicate" {
            events.insert(5, e);
        } else {
            events.remove(4);
        }
        write_json(&p, &s);
        let out = f.run();
        let codes = &rows(&out.join("excluded-trials.csv"))[0]["reason_codes"];
        assert!(codes.contains(match mode {
            "pause" => "PausedTrial",
            "duplicate" => "DuplicateMarker",
            _ => "MissingAnchor",
        }));
    }
}
#[test]
fn invalid_provenance_and_malformed_exclusions_fail() {
    for field in [
        "dsp_algorithm_version",
        "psd_algorithm_version",
        "windowing_algorithm_version",
    ] {
        let f = Fixture::new();
        let p = f.bundle.join("manifest.json");
        let mut m = read_json(&p);
        m[field] = json!(99);
        write_json(&p, &m);
        assert!(run(f.args("out")).is_err());
    }
    let f = Fixture::new();
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(f.bundle.join("excluded-windows.csv"))
        .expect("open");
    writeln!(file, "A0,wrong,0,0,1,Outlier,bad").expect("write");
    assert!(run(f.args("out")).is_err());
}
#[test]
fn output_overlap_and_nonempty_are_rejected() {
    let f = Fixture::new();
    let mut a = f.args("out");
    a.output = f.bundle.join("nested");
    assert!(run(a).is_err());
    assert!(!f.bundle.join("nested").exists());
    let out = f.run();
    assert!(run(f.args("out")).is_err());
    assert!(out.join("summary.json").exists());
}
#[test]
fn external_process_passes_platform_validation() {
    let f = Fixture::new();
    let plugin = f.root.join("plugin");
    fs::create_dir(&plugin).expect("dir");
    let exe = if cfg!(windows) {
        "mu-erd.exe"
    } else {
        "mu-erd"
    };
    fs::copy(env!("CARGO_BIN_EXE_mu-erd"), plugin.join(exe)).expect("binary");
    write_json(
        &plugin.join("plugin.json"),
        &json!({"schema_version":1,"id":"mu-erd","display_name":"Event-locked Mu-band power analysis","version":env!("CARGO_PKG_VERSION"),"api_version":1,"description":"test","entrypoint":{"program":exe,"args":[]}}),
    );
    let mut platform = plugins::Platform::open(f.root.join("registry.json")).expect("platform");
    platform.add(&plugin).expect("register");
    let before = snapshot(&f.bundle);
    let report = platform
        .run(&plugins::RunOptions {
            plugin: "mu-erd".into(),
            input: f.bundle.clone(),
            config: f.config.clone(),
            output: f.root.join("external"),
            overwrite: false,
            timeout: Duration::from_secs(60),
        })
        .expect("run");
    assert_eq!(report.result.plugin.id, "mu-erd");
    assert!(report.result.artifacts.len() >= 10);
    assert_eq!(before, snapshot(&f.bundle));
}

#[test]
fn zero_baseline_and_short_epochs_are_excluded() {
    let f = Fixture::new();
    f.change_samples(|r| {
        if r[2] == "A0" && r[1].parse::<u64>().expect("seq") < 600 {
            r[4] = "0".into();
        }
    });
    let out = f.run();
    assert!(
        rows(&out.join("excluded-trials.csv"))[0]["reason_codes"].contains("NonPositiveBaseline")
    );
    let f = Fixture::new();
    f.change_samples(|r| {
        let n = r[1].parse::<u64>().expect("seq");
        if (200..300).contains(&n) {
            r.clear();
        }
    });
    let out = f.run();
    assert!(rows(&out.join("excluded-trials.csv"))[0]["reason_codes"].contains("IncompleteEpoch"));
}
#[test]
fn timestamped_quality_events_exclude_only_the_affected_trial_channel() {
    let f = Fixture::new();
    fs::write(
        f.bundle.join("quality-events.csv"),
        "timestamp_ns,channel,kind,severity,details\n6500000000,A0,LowQuality,warning,synthetic\n",
    )
    .expect("event");
    let out = f.run();
    let r = rows(&out.join("excluded-trials.csv"));
    assert_eq!(r.len(), 1);
    assert_eq!(r[0]["channel"], "A0");
    assert!(r[0]["reason_codes"].contains("LowQuality"));
}

#[test]
fn conditions_are_separate_and_missing_all_anchors_is_an_error() {
    let f = Fixture::new();
    let path = f.bundle.join("experiment.json");
    let mut s = read_json(&path);
    s["resolved_plan"][1]["condition"] = json!("other");
    for e in s["events"].as_array_mut().expect("events") {
        let mut text: Value =
            serde_json::from_str(e["text"].as_str().expect("text")).expect("json");
        if text["trial_number"] == 2 {
            text["condition"] = json!("other");
            e["text"] = json!(text.to_string());
        }
    }
    write_json(&path, &s);
    f.edit_config("conditions", json!(["squeeze", "other"]));
    let out = f.run();
    let curves = rows(&out.join("timecourse.csv"));
    assert!(curves.iter().all(|r| r["trial_count"] == "1"));
    assert!(curves.iter().any(|r| r["condition"] == "other"));
    let f = Fixture::new();
    let p = f.bundle.join("experiment.json");
    let mut s = read_json(&p);
    s["events"]
        .as_array_mut()
        .expect("events")
        .retain(|e| e["kind"] != "TaskStart");
    write_json(&p, &s);
    assert!(run(f.args("out"))
        .expect_err("anchor")
        .contains("anchor event"));
}
