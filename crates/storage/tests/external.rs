use core_types::{Config, RawSignalBlock};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
use storage::{
    external::{self, Options},
    read_metadata, SessionReader,
};
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "lemon-external-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).expect("temp");
        Self(p)
    }
    fn file(&self, name: &str, text: &str) -> PathBuf {
        let p = self.0.join(name);
        fs::write(&p, text).expect("fixture");
        p
    }
    fn import(&self, p: &Path, o: &Options) -> PathBuf {
        external::import(p, None, &self.0, o, &Config::default()).expect("import")
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn fixture(s: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(s)
}
fn explicit() -> Options {
    Options {
        sample_rate: Some(250.),
        ..Options::default()
    }
}
fn blocks(p: &Path) -> Vec<RawSignalBlock> {
    let mut r = SessionReader::open(p).expect("reader");
    let mut v = Vec::new();
    while let Some(b) = r.next_block().expect("block") {
        v.push(b);
    }
    v
}

#[test]
fn dat_whitespace_repeated_times_preserves_values_on_strict_grid() {
    let t = Temp::new();
    let p = t.import(&fixture("bitronics/demo_A0.dat"), &explicit());
    let b = blocks(&p);
    let s: Vec<_> = b.iter().flat_map(|b| &b.channels[0].samples).collect();
    assert_eq!(
        s.iter().map(|s| s.value).collect::<Vec<_>>(),
        [1., 2., 99., -3.]
    );
    assert_eq!(
        s.iter().map(|s| s.timestamp_ns).collect::<Vec<_>>(),
        [0, 4_000_000, 8_000_000, 12_000_000]
    );
    let m = read_metadata(&p).expect("metadata");
    assert_eq!(m.format_version, 2);
    assert!(m
        .provenance
        .expect("provenance")
        .import_warnings
        .iter()
        .any(|w| w.contains("repeated")));
}
#[test]
fn directory_and_dmov_keep_independent_channels_and_ignore_only_exact_copy() {
    for input in [fixture("bitronics"), fixture("bitronics/demo.dmov")] {
        let t = Temp::new();
        let p = t.import(&input, &explicit());
        let b = blocks(&p);
        let a: Vec<_> = b.iter().flat_map(|b| &b.channels[0].samples).collect();
        let c: Vec<_> = b.iter().flat_map(|b| &b.channels[1].samples).collect();
        assert_eq!((a.len(), c.len()), (4, 3));
        assert_eq!(c[0].timestamp_ns, 8_000_000);
        assert_eq!(a[0].timestamp_ns, 0);
        assert!(b.iter().all(|b| !b.synchronized_clock));
        let pr = read_metadata(&p)
            .expect("metadata")
            .provenance
            .expect("provenance");
        assert_eq!(pr.duplicate_files_ignored.len(), 1);
        assert_eq!(pr.detected_channels, ["A0", "A1"]);
    }
}
#[test]
fn differing_duplicate_and_missing_dmov_dat_require_explicit_action() {
    let t = Temp::new();
    t.file("demo_A0.dat", "0 1\n.004 2\n");
    t.file("demo_A0_1.dat", include_str!("fixtures/different_A0_1.dat"));
    let error =
        external::import(&t.0, None, &t.0, &explicit(), &Config::default()).expect_err("different");
    assert!(error.contains("differs"));
    let d = Temp::new();
    let p = d.file("empty.dmov", "dummy");
    let e = external::prepare(&p, &explicit(), &Config::default())
        .err()
        .expect("no DAT");
    assert!(e.contains("DAT-файлы не найдены"));
}
#[test]
fn damaged_and_backward_dat_report_line_and_do_not_create_output() {
    let t = Temp::new();
    let p = t.file("back_A0.dat", "1.004 1\n1.000 2\n");
    for input in [p, fixture("broken.dat")] {
        let output = t.0.join("output");
        let e = external::import(&input, Some(&output), &t.0, &explicit(), &Config::default())
            .expect_err("bad");
        assert!(e.contains("line 2"), "{e}");
        assert!(e.contains(&input.display().to_string()));
        assert!(!output.exists());
    }
}
#[test]
fn rate_estimate_uses_long_windows_not_zero_intervals() {
    let t = Temp::new();
    let text = (0..1025)
        .map(|i| format!("{:.3} {}\n", (i / 2) as f64 * 0.008, 1 + i % 7))
        .collect::<String>();
    let p = t.file("rate_A0.dat", &text);
    let out = t.import(&p, &Options::default());
    let m = read_metadata(&out).expect("metadata");
    assert!((m.config.source.sample_rate_hz - 250.).abs() < 1e-8);
    assert_eq!(
        m.provenance.expect("provenance").sample_rate_source,
        "estimated"
    );
    let e = external::prepare(
        &fixture("bitronics/demo_A0.dat"),
        &Options::default(),
        &Config::default(),
    )
    .err()
    .expect("short");
    assert!(e.contains("--sample-rate"));
}
#[test]
fn csv_headers_bom_one_two_channels_and_delimiters() {
    let t = Temp::new();
    for (file, n) in [("one.csv", 1), ("two.csv", 2), ("bom.csv", 2)] {
        let p = t.import(&fixture(file), &Options::default());
        let b = blocks(&p);
        assert_eq!(b[0].channels.len(), n);
        assert_eq!(b[0].channels[0].samples[1].timestamp_ns, 4_000_000);
    }
    for delimiter in [",", ";", "\t", " "] {
        for channels in [1, 2] {
            let rows = if channels == 1 {
                vec![vec!["0.000", "1"], vec!["0.004", "2"]]
            } else {
                vec![vec!["0.000", "1", "3"], vec!["0.004", "2", "4"]]
            };
            let text = rows
                .iter()
                .map(|r| r.join(delimiter))
                .collect::<Vec<_>>()
                .join("\r\n");
            let input = t.file("no_header.csv", &text);
            let p = t.import(&input, &Options::default());
            assert_eq!(blocks(&p)[0].channels.len(), channels);
        }
    }
}
#[test]
fn missing_cells_are_omitted_not_zero_and_export_preserves_timestamps() {
    let t = Temp::new();
    let p = t.import(&fixture("missing.csv"), &Options::default());
    let b = blocks(&p);
    let a = &b[0].channels[0].samples;
    assert_eq!(a.iter().map(|s| s.sequence).collect::<Vec<_>>(), [0, 2]);
    let out = t.0.join("flat.csv");
    external::export_csv(&p, &out).expect("export");
    let mut csv = csv::Reader::from_path(&out).expect("csv");
    assert_eq!(
        csv.headers().expect("header").iter().collect::<Vec<_>>(),
        ["timestamp", "channel", "value"]
    );
    let records = csv
        .records()
        .collect::<std::result::Result<Vec<_>, _>>()
        .expect("rows");
    assert_eq!(records.len(), 4);
    assert!(records.iter().all(|r| r.get(2) != Some("0")));
    assert!(records.iter().any(|r| r.get(0) == Some("0.008000000")));
    assert!(external::export_csv(&p, &out).is_err());
    assert!(p.join("events.csv").exists());
}
#[test]
fn csv_no_time_needs_rate_and_explicit_columns_resolve_ambiguity() {
    let t = Temp::new();
    assert!(external::prepare(
        &fixture("no_time.csv"),
        &Options::default(),
        &Config::default()
    )
    .is_err());
    let p = t.import(&fixture("no_time.csv"), &explicit());
    assert_eq!(blocks(&p)[0].channels[0].samples[1].timestamp_ns, 4_000_000);
    let p = t.file("many.csv", "time,x,y,label\n0.000,1,2,3\n0.004,4,5,6\n");
    assert!(external::prepare(&p, &Options::default(), &Config::default()).is_err());
    let o = Options {
        channel_columns: vec!["x".into()],
        ..Options::default()
    };
    let out = t.import(&p, &o);
    assert_eq!(blocks(&out)[0].channels[0].samples[1].value, 4.);
}
#[test]
fn invalid_csv_nonfinite_time_columns_and_limits_are_rejected() {
    let t = Temp::new();
    for text in [
        "time,x\n0,1\n0.004,2,3\n",
        "time,x\n0,NaN\n",
        "time,x\n0,inf\n",
        "time,x\n0,1\n0,2\n",
        "time,x\n0.004,1\n0,2\n",
    ] {
        let p = t.file("bad.csv", text);
        assert!(external::prepare(&p, &explicit(), &Config::default()).is_err());
    }
    let p = t.file("large.csv", &"x".repeat(70_000));
    let e = external::prepare(&p, &explicit(), &Config::default())
        .err()
        .expect("too large");
    assert!(e.contains("64 KiB"));
}
#[test]
fn timestamp_ns_is_exact_and_native_raw_csv_uses_existing_reader() {
    let t = Temp::new();
    let p = t.file(
        "exact.csv",
        "timestamp_ns,x\n1700000000000000001,1\n1700000000004000002,2\n",
    );
    let out = t.import(&p, &explicit());
    let b = blocks(&out);
    assert_eq!(b[0].channels[0].samples[1].timestamp_ns, 4_000_001);
    let prepared = external::prepare(
        &out.join("raw.csv"),
        &Options::default(),
        &Config::default(),
    )
    .expect("native");
    assert_eq!(prepared.path, out);
    drop(prepared);
    assert!(out.exists());
}
#[test]
fn time_gap_creates_sequence_break_without_inventing_samples() {
    let t = Temp::new();
    for (name, text) in [
        ("gap.csv", "time,ch1\n0.000,1\n0.004,2\n1.000,3\n1.004,4\n"),
        ("gap_A0.dat", "0.000 1\n0.004 2\n1.000 3\n1.004 4\n"),
    ] {
        let input = t.file(name, text);
        let p = t.import(&input, &explicit());
        let b = blocks(&p);
        let s: Vec<_> = b.iter().flat_map(|b| &b.channels[0].samples).collect();
        assert_eq!(s.len(), 4);
        assert_eq!(s[2].value, 3.);
        assert_eq!(s[2].timestamp_ns, 1_000_000_000);
        assert!(s[2].sequence > s[1].sequence + 1);
        assert!(fs::read_to_string(p.join("events.csv"))
            .expect("events")
            .contains("ImportGap"));
    }
}
#[test]
fn explicit_columns_do_not_drop_headerless_row_with_unselected_text() {
    let t = Temp::new();
    let p = t.file("labels.csv", "0.000,1,word\n0.004,2,other\n");
    let o = Options {
        time_column: Some("0".into()),
        channel_columns: vec!["1".into()],
        ..Options::default()
    };
    let output = t.import(&p, &o);
    let b = blocks(&output);
    assert_eq!(b[0].channels[0].samples[0].value, 1.);
}
#[test]
fn temporary_session_is_removed_and_permanent_output_is_never_overwritten() {
    let temp = external::prepare(&fixture("one.csv"), &Options::default(), &Config::default())
        .expect("prepare");
    let p = temp.path.clone();
    assert_eq!(blocks(&p)[0].channels[0].samples.len(), 3);
    drop(temp);
    assert!(!p.exists());
    let t = Temp::new();
    let out = t.0.join("session");
    external::import(
        &fixture("one.csv"),
        Some(&out),
        &t.0,
        &Options::default(),
        &Config::default(),
    )
    .expect("first");
    let original = fs::read(out.join("raw.csv")).expect("read");
    assert!(external::import(
        &fixture("two.csv"),
        Some(&out),
        &t.0,
        &Options::default(),
        &Config::default()
    )
    .is_err());
    assert_eq!(fs::read(out.join("raw.csv")).expect("read"), original);
}
