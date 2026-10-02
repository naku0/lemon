use analysis::{analyze, AnalysisInput, AnalysisOptions};
use core_types::{ChannelSamples, Config, RawSignalBlock, Sample};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
use storage::{Metadata, SessionWriter};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "lemon-analysis-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).expect("temp");
        Self(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn config() -> Config {
    let mut c = Config::default();
    c.source.channels = 1;
    c.source.sample_rate_hz = 256.;
    c.source.block_size = 16;
    c.processing.window_samples = 64;
    c.processing.hop_samples = 16;
    c.processing.low_pass_hz = 80.;
    c.processing.notch = false;
    c
}
fn session(dir: &Path, block_size: usize, hz: f64) -> PathBuf {
    let c = config();
    let mut m = Metadata::new(c.clone());
    m.config.session.name = "known".into();
    let p = dir.join(format!("session-{block_size}"));
    let mut w = SessionWriter::create_at(&p, &m).expect("writer");
    for (sequence, start) in (0..1024).step_by(block_size).enumerate() {
        let samples = (start..(start + block_size).min(1024))
            .map(|i| Sample {
                sequence: i as u64,
                timestamp_ns: (i as f64 / c.source.sample_rate_hz * 1e9) as u64,
                value: (std::f64::consts::TAU * hz * i as f64 / c.source.sample_rate_hz).sin(),
                flags: vec![],
            })
            .collect();
        w.write_block(&RawSignalBlock {
            sequence: sequence as u64,
            started_at: start as u64 * 3_906_250,
            sample_rate_hz: c.source.sample_rate_hz,
            source: "synthetic".into(),
            synchronized_clock: true,
            channels: vec![ChannelSamples {
                id: "ch1".into(),
                device: "test".into(),
                samples,
            }],
            flags: vec![],
        })
        .expect("block");
    }
    w.flush().expect("flush");
    p
}
fn rows(p: &Path, name: &str) -> usize {
    fs::read_to_string(p.join(name))
        .expect("csv")
        .lines()
        .count()
        - 1
}
#[test]
fn known_tones_have_complete_frames_and_streaming_block_invariance() {
    let t = Temp::new();
    let a = session(&t.0, 16, 10.);
    let b = session(&t.0, 37, 10.);
    let r1 = analyze(
        AnalysisInput { path: a },
        AnalysisOptions {
            output: Some(t.0.join("r1")),
            ..Default::default()
        },
    )
    .expect("analysis");
    let r2 = analyze(
        AnalysisInput { path: b },
        AnalysisOptions {
            output: Some(t.0.join("r2")),
            ..Default::default()
        },
    )
    .expect("analysis");
    assert!(rows(&r1.directory, "spectrum.csv") > 500);
    assert_eq!(
        rows(&r1.directory, "spectrum.csv"),
        rows(&r2.directory, "spectrum.csv")
    );
    assert_eq!(
        rows(&r1.directory, "band-power.csv"),
        rows(&r2.directory, "band-power.csv")
    );
    for file in [
        "spectrum.csv",
        "band-power.csv",
        "processed.csv",
        "summary.json",
    ] {
        assert_eq!(
            fs::read(r1.directory.join(file)).expect("first"),
            fs::read(r2.directory.join(file)).expect("second"),
            "block invariance: {file}"
        );
    }
    assert_eq!(r1.summary.lost_sample_count, 0);
    assert_eq!(
        rows(&r1.directory, "spectrum.csv"),
        (1 + (1024 - 64) / 16) * 33
    );
    let s = fs::read_to_string(r1.directory.join("summary.json")).expect("summary");
    assert!(s.contains("dominant_peak_frequency_hz"));
    let summary: serde_json::Value = serde_json::from_str(&s).expect("json");
    let peak = summary["channels"]["ch1"]["dominant_peak_frequency_hz"]
        .as_f64()
        .expect("peak");
    assert!((peak - 10.0).abs() <= 4.0, "peak={peak}");
}
#[test]
fn bundle_has_all_files_and_neutral_summary() {
    let t = Temp::new();
    let p = session(&t.0, 25, 6.);
    let b = analyze(
        AnalysisInput { path: p },
        AnalysisOptions {
            output: Some(t.0.join("report")),
            ..Default::default()
        },
    )
    .expect("analysis");
    for name in [
        "manifest.json",
        "summary.json",
        "summary.md",
        "processed.csv",
        "spectrum.csv",
        "band-power.csv",
        "quality-events.csv",
    ] {
        assert!(b.directory.join(name).is_file(), "{name}");
    }
    let md = fs::read_to_string(b.directory.join("summary.md"))
        .expect("md")
        .to_lowercase();
    for forbidden in [
        "relaxed",
        "focused",
        "attention increased",
        "cognitive state",
        "diagnosis",
        "пользователь был расслаблен",
        "пользователь концентрировался",
        "обнаружено заболевание",
        "активность мозга повысилась",
    ] {
        assert!(!md.contains(forbidden), "forbidden {forbidden}");
    }
}
#[test]
fn overwrite_is_scoped_and_missing_window_is_explicit() {
    let t = Temp::new();
    let p = session(&t.0, 20, 20.);
    let out = t.0.join("report");
    analyze(
        AnalysisInput { path: p.clone() },
        AnalysisOptions {
            output: Some(out.clone()),
            ..Default::default()
        },
    )
    .expect("first");
    assert!(analyze(
        AnalysisInput { path: p.clone() },
        AnalysisOptions {
            output: Some(out.clone()),
            ..Default::default()
        }
    )
    .is_err());
    analyze(
        AnalysisInput { path: p },
        AnalysisOptions {
            output: Some(out.clone()),
            overwrite: true,
            ..Default::default()
        },
    )
    .expect("overwrite");
    let s = fs::read_to_string(out.join("summary.json")).expect("summary");
    assert!(s.contains("sample_rate_hz"));
}

#[test]
fn unsafe_overwrite_preserves_source_and_unrelated_files() {
    let t = Temp::new();
    let source = session(&t.0, 37, 10.);
    let before = fs::read(source.join("raw.csv")).expect("raw");
    for target in [source.clone(), t.0.clone()] {
        assert!(analyze(
            AnalysisInput {
                path: source.clone()
            },
            AnalysisOptions {
                output: Some(target),
                overwrite: true,
                ..Default::default()
            }
        )
        .is_err());
    }
    assert_eq!(before, fs::read(source.join("raw.csv")).expect("unchanged"));
}

#[test]
fn known_peaks_and_real_channel_names() {
    for hz in [6., 10., 20.] {
        let t = Temp::new();
        let p = session(&t.0, 37, hz);
        let mut c = config();
        c.processing.window_samples = 256;
        c.processing.hop_samples = 64;
        let b = analyze(
            AnalysisInput { path: p },
            AnalysisOptions {
                output: Some(t.0.join("analysis")),
                config: Some(c),
                ..Default::default()
            },
        )
        .expect("analysis");
        assert!(
            (b.summary.channels["ch1"]
                .dominant_peak_frequency_hz
                .expect("peak")
                - hz)
                .abs()
                <= 1.
        );
        assert_eq!(b.summary.lost_sample_count, 0);
        assert_eq!(b.summary.continuous_segment_count, 1);
    }
}

#[test]
fn asynchronous_channels_keep_all_windows_and_gaps() {
    let t = Temp::new();
    let mut c = config();
    c.source.channels = 2;
    let p = t.0.join("input");
    let mut w = SessionWriter::create_at(&p, &Metadata::new(c.clone())).expect("writer");
    let samples = |count: usize, skip: bool| {
        (0..count)
            .filter(|i| !skip || *i != 128)
            .map(|i| Sample {
                sequence: i as u64,
                timestamp_ns: i as u64 * 3_906_250,
                value: (std::f64::consts::TAU * 10. * i as f64 / 256.).sin(),
                flags: vec![],
            })
            .collect()
    };
    w.write_block(&RawSignalBlock {
        sequence: 0,
        started_at: 0,
        sample_rate_hz: 256.,
        source: "test".into(),
        synchronized_clock: false,
        channels: vec![
            ChannelSamples {
                id: "C3".into(),
                device: "a".into(),
                samples: samples(256, true),
            },
            ChannelSamples {
                id: "C4".into(),
                device: "b".into(),
                samples: samples(80, false),
            },
        ],
        flags: vec![],
    })
    .expect("write");
    w.flush().expect("flush");
    let b = analyze(
        AnalysisInput { path: p },
        AnalysisOptions {
            output: Some(t.0.join("result")),
            ..Default::default()
        },
    )
    .expect("analyze");
    assert_eq!(b.summary.sample_count_by_channel["C3"], 255);
    assert_eq!(b.summary.missing_count_by_channel["C3"], 1);
    assert_eq!(b.summary.lost_sample_count, 1);
    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    for row in csv::Reader::from_path(b.directory.join("spectrum.csv"))
        .expect("csv")
        .records()
    {
        let row = row.expect("row");
        *counts.entry(row[3].into()).or_default() += 1;
        if row[3] == *"C3" {
            let start: u64 = row[1].parse().expect("start");
            let end: u64 = row[2].parse().expect("end");
            assert!(!(start < 128 * 3_906_250 && end > 128 * 3_906_250));
        }
    }
    assert_eq!(counts["C3"], 9 * 33);
    assert_eq!(counts["C4"], 2 * 33);
}

#[test]
fn notch_and_band_ranking_use_existing_dsp_outputs() {
    let t = Temp::new();
    let input = t.0.join("tones.csv");
    let mut csv = String::from("time,ch1\n");
    for i in 0..4096 {
        let time = i as f64 / 256.;
        let signal = [(6., 1.), (10., 4.), (20., 2.), (50., 3.)]
            .iter()
            .map(|(f, a)| a * (std::f64::consts::TAU * f * time).sin())
            .sum::<f64>();
        csv.push_str(&format!("{time},{signal}\n"));
    }
    fs::write(&input, csv).expect("input");
    let mut c = config();
    c.processing.window_samples = 256;
    c.processing.hop_samples = 64;
    let off = analyze(
        AnalysisInput {
            path: input.clone(),
        },
        AnalysisOptions {
            config: Some(c.clone()),
            output: Some(t.0.join("off")),
            ..Default::default()
        },
    )
    .expect("off");
    c.processing.notch = true;
    let on = analyze(
        AnalysisInput { path: input },
        AnalysisOptions {
            config: Some(c),
            output: Some(t.0.join("on")),
            ..Default::default()
        },
    )
    .expect("on");
    let a = &off.summary.channels["ch1"];
    let b = &on.summary.channels["ch1"];
    assert!(b.mains_power_after < a.mains_power_after * 0.15);
    let alpha = |c: &analysis::ChannelSummary| {
        c.band_statistics
            .values()
            .find(|v| v.low_hz == 8. && v.high_hz == 13.)
            .expect("alpha")
            .mean_absolute_power
    };
    assert!(alpha(b) > alpha(a) * 0.9);
    assert!(b
        .band_statistics
        .values()
        .filter(|v| v.low_hz != 8.)
        .all(|v| v.mean_absolute_power < alpha(b)));
    assert!(b.mains_reduction_percent.expect("reduction") > 80.);
    assert!(a.mains_reduction_percent.is_none());
}

#[test]
fn spectral_window_bounds_follow_actual_sample_timestamps() {
    let t = Temp::new();
    let c = config();
    let input = t.0.join("jitter");
    let mut writer = SessionWriter::create_at(&input, &Metadata::new(c.clone())).expect("writer");
    let timestamps: Vec<u64> = (0..128)
        .map(|i| 1_000_000_000 + i * 3_906_250 + (i % 3) * 50_000)
        .collect();
    writer
        .write_block(&RawSignalBlock {
            sequence: 0,
            started_at: timestamps[0],
            sample_rate_hz: 256.,
            source: "synthetic".into(),
            synchronized_clock: true,
            channels: vec![ChannelSamples {
                id: "ch1".into(),
                device: "test".into(),
                samples: timestamps
                    .iter()
                    .enumerate()
                    .map(|(i, &timestamp_ns)| Sample {
                        sequence: i as u64,
                        timestamp_ns,
                        value: (std::f64::consts::TAU * 10. * i as f64 / 256.).sin(),
                        flags: vec![],
                    })
                    .collect(),
            }],
            flags: vec![],
        })
        .expect("block");
    writer.flush().expect("flush");
    let output = t.0.join("analysis");
    analyze(
        AnalysisInput { path: input },
        AnalysisOptions {
            output: Some(output.clone()),
            ..Default::default()
        },
    )
    .expect("analyze");
    let mut reader = csv::Reader::from_path(output.join("spectrum.csv")).expect("spectrum");
    for row in reader.records() {
        let row = row.expect("row");
        let index: usize = row[0].parse().expect("index");
        assert_eq!(
            row[1].parse::<u64>().expect("start"),
            timestamps[index * 16]
        );
        assert_eq!(
            row[2].parse::<u64>().expect("end"),
            timestamps[index * 16 + 63]
        );
    }
    let summary: serde_json::Value =
        serde_json::from_slice(&fs::read(output.join("summary.json")).expect("summary"))
            .expect("json");
    assert_eq!(
        summary["channels"]["ch1"]["quality_distribution"]
            .as_object()
            .expect("distribution")
            .len(),
        5
    );
}
