use core_types::Config;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
use study::{run, StudyRequest};

struct Temp(PathBuf);

impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "lemon-study-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("temp directory");
        Self(path)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn config() -> Config {
    let mut config = Config::default();
    config.source.channels = 1;
    config.source.sample_rate_hz = 250.0;
    config.source.units = Some("adc_counts_u8".into());
    config.processing.window_samples = 8;
    config.processing.hop_samples = 4;
    config.processing.filter_warmup_seconds = 0.0;
    config.processing.low_pass_hz = 80.0;
    config
}

fn dmov_packet(tick: u32, payload: &[u8]) -> Vec<u8> {
    let mut bytes = ((5 + payload.len()) as u32).to_be_bytes().to_vec();
    bytes.push(b'T');
    bytes.extend(tick.to_le_bytes());
    bytes.extend(payload);
    bytes
}

fn fixtures(root: &Path) -> (PathBuf, Vec<u8>) {
    fs::create_dir(root.join("background")).expect("background");
    fs::create_dir(root.join("right")).expect("right");
    fs::create_dir(root.join("imagined")).expect("imagined");
    let csv = (0..40)
        .map(|index| format!("{:.3},{}\n", index as f64 / 250.0, index % 16))
        .collect::<String>();
    fs::write(
        root.join("background/recording.csv"),
        format!("time,A0\n{csv}"),
    )
    .expect("CSV");
    let dat = (0..40)
        .map(|index| format!("{:.3} {}\n", index as f64 / 250.0, index % 12))
        .collect::<String>();
    fs::write(root.join("right/recording_A0.dat"), dat).expect("DAT");
    let mut dmov = Vec::new();
    for index in 0..40u32 {
        dmov.extend(dmov_packet(
            1000 + index * 500,
            &[b'A', b'0', index as u8, b'A', b'2', (index + 1) as u8],
        ));
    }
    fs::write(root.join("imagined/recording.dmov"), &dmov).expect("DMOV");
    (root.join("study.json"), dmov)
}

fn write_manifest(path: &Path, extra: serde_json::Value) {
    let mut value = serde_json::json!({
        "schema_version": 1,
        "id": "lab2-offline",
        "display_name": "Lab 2 offline",
        "description": "Synthetic fixture",
        "baseline": {"label": "background", "input": "background/recording.csv"},
        "conditions": [
            {"label": "right-hand-squeeze", "input": "right/recording_A0.dat"},
            {"label": "imagined-squeeze", "input": "imagined/recording.dmov"}
        ]
    });
    if let (Some(object), Some(extra)) = (value.as_object_mut(), extra.as_object()) {
        object.extend(extra.clone());
    }
    fs::write(path, serde_json::to_vec_pretty(&value).expect("JSON")).expect("manifest");
}

#[test]
fn csv_dat_and_dmov_create_atomic_study_and_relative_plugin_config() {
    let temp = Temp::new();
    let (manifest, dmov_before) = fixtures(&temp.0);
    write_manifest(&manifest, serde_json::json!({}));
    let output = temp.0.join("result");
    let result = run(StudyRequest {
        manifest: manifest.clone(),
        output: output.clone(),
        config: config(),
        overwrite: false,
    })
    .expect("study");

    assert_eq!(result.directory, output);
    for directory in ["background", "right-hand-squeeze", "imagined-squeeze"] {
        assert!(output.join(directory).join("manifest.json").is_file());
        assert!(output.join(directory).join("processed.csv").is_file());
    }
    assert_eq!(
        fs::read(temp.0.join("imagined/recording.dmov")).expect("unchanged DMOV"),
        dmov_before
    );
    let study: serde_json::Value = serde_json::from_reader(
        fs::File::open(output.join("study-manifest.json")).expect("study manifest"),
    )
    .expect("study JSON");
    let expected_hash = study["dsp_config_hash"].as_str().expect("config hash");
    assert!(expected_hash.starts_with("sha256:"));
    assert_eq!(study["baseline"]["dsp_config_hash"], expected_hash);
    for condition in study["conditions"].as_array().expect("conditions") {
        assert_eq!(condition["dsp_config_hash"], expected_hash);
        assert!(condition["source_hash"]
            .as_str()
            .is_some_and(|hash| hash.starts_with("sha256:")));
    }
    let plugin: serde_json::Value = serde_json::from_reader(
        fs::File::open(output.join("mu-condition-analysis.json")).expect("plugin config"),
    )
    .expect("plugin JSON");
    assert_eq!(plugin["channels"], serde_json::json!(["A0"]));
    assert_eq!(
        plugin["conditions"][0]["analysis_bundle"],
        "right-hand-squeeze"
    );
    assert!(plugin["conditions"]
        .as_array()
        .expect("conditions")
        .iter()
        .all(
            |condition| !Path::new(condition["analysis_bundle"].as_str().expect("bundle path"))
                .is_absolute()
        ));

    fs::write(temp.0.join("outside.txt"), "outside").expect("outside file");
    run(StudyRequest {
        manifest,
        output,
        config: config(),
        overwrite: true,
    })
    .expect("overwrite the selected study only");
    assert_eq!(
        fs::read_to_string(temp.0.join("outside.txt")).expect("outside unchanged"),
        "outside"
    );
}

#[test]
fn invalid_manifest_roles_and_unknown_fields_are_rejected() {
    let temp = Temp::new();
    let (manifest, _) = fixtures(&temp.0);
    let cases = [
        serde_json::json!({"unknown": true}),
        serde_json::json!({"schema_version": 2}),
        serde_json::json!({"id": "Bad ID"}),
        serde_json::json!({"conditions": []}),
        serde_json::json!({"conditions": [{"label": "", "input": "right/recording_A0.dat"}]}),
        serde_json::json!({"conditions": [{"label": "background", "input": "right/recording_A0.dat"}]}),
        serde_json::json!({"conditions": [{"label": "right", "input": "background/recording.csv"}]}),
        serde_json::json!({"conditions": [{"label": "missing", "input": "missing.csv"}]}),
    ];
    for (index, extra) in cases.into_iter().enumerate() {
        write_manifest(&manifest, extra);
        let error = run(StudyRequest {
            manifest: manifest.clone(),
            output: temp.0.join(format!("invalid-{index}")),
            config: config(),
            overwrite: false,
        })
        .expect_err("invalid manifest");
        assert!(!error.is_empty());
    }
}

#[test]
fn failed_input_leaves_existing_output_and_siblings_unchanged() {
    let temp = Temp::new();
    let (manifest, _) = fixtures(&temp.0);
    fs::write(temp.0.join("right/recording_A0.dat"), "not numeric\n").expect("broken DAT");
    write_manifest(&manifest, serde_json::json!({}));
    let output = temp.0.join("result");
    fs::create_dir(&output).expect("existing output");
    fs::write(output.join("keep.txt"), "previous").expect("existing file");
    fs::write(temp.0.join("sibling.txt"), "sibling").expect("sibling");
    let error = run(StudyRequest {
        manifest,
        output: output.clone(),
        config: config(),
        overwrite: true,
    })
    .expect_err("broken input");
    assert!(error.contains("right-hand-squeeze"), "{error}");
    assert_eq!(
        fs::read_to_string(output.join("keep.txt")).expect("previous output"),
        "previous"
    );
    assert_eq!(
        fs::read_to_string(temp.0.join("sibling.txt")).expect("sibling"),
        "sibling"
    );
    assert!(!temp
        .0
        .read_dir()
        .expect("root")
        .filter_map(Result::ok)
        .any(|entry| entry.file_name().to_string_lossy().contains(".partial")));
}
