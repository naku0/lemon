//! Shared data and bounded in-process subscriptions. Timestamps are source-relative nanoseconds.
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
        Arc,
    },
};
pub type Result<T> = std::result::Result<T, String>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SignalFlag {
    LostSample,
    InvalidPacket,
    Disconnected,
    Outlier,
    Saturation,
    LowQuality,
    ChannelMismatch,
    InsufficientData,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SignalQuality {
    Good,
    Acceptable,
    Poor,
    Disconnected,
    Unknown,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Sample {
    pub sequence: u64,
    pub timestamp_ns: u64,
    pub value: f64,
    pub flags: Vec<SignalFlag>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChannelSamples {
    pub id: String,
    pub device: String,
    pub samples: Vec<Sample>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RawSignalBlock {
    pub sequence: u64,
    pub started_at: u64,
    pub sample_rate_hz: f64,
    pub source: String,
    pub synchronized_clock: bool,
    pub channels: Vec<ChannelSamples>,
    pub flags: Vec<SignalFlag>,
}
#[derive(Debug, Clone)]
pub struct FilteredChannel {
    pub id: String,
    pub samples: Vec<Option<f64>>,
}
#[derive(Debug, Clone)]
pub struct ChannelSpectrum {
    pub timestamp_ns: u64,
    pub channel: String,
    pub raw_psd: Vec<f64>,
    pub filtered_psd: Vec<f64>,
    pub mains_ratio: f64,
}
#[derive(Debug, Clone)]
pub struct SpectrumSnapshot {
    pub timestamp_ns: u64,
    pub frequencies: Vec<f64>,
    pub channels: Vec<ChannelSpectrum>,
}
#[derive(Debug, Clone)]
pub struct BandPower {
    pub channel: String,
    pub band: String,
    pub absolute: f64,
    pub relative: f64,
    pub smoothed: f64,
    pub baseline_change_pct: Option<f64>,
    pub asymmetry: Option<f64>,
}
#[derive(Debug, Clone)]
pub struct ProcessedSignalBlock {
    pub sequence: u64,
    pub started_at: u64,
    pub sample_rate_hz: f64,
    pub raw_channels: Vec<ChannelSamples>,
    pub filtered_channels: Vec<FilteredChannel>,
    pub channel_quality: Vec<SignalQuality>,
    pub spectrum: Option<SpectrumSnapshot>,
    pub band_power: Option<Vec<BandPower>>,
    pub flags: Vec<SignalFlag>,
    pub lost_samples: u64,
    pub invalid_samples: u64,
    pub outliers: u64,
    pub skew_ms: Option<f64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub timestamp_ns: u64,
    pub kind: String,
    pub text: String,
}

/// Bounded history, including the zero-capacity case.
#[derive(Debug, Clone)]
pub struct RingBuffer<T> {
    capacity: usize,
    data: VecDeque<T>,
}
impl<T> RingBuffer<T> {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            data: VecDeque::new(),
        }
    }
    pub fn push(&mut self, value: T) {
        if self.capacity == 0 {
            return;
        }
        if self.data.len() == self.capacity {
            self.data.pop_front();
        }
        self.data.push_back(value);
    }
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.data.iter()
    }
    pub fn len(&self) -> usize {
        self.data.len()
    }
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
    pub fn clear(&mut self) {
        self.data.clear();
    }
}
struct Subscriber<T> {
    sender: SyncSender<Arc<T>>,
    dropped: Arc<AtomicU64>,
}
pub struct Subscription<T> {
    pub receiver: Receiver<Arc<T>>,
    pub dropped: Arc<AtomicU64>,
}
/// Slow optional consumers drop new blocks, never stall acquisition or recording.
/// Lossless raw recording belongs upstream of this fan-out.
pub struct Broadcast<T> {
    subscribers: Vec<Subscriber<T>>,
}
impl<T> Default for Broadcast<T> {
    fn default() -> Self {
        Self {
            subscribers: Vec::new(),
        }
    }
}
impl<T> Broadcast<T> {
    pub fn subscribe(&mut self, capacity: usize) -> Subscription<T> {
        let (sender, receiver) = mpsc::sync_channel(capacity.max(1));
        let dropped = Arc::new(AtomicU64::new(0));
        self.subscribers.push(Subscriber {
            sender,
            dropped: dropped.clone(),
        });
        Subscription { receiver, dropped }
    }
    pub fn publish(&mut self, value: T) {
        let value = Arc::new(value);
        self.subscribers
            .retain(|s| match s.sender.try_send(value.clone()) {
                Ok(()) => true,
                Err(TrySendError::Full(_)) => {
                    s.dropped.fetch_add(1, Ordering::Relaxed);
                    true
                }
                Err(TrySendError::Disconnected(_)) => false,
            });
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub source: SourceConfig,
    pub processing: ProcessingConfig,
    pub tui_hz: u32,
    pub recording_dir: String,
    pub session: SessionConfig,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            source: SourceConfig::default(),
            processing: ProcessingConfig::default(),
            tui_hz: 15,
            recording_dir: "recordings".into(),
            session: SessionConfig::default(),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SessionConfig {
    pub name: String,
    pub user_id: String,
    pub electrodes: Vec<String>,
    pub notes: String,
}
impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            name: "demo".into(),
            user_id: String::new(),
            electrodes: vec![String::new(), String::new()],
            notes: String::new(),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SourceConfig {
    pub mode: String,
    pub channels: usize,
    pub sample_rate_hz: f64,
    pub block_size: usize,
    pub seed: u64,
    pub components: Vec<Component>,
    pub noise: f64,
    pub mains: f64,
    pub drift: f64,
    pub anomalies: Anomalies,
    pub ports: Vec<String>,
    pub baud_rate: u32,
    pub replay_path: String,
    pub replay_speed: f64,
    pub units: Option<String>,
    pub serial_idle_timeout_ms: u64,
}
impl Default for SourceConfig {
    fn default() -> Self {
        Self {
            mode: "synthetic".into(),
            channels: 2,
            sample_rate_hz: 250.,
            block_size: 25,
            seed: 42,
            components: vec![
                Component {
                    hz: 6.,
                    amplitude: 8.,
                },
                Component {
                    hz: 10.,
                    amplitude: 20.,
                },
                Component {
                    hz: 20.,
                    amplitude: 6.,
                },
            ],
            noise: 2.,
            mains: 12.,
            drift: 15.,
            anomalies: Anomalies::default(),
            ports: Vec::new(),
            baud_rate: 115200,
            replay_path: String::new(),
            replay_speed: 1.,
            units: None,
            serial_idle_timeout_ms: 3000,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Component {
    pub hz: f64,
    pub amplitude: f64,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Anomalies {
    pub skip_every: u64,
    pub spike_every: u64,
    pub spike_amplitude: f64,
    pub channel2_delay_ms: f64,
    pub disconnect_after_samples: Option<u64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProcessingConfig {
    pub high_pass_hz: f64,
    pub low_pass_hz: f64,
    pub notch: bool,
    pub notch_hz: f64,
    pub notch_q: f64,
    pub window_samples: usize,
    pub hop_samples: usize,
    pub bands: Vec<Band>,
    pub smoothing: f64,
    pub expected_abs_max: f64,
    pub outlier_step: f64,
    pub flat_stddev: f64,
    pub mains_ratio_limit: f64,
    pub sync_tolerance_ms: f64,
    /// Per channel, per band; power in units squared. Empty means no baseline.
    pub baseline: Vec<Vec<f64>>,
}
impl Default for ProcessingConfig {
    fn default() -> Self {
        Self {
            high_pass_hz: 1.,
            low_pass_hz: 80.,
            notch: true,
            notch_hz: 50.,
            notch_q: 30.,
            window_samples: 500,
            hop_samples: 50,
            bands: vec![
                Band {
                    name: "Theta".into(),
                    low_hz: 4.,
                    high_hz: 8.,
                },
                Band {
                    name: "Alpha/Mu".into(),
                    low_hz: 8.,
                    high_hz: 13.,
                },
                Band {
                    name: "Beta".into(),
                    low_hz: 13.,
                    high_hz: 30.,
                },
            ],
            smoothing: 0.25,
            expected_abs_max: 500.,
            outlier_step: 150.,
            flat_stddev: 0.01,
            mains_ratio_limit: 0.2,
            sync_tolerance_ms: 4.,
            baseline: Vec::new(),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Band {
    pub name: String,
    pub low_hz: f64,
    pub high_hz: f64,
}
impl Config {
    pub fn validate(&self) -> Result<()> {
        let s = &self.source;
        let p = &self.processing;
        let fs = s.sample_rate_hz;
        if !["synthetic", "replay", "serial-test", "bitronics"].contains(&s.mode.as_str()) {
            return Err("source.mode: synthetic | replay | serial-test | bitronics".into());
        }
        if s.anomalies.skip_every == 1 {
            return Err("skip_every: 0 (выключено) или >= 2".into());
        }
        if self.session.name.len() > 256
            || self.session.user_id.len() > 256
            || self.session.notes.len() > 4096
            || self.session.electrodes.len() > 2
            || self.session.electrodes.iter().any(|v| v.len() > 256)
        {
            return Err(
                "Слишком длинные поля сеанса (name/user 256, notes 4096, electrodes 2 × 256)"
                    .into(),
            );
        }
        if !(1..=2).contains(&s.channels) {
            return Err("channels: требуется 1 или 2".into());
        }
        if !fs.is_finite() || !(2.0..=100_000.).contains(&fs) {
            return Err("sample_rate_hz: требуется конечное значение 2..100000".into());
        }
        if !(1..=4096).contains(&s.block_size) || !(1..=60).contains(&self.tui_hz) {
            return Err("block_size: 1..4096; tui_hz: 1..60".into());
        }
        if !(8..=65536).contains(&p.window_samples)
            || p.hop_samples == 0
            || p.hop_samples > p.window_samples
        {
            return Err("window_samples: 8..65536; hop_samples: 1..window_samples".into());
        }
        let nyquist = fs / 2.;
        if !p.high_pass_hz.is_finite()
            || !p.low_pass_hz.is_finite()
            || p.high_pass_hz <= 0.
            || p.low_pass_hz <= p.high_pass_hz
            || p.low_pass_hz >= nyquist
        {
            return Err(format!(
                "Фильтры: 0 < high_pass_hz < low_pass_hz < Найквист ({nyquist} Гц)"
            ));
        }
        if p.notch
            && (!p.notch_hz.is_finite()
                || p.notch_hz <= 0.
                || p.notch_hz >= nyquist
                || !p.notch_q.is_finite()
                || p.notch_q <= 0.)
        {
            return Err("Notch: частота должна быть ниже Найквиста, Q > 0".into());
        }
        if p.bands.is_empty()
            || p.bands.len() > 16
            || p.bands.iter().any(|b| {
                b.name.is_empty()
                    || !b.low_hz.is_finite()
                    || !b.high_hz.is_finite()
                    || b.low_hz < 0.
                    || b.high_hz <= b.low_hz
                    || b.high_hz > nyquist
            })
        {
            return Err("Некорректные частотные диапазоны (границы/Найквист)".into());
        }
        for (i, b) in p.bands.iter().enumerate() {
            if p.bands[..i].iter().any(|a| a.name == b.name) {
                return Err("Имена диапазонов должны быть уникальны".into());
            }
        }
        for (name, value) in [
            ("expected_abs_max", p.expected_abs_max),
            ("outlier_step", p.outlier_step),
            ("flat_stddev", p.flat_stddev),
        ] {
            if !value.is_finite() || value <= 0. {
                return Err(format!("{name}: требуется конечное значение > 0"));
            }
        }
        if !p.smoothing.is_finite()
            || !(0.0..=1.).contains(&p.smoothing)
            || !p.mains_ratio_limit.is_finite()
            || !(0.0..=1.).contains(&p.mains_ratio_limit)
            || !p.sync_tolerance_ms.is_finite()
            || !(0.0..=1000.).contains(&p.sync_tolerance_ms)
        {
            return Err("Некорректные smoothing/mains_ratio_limit/sync_tolerance_ms".into());
        }
        if !p.baseline.is_empty()
            && (p.baseline.len() != s.channels
                || p.baseline.iter().any(|r| {
                    r.len() != p.bands.len() || r.iter().any(|v| !v.is_finite() || *v <= 0.)
                }))
        {
            return Err(
                "baseline: положительная мощность каждого диапазона для каждого канала".into(),
            );
        }
        if !s.replay_speed.is_finite() || !(0.01..=100.).contains(&s.replay_speed) {
            return Err("replay_speed: 0.01..100".into());
        }
        if s.components.len() > 32
            || s.components.iter().any(|c| {
                !c.hz.is_finite()
                    || c.hz <= 0.
                    || c.hz >= nyquist
                    || !c.amplitude.is_finite()
                    || c.amplitude.abs() > 1e6
            })
        {
            return Err("Синусоиды: конечные амплитуды и частоты ниже Найквиста".into());
        }
        for v in [
            s.noise,
            s.mains,
            s.drift,
            s.anomalies.spike_amplitude,
            s.anomalies.channel2_delay_ms,
        ] {
            if !v.is_finite() || !(0.0..=1e6).contains(&v) {
                return Err("Амплитуды/задержка: 0..1000000".into());
            }
        }
        if s.mode == "synthetic" && s.mains > 0. && nyquist <= 50. {
            return Err("Помеха 50 Гц требует sample_rate_hz > 100".into());
        }
        if s.mode == "serial-test"
            && (s.ports.is_empty()
                || s.ports.len() > s.channels
                || s.baud_rate == 0
                || !(100..=60000).contains(&s.serial_idle_timeout_ms))
        {
            return Err("serial-test: укажите 1 порт (1/2 канала) или 2 порта (по каналу), baud_rate > 0, timeout 100..60000 мс".into());
        }
        if s.ports.len() == 2 && s.ports[0] == s.ports[1] {
            return Err("Порты должны различаться".into());
        }
        if s.mode == "replay" && s.replay_path.is_empty() {
            return Err("replay_path не задан".into());
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum Command {
    ToggleRecording,
    StartRecording(RecordingRequest),
    Marker(String),
    Disconnect,
}
#[derive(Debug, Clone)]
pub struct RecordingRequest {
    pub session: SessionConfig,
    pub directory: String,
    pub create_directory: bool,
}
#[derive(Debug, Clone)]
pub struct RecordingSummary {
    pub duration_ns: u64,
    pub samples: u64,
    pub lost: u64,
    pub errors: u64,
    pub path: String,
}
#[derive(Debug, Clone)]
pub struct RuntimeStatus {
    pub connection: String,
    pub received_samples: u64,
    pub processed_blocks: u64,
    pub lost_samples: u64,
    pub errors: u64,
    pub outliers: u64,
    pub rate_hz: f64,
    pub recording: Option<String>,
    pub recording_active: bool,
    pub recording_samples: u64,
    pub recording_started_ns: Option<u64>,
    pub recording_summary: Option<RecordingSummary>,
    pub now_ns: u64,
    pub events: RingBuffer<Event>,
    pub finished: bool,
    pub fatal_error: Option<String>,
    pub consumer_blocks: u64,
    pub consumer_dropped: u64,
}
impl Default for RuntimeStatus {
    fn default() -> Self {
        Self {
            connection: "Connecting".into(),
            received_samples: 0,
            processed_blocks: 0,
            lost_samples: 0,
            errors: 0,
            outliers: 0,
            rate_hz: 0.,
            recording: None,
            recording_active: false,
            recording_samples: 0,
            recording_started_ns: None,
            recording_summary: None,
            now_ns: 0,
            events: RingBuffer::new(30),
            finished: false,
            fatal_error: None,
            consumer_blocks: 0,
            consumer_dropped: 0,
        }
    }
}
