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

// Construct the verified binary profile, never use participant recordings as fixtures.
fn dmov_packet(tick: u32, payload: &[u8]) -> Vec<u8> {
    let mut bytes = ((5 + payload.len()) as u32).to_be_bytes().to_vec();
    bytes.push(b'T');
    bytes.extend(tick.to_le_bytes());
    bytes.extend(payload);
    bytes
}

#[test]
fn dmov_recovers_split_fields_and_validates_dat_without_scaling_raw() {
    let t = Temp::new();
    let input = t.0.join("запись.dmov");
    let mut bytes = dmov_packet(12207, b"A0\x41A2\xffA");
    bytes.extend(dmov_packet(12707, b"0\x30A2\x31A7\x00"));
    fs::write(&input, &bytes).expect("fixture");
    let row = |tick: u32, value: u8| {
        format!(
            "{:.8} {:.8}\n",
            (tick as f64 * 8192. / 1e9) as f32,
            (value as f64 / 255. * 5.) as f32
        )
    };
    t.file("запись_EMG.dat", &row(12207, 65));
    t.file("запись_EEG.dat", &(row(12207, 255) + &row(12707, 49)));
    let out = t.import(&input, &explicit());
    let data = blocks(&out);
    assert!(data.iter().all(|b| !b.synchronized_clock));
    for (ch, expected) in [[65., 48.], [255., 49.]].iter().enumerate() {
        let samples: Vec<_> = data.iter().flat_map(|b| &b.channels[ch].samples).collect();
        assert_eq!(
            samples.iter().map(|s| s.value).collect::<Vec<_>>(),
            *expected
        );
        assert_eq!(
            samples.iter().map(|s| s.timestamp_ns).collect::<Vec<_>>(),
            [0, 4_000_000]
        );
        assert_eq!(
            samples.iter().map(|s| s.sequence).collect::<Vec<_>>(),
            [0, 1]
        );
    }
    let meta = read_metadata(&out).expect("metadata");
    assert_eq!(meta.channel_ids, ["A0", "A2"]);
    assert_eq!(meta.units.as_deref(), Some("adc_counts_u8"));
    let provenance = serde_json::to_value(meta.provenance).expect("provenance");
    assert_eq!(
        provenance["source_channel_map"]["A0"],
        serde_json::Value::Null
    );
    assert_eq!(
        provenance["validation_against_dat"]["A0"]["matched_rows"],
        1
    );
    assert_eq!(
        provenance["validation_against_dat"]["A0"]["cross_record_fields_recovered"],
        1
    );
    let events = fs::read_to_string(out.join("events.csv")).expect("events");
    assert!(events.contains("12207") && events.contains("12707"));
    assert_eq!(fs::read(input).expect("input preserved"), bytes);
}

#[test]
fn dmov_missing_channel_is_empty_and_time_gap_is_an_event() {
    for tag in b"02".iter().copied() {
        let t = Temp::new();
        let input = t.0.join("missing.dmov");
        let mut bytes = dmov_packet(100, &[b'A', tag, 1]);
        bytes.extend(dmov_packet(110, &[b'A', tag, 2]));
        bytes.extend(dmov_packet(20000, &[b'A', tag, 3]));
        fs::write(&input, bytes).expect("fixture");
        let out = t.import(&input, &explicit());
        let data = blocks(&out);
        let selected = usize::from(tag == b'2');
        assert!(data
            .iter()
            .all(|b| b.channels[1 - selected].samples.is_empty()));
        let samples: Vec<_> = data
            .iter()
            .flat_map(|b| &b.channels[selected].samples)
            .collect();
        assert_eq!(samples.len(), 3);
        assert!(samples
            .windows(2)
            .all(|s| s[0].timestamp_ns < s[1].timestamp_ns));
        assert!(samples[2]
            .flags
            .contains(&core_types::SignalFlag::LostSample));
        assert!(fs::read_to_string(out.join("events.csv"))
            .expect("events")
            .contains("ImportGap"));
    }
}

#[test]
fn dmov_rejects_corruption_with_offset_before_creating_output() {
    let mut truncated = dmov_packet(100, b"A0\x01A2\x02");
    truncated.pop();
    let mut backwards = dmov_packet(100, b"A0\x01");
    backwards.extend(dmov_packet(99, b"A2\x02"));
    let cases = vec![
        vec![],
        vec![1, 2],
        vec![0, 0, 0, 5, b'T'],
        vec![0, 0, 16, 1, b'T'],
        truncated,
        backwards,
        dmov_packet(100, b"A0"),
        dmov_packet(100, b"B0\x01"),
        vec![255; 5000],
    ];
    for bytes in cases {
        let t = Temp::new();
        let input = t.0.join("broken.dmov");
        fs::write(&input, bytes).expect("fixture");
        let out = t.0.join("out");
        let err = external::import(&input, Some(&out), &t.0, &explicit(), &Config::default())
            .expect_err("reject");
        assert!(err.contains("byte") && err.contains("DMOV"), "{err}");
        assert!(!out.exists());
    }
}

#[test]
fn dmov_mismatched_companion_is_not_silently_accepted() {
    let t = Temp::new();
    let input = t.0.join("x.dmov");
    fs::write(&input, dmov_packet(100, b"A0\x01A2\x02")).expect("fixture");
    t.file("x_EMG.dat", "0 99\n");
    let err = external::import(&input, None, &t.0, &explicit(), &Config::default())
        .expect_err("mismatch");
    assert!(err.contains("validation") && err.contains("line"), "{err}");
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
    assert!(e.contains("DMOV") && e.contains("byte 0"), "{e}");
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
