use core_types::*;
use processing::Processor;
use source::{parse_test_line, SignalSource, SourcePoll, Synthetic};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};
use storage::{Metadata, SessionReader, SessionWriter};

fn config() -> Config {
    let mut c = Config::default();
    c.source.channels = 1;
    c.source.noise = 0.;
    c.source.drift = 0.;
    c.source.mains = 0.;
    c
}
fn tone(start: u64, count: usize, sequence: u64, hz: f64, amplitude: f64) -> RawSignalBlock {
    RawSignalBlock {
        sequence,
        started_at: start * 4_000_000,
        sample_rate_hz: 250.,
        source: "test".into(),
        synchronized_clock: true,
        flags: vec![],
        channels: vec![ChannelSamples {
            id: "ch1".into(),
            device: "test-device".into(),
            samples: (start..start + count as u64)
                .map(|i| Sample {
                    sequence: i,
                    timestamp_ns: i * 4_000_000,
                    value: amplitude * (std::f64::consts::TAU * hz * i as f64 / 250.).sin(),
                    flags: vec![],
                })
                .collect(),
        }],
    }
}
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static ID: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "eeg-test-{}-{}",
            std::process::id(),
            ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&p).expect("temp dir");
        Self(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn parser_accepts_only_the_configured_number_of_finite_values() {
    assert_eq!(parse_test_line(" 123\r\n", 1).expect("valid"), vec![123.]);
    assert_eq!(
        parse_test_line("-1.5, 2e2", 2).expect("valid"),
        vec![-1.5, 200.]
    );
    for line in ["", "1,2,3", "NaN,2", "inf,0", "abc,2", "1,", "1"] {
        assert!(parse_test_line(line, 2).is_err(), "{line}");
    }
    assert!(parse_test_line("1,2", 1).is_err());
    assert!(parse_test_line("1", 0).is_err());
}
#[test]
fn config_checks_nyquist_and_resources() {
    let mut c = config();
    c.source.sample_rate_hz = 100.;
    assert!(c.validate().is_err());
    c.processing.low_pass_hz = 40.;
    assert!(c.validate().is_err()); // notch is at Nyquist
    c.processing.notch = false;
    assert!(c.validate().is_ok());
    c.processing.bands[0].high_hz = 51.;
    assert!(c.validate().is_err());
    c = config();
    c.source.channels = 3;
    assert!(c.validate().is_err());
    c = config();
    c.processing.hop_samples = 0;
    assert!(c.validate().is_err());
    c = config();
    c.source.replay_speed = f64::NAN;
    assert!(c.validate().is_err());
}
#[test]
fn source_seed_reproduces_values_and_anomalies() {
    let mut c = Config::default().source;
    c.noise = 3.;
    c.anomalies.skip_every = 9;
    c.anomalies.spike_every = 11;
    c.anomalies.spike_amplitude = 900.;
    c.anomalies.channel2_delay_ms = 12.;
    let mut a = Synthetic::new(c.clone());
    let mut b = Synthetic::new(c.clone());
    for _ in 0..5 {
        assert_eq!(a.generate(), b.generate());
    }
    let mut original = Synthetic::new(c.clone());
    c.seed += 1;
    let mut different = Synthetic::new(c);
    assert_ne!(different.generate(), original.generate());
}
#[test]
fn rings_and_subscriptions_remain_bounded_without_stalling_other_consumers() {
    let mut ring = RingBuffer::new(3);
    for i in 0..100 {
        ring.push(i);
    }
    assert_eq!(ring.iter().copied().collect::<Vec<_>>(), vec![97, 98, 99]);
    let mut zero = RingBuffer::new(0);
    zero.push(1);
    assert!(zero.is_empty());
    let mut bus = Broadcast::default();
    let slow = bus.subscribe(2);
    let fast = bus.subscribe(1);
    for i in 0..100 {
        bus.publish(i);
        assert_eq!(*fast.receiver.recv().expect("consumer"), i);
    }
    assert_eq!(slow.dropped.load(Ordering::Relaxed), 98);
    assert_eq!(slow.receiver.try_iter().count(), 2);
}
#[test]
fn detects_missing_sequences_and_does_not_interpolate_raw() {
    let c = config();
    let mut p = Processor::new(&c).expect("processor");
    p.process(tone(0, 25, 0, 10., 2.)).expect("first");
    let b = tone(28, 25, 1, 10., 2.);
    let originals = b.channels.clone();
    let output = p.process(b).expect("second");
    assert_eq!(output.lost_samples, 3);
    assert!(output.flags.contains(&SignalFlag::LostSample));
    assert_eq!(output.raw_channels, originals);
    assert!(output.flags.contains(&SignalFlag::InsufficientData));
}
#[test]
fn channel_count_and_sample_rate_changes_are_rejected() {
    let mut p = Processor::new(&config()).expect("processor");
    let mut b = tone(0, 25, 0, 10., 2.);
    b.channels.push(b.channels[0].clone());
    assert!(p.process(b).is_err());
    let mut b = tone(0, 25, 0, 10., 2.);
    b.sample_rate_hz = 0.;
    assert!(p.process(b).is_err());
}
#[test]
fn streaming_filter_is_invariant_to_block_boundaries() {
    let c = config();
    let mut whole = Processor::new(&c).expect("processor");
    let mut split = Processor::new(&c).expect("processor");
    let expected = whole
        .process(tone(0, 1000, 0, 10., 2.))
        .expect("whole")
        .filtered_channels
        .remove(0)
        .samples;
    let mut actual = Vec::new();
    for i in 0..40 {
        actual.extend(
            split
                .process(tone(i * 25, 25, i, 10., 2.))
                .expect("split")
                .filtered_channels
                .remove(0)
                .samples,
        );
    }
    assert_eq!(actual, expected);
    assert!(actual[30].is_some_and(|v| v.abs() > 0.1));
}
#[test]
fn a_short_window_is_a_normal_state() {
    let mut p = Processor::new(&config()).expect("processor");
    let b = p.process(tone(0, 100, 0, 10., 2.)).expect("short");
    assert!(b.spectrum.is_none());
    assert!(b.band_power.is_none());
    assert!(b.flags.contains(&SignalFlag::InsufficientData));
}
#[test]
fn psd_recovers_tone_power_relative_power_and_baseline() {
    let mut c = config();
    c.processing.baseline = vec![vec![1., 1., 1.]];
    let mut p = Processor::new(&c).expect("processor");
    let b = p.process(tone(0, 2000, 0, 10., 2.)).expect("spectrum");
    let spectrum = b.spectrum.expect("enough data");
    let ch = &spectrum.channels[0];
    let peak = ch
        .filtered_psd
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .expect("peak")
        .0;
    assert!((spectrum.frequencies[peak] - 10.).abs() < 0.51);
    let powers = b.band_power.expect("bands");
    let alpha = &powers[1];
    assert!((alpha.absolute - 2.).abs() < 0.05, "{}", alpha.absolute);
    assert!(alpha.relative > 0.99);
    assert!((alpha.baseline_change_pct.expect("baseline") - 100.).abs() < 5.);
    assert!(powers[0].relative < 0.001 && powers[2].relative < 0.001);
}
#[test]
fn notch_reduces_fifty_hz_while_preserving_ten_hz() {
    let c = config();
    let mut enabled = Processor::new(&c).expect("processor");
    let mut off = c.clone();
    off.processing.notch = false;
    let mut disabled = Processor::new(&off).expect("processor");
    let mut input = tone(0, 4000, 0, 10., 20.);
    for sample in &mut input.channels[0].samples {
        sample.value += 12. * (std::f64::consts::TAU * 50. * sample.sequence as f64 / 250.).sin();
    }
    let a = enabled
        .process(input.clone())
        .expect("enabled")
        .spectrum
        .expect("spectrum");
    let b = disabled
        .process(input)
        .expect("disabled")
        .spectrum
        .expect("spectrum");
    assert!(a.channels[0].raw_psd[100] > 50.);
    assert!(a.channels[0].filtered_psd[100] / b.channels[0].filtered_psd[100] < 0.001);
    assert!((a.channels[0].filtered_psd[20] / b.channels[0].filtered_psd[20] - 1.).abs() < 0.02);
}
#[test]
fn invalid_samples_are_preserved_but_not_fed_to_filters() {
    let mut p = Processor::new(&config()).expect("processor");
    let mut input = tone(0, 25, 0, 10., 2.);
    input.channels[0].samples[5].value = f64::NAN;
    let output = p.process(input).expect("survives");
    assert!(output.raw_channels[0].samples[5].value.is_nan());
    assert_eq!(output.filtered_channels[0].samples[5], None);
    assert_eq!(output.invalid_samples, 1);
    assert!(output.filtered_channels[0].samples[6].is_some());
}
#[test]
fn artifacts_and_channel_skew_affect_quality_and_disable_asymmetry() {
    let mut c = Config::default();
    c.source.block_size = 1000;
    c.source.anomalies.channel2_delay_ms = 20.;
    c.source.anomalies.spike_every = 11;
    c.source.anomalies.spike_amplitude = 900.;
    let input = Synthetic::new(c.source.clone()).generate().expect("block");
    let output = Processor::new(&c)
        .expect("processor")
        .process(input)
        .expect("output");
    assert!(output.outliers > 0);
    assert!(output.flags.contains(&SignalFlag::Saturation));
    assert!(output.flags.contains(&SignalFlag::ChannelMismatch));
    assert_eq!(output.skew_ms, Some(20.));
    assert!(output.channel_quality.contains(&SignalQuality::Poor));
    assert!(output
        .band_power
        .expect("bands")
        .iter()
        .all(|b| b.asymmetry.is_none()));
}
#[test]
fn asymmetry_exists_for_synchronized_channels() {
    let mut c = Config::default();
    c.source.block_size = 1000;
    let input = Synthetic::new(c.source.clone()).generate().expect("block");
    let output = Processor::new(&c)
        .expect("processor")
        .process(input)
        .expect("output");
    assert!(output
        .band_power
        .expect("bands")
        .iter()
        .all(|b| b.asymmetry.is_some()));
}
#[test]
fn session_roundtrip_preserves_raw_values_sequences_flags_and_timing() {
    let temp = Temp::new();
    let c = config();
    let mut w = SessionWriter::create(&temp.0, &Metadata::new(c)).expect("writer");
    let mut a = tone(500, 25, 20, 10., 2.);
    a.flags.push(SignalFlag::LostSample);
    a.channels[0].samples[2].flags.push(SignalFlag::Outlier);
    let b = tone(530, 25, 21, 10., 2.);
    w.write_block(&a).expect("write a");
    w.write_block(&b).expect("write b");
    w.write_event(&Event {
        timestamp_ns: a.started_at,
        kind: "Marker".into(),
        text: "русская метка, \"quoted\"\nnext".into(),
    })
    .expect("marker");
    w.flush().expect("flush");
    let mut r = SessionReader::open(&w.directory).expect("reader");
    assert_eq!(r.next_block().expect("a"), Some(a));
    assert_eq!(r.next_block().expect("b"), Some(b));
    assert!(r.next_block().expect("eof").is_none());
}
struct FastSource {
    index: u64,
}
impl SignalSource for FastSource {
    fn poll(&mut self) -> Result<SourcePoll> {
        if self.index == 100 {
            return Ok(SourcePoll::End);
        }
        let b = tone(self.index * 25, 25, self.index, 10., 2.);
        self.index += 1;
        Ok(SourcePoll::Block(b))
    }
}
fn wait_finished(runtime: &app::Runtime) {
    let start = Instant::now();
    loop {
        if runtime.status.lock().expect("status").finished {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "pipeline did not terminate"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}
#[test]
fn raw_recording_and_second_consumer_are_independent_of_tui() {
    for hz in [1, 60] {
        let temp = Temp::new();
        let mut c = config();
        c.tui_hz = hz;
        c.recording_dir = temp.0.display().to_string();
        let runtime =
            app::Runtime::start(c, Box::new(FastSource { index: 0 }), true).expect("runtime");
        wait_finished(&runtime);
        assert!(
            runtime
                .subscription
                .as_ref()
                .expect("ui subscription")
                .dropped
                .load(Ordering::Relaxed)
                > 0
        );
        let status = runtime.shutdown().expect("shutdown");
        assert_eq!(status.processed_blocks, 100);
        assert_eq!(status.consumer_blocks + status.consumer_dropped, 100);
        assert!(status.consumer_blocks > 0);
        let file = PathBuf::from(status.recording.expect("recording"));
        let mut reader = SessionReader::open(file.parent().expect("parent")).expect("reader");
        let mut count = 0;
        while let Some(block) = reader.next_block().expect("read") {
            count += block.channels[0].samples.len();
        }
        assert_eq!(count, 2500);
    }
}
#[test]
fn pipeline_finishes_after_source_disconnect() {
    let mut c = config();
    c.source.block_size = 5;
    c.source.anomalies.disconnect_after_samples = Some(15);
    let source = Synthetic::new(c.source.clone());
    let runtime = app::Runtime::start(c, Box::new(source), false).expect("runtime");
    wait_finished(&runtime);
    let status = runtime.shutdown().expect("shutdown");
    assert_eq!(status.received_samples, 15);
    assert_eq!(status.connection, "Disconnected");
    assert!(status.events.iter().any(|e| e.kind == "Disconnected"));
}
#[test]
fn replay_preserves_source_time_at_changed_speed() {
    let temp = Temp::new();
    let mut writer = SessionWriter::create(&temp.0, &Metadata::new(config())).expect("writer");
    let a = tone(500, 25, 20, 10., 2.);
    let b = tone(750, 25, 21, 10., 2.);
    writer.write_block(&a).expect("a");
    writer.write_block(&b).expect("b");
    writer.flush().expect("flush");
    let mut replay = source::Replay::open(&writer.directory, 20.).expect("replay");
    let start = Instant::now();
    let mut blocks = Vec::new();
    loop {
        match replay.poll().expect("poll") {
            SourcePoll::Block(b) => blocks.push(b),
            SourcePoll::End => break,
            _ => std::thread::sleep(Duration::from_millis(1)),
        }
        assert!(start.elapsed() < Duration::from_secs(2));
    }
    assert_eq!(blocks, vec![a, b]);
    assert!(start.elapsed() >= Duration::from_millis(45));
}

#[test]
fn external_consumer_attaches_without_changing_tui_or_processor() {
    let mut bus = Broadcast::default();
    let external = bus.subscribe(128);
    let runtime =
        app::Runtime::start_with_bus(config(), Box::new(FastSource { index: 0 }), false, bus)
            .expect("runtime");
    wait_finished(&runtime);
    let status = runtime.shutdown().expect("shutdown");
    let blocks: Vec<_> = external.receiver.try_iter().collect();
    assert_eq!(blocks.len() as u64, status.processed_blocks);
    assert_eq!(blocks[99].sequence, 99);
}
#[test]
fn replay_includes_multiline_markers_with_original_timestamps() {
    let temp = Temp::new();
    let mut writer = SessionWriter::create(&temp.0, &Metadata::new(config())).expect("writer");
    writer.write_block(&tone(0, 25, 0, 10., 2.)).expect("block");
    let label = "метка, \"кавычки\"\nстрока";
    writer
        .write_event(&Event {
            timestamp_ns: 50_000_000,
            kind: "Marker".into(),
            text: label.into(),
        })
        .expect("event");
    writer.flush().expect("flush");
    let mut replay = source::Replay::open(&writer.directory, 100.).expect("replay");
    let start = Instant::now();
    let mut found = false;
    loop {
        match replay.poll().expect("poll") {
            SourcePoll::Event(e) => {
                assert_eq!(e.timestamp_ns, 50_000_000);
                assert_eq!(e.text, label);
                assert_eq!(e.kind, "Replay/Marker");
                found = true;
            }
            SourcePoll::End => break,
            _ => std::thread::sleep(Duration::from_millis(1)),
        }
        assert!(start.elapsed() < Duration::from_secs(1));
    }
    assert!(found);
}
#[test]
fn oversized_or_corrupt_csv_reports_errors() {
    let temp = Temp::new();
    let mut writer = SessionWriter::create(&temp.0, &Metadata::new(config())).expect("writer");
    writer.write_block(&tone(0, 25, 0, 10., 2.)).expect("block");
    writer.flush().expect("flush");
    let file = writer.directory.join("raw.csv");
    let original = fs::read_to_string(&file).expect("csv");
    let header = original.lines().next().expect("header");
    fs::write(&file, format!("{header}\n\"{}", "x".repeat(100_000))).expect("corrupt file");
    let mut reader = SessionReader::open(&writer.directory).expect("header");
    assert!(reader.next_block().is_err());
    fs::write(&file, "not,our,header\n1,2,3\n").expect("bad header");
    assert!(SessionReader::open(&writer.directory).is_err());
}
#[test]
fn asynchronous_channels_keep_individual_sequences_and_empty_cells() {
    let temp = Temp::new();
    let c = Config::default();
    let mut writer = SessionWriter::create(&temp.0, &Metadata::new(c)).expect("writer");
    let mut block = tone(0, 1, 0, 10., 2.);
    block.synchronized_clock = false;
    block.channels.push(ChannelSamples {
        id: "ch2".into(),
        device: "second-port".into(),
        samples: Vec::new(),
    });
    writer.write_block(&block).expect("write");
    writer.flush().expect("flush");
    let mut reader = SessionReader::open(&writer.directory).expect("reader");
    assert_eq!(reader.next_block().expect("read"), Some(block));
}
#[test]
fn invalid_channel_identity_does_not_reuse_another_channels_filter() {
    let mut p = Processor::new(&config()).expect("processor");
    p.process(tone(0, 25, 0, 10., 2.)).expect("first");
    let mut second = tone(25, 25, 1, 10., 2.);
    second.channels[0].device = "different-device".into();
    assert!(p.process(second).is_err());
}

#[test]
fn channels_do_not_share_filter_state() {
    let c = Config::default();
    let mut block = tone(0, 1000, 0, 10., 2.);
    let mut silent = block.channels[0].clone();
    silent.id = "ch2".into();
    for s in &mut silent.samples {
        s.value = 0.;
    }
    block.channels.push(silent);
    let output = Processor::new(&c)
        .expect("processor")
        .process(block)
        .expect("output");
    assert!(output.filtered_channels[1]
        .samples
        .iter()
        .all(|v| *v == Some(0.)));
    assert!(output.filtered_channels[0]
        .samples
        .iter()
        .flatten()
        .any(|v| v.abs() > 1.));
}
#[test]
fn overlapping_bands_do_not_inflate_relative_power_denominator() {
    let mut c = config();
    c.processing.bands.push(Band {
        name: "Mu".into(),
        low_hz: 8.,
        high_hz: 13.,
    });
    let mut block = tone(0, 2000, 0, 10., 2.);
    for sample in &mut block.channels[0].samples {
        sample.value += (std::f64::consts::TAU * 20. * sample.sequence as f64 / 250.).sin();
    }
    let powers = Processor::new(&c)
        .expect("processor")
        .process(block)
        .expect("output")
        .band_power
        .expect("bands");
    assert!((powers[1].relative - 0.8).abs() < 0.01);
    assert!((powers[2].relative - 0.2).abs() < 0.01);
    assert_eq!(powers[1].relative, powers[3].relative);
}

#[test]
fn delayed_channel_disconnect_does_not_move_event_time_backwards() {
    let temp = Temp::new();
    let mut c = Config::default();
    c.source.block_size = 5;
    c.source.anomalies.channel2_delay_ms = 12.;
    c.source.anomalies.disconnect_after_samples = Some(15);
    c.recording_dir = temp.0.display().to_string();
    let source = Synthetic::new(c.source.clone());
    let runtime = app::Runtime::start(c, Box::new(source), true).expect("runtime");
    wait_finished(&runtime);
    let status = runtime.shutdown().expect("shutdown");
    assert!(!status.recording_active);
    let raw = PathBuf::from(status.recording.expect("recording"));
    let mut events = storage::EventReader::open(raw.parent().expect("parent")).expect("events");
    let mut count = 0;
    while events
        .next_event()
        .expect("monotonic event times")
        .is_some()
    {
        count += 1;
    }
    assert!(count >= 3);
}
#[test]
fn within_block_misalignment_disables_asymmetry_even_if_last_samples_match() {
    let c = Config::default();
    let mut block = tone(0, 1000, 0, 10., 2.);
    let mut other = block.channels[0].clone();
    other.id = "ch2".into();
    // Smooth bounded distortion keeps each channel's timestamps monotonic.
    for sample in &mut other.samples {
        if sample.sequence < 900 {
            sample.timestamp_ns +=
                ((std::f64::consts::PI * sample.sequence as f64 / 900.).sin() * 20_000_000.) as u64;
        }
    }
    block.channels.push(other);
    let output = Processor::new(&c)
        .expect("processor")
        .process(block)
        .expect("output");
    assert!(output.flags.contains(&SignalFlag::ChannelMismatch));
    assert!(output
        .band_power
        .expect("powers")
        .iter()
        .all(|b| b.asymmetry.is_none()));
}
