//! Sources deliver blocks; protocol framing is separate from serial I/O.
use core_types::*;
use std::{path::Path, time::Instant};
pub mod decoder;
mod serial;
mod tagged_probe;
pub use serial::{describe_ports, probe, ProbeOptions, ProbeSummary, SerialSource};
use storage::{EventReader, SessionReader};
pub use tagged_probe::{inspect_tagged_capture, CounterCandidate, TaggedCandidate};

pub enum SourcePoll {
    Block(RawSignalBlock),
    Event(Event),
    Pending,
    End,
}
/// Poll must return within a bounded time so cancellation can be observed.
pub trait SignalSource: Send {
    fn poll(&mut self) -> Result<SourcePoll>;
    /// Drain already decoded data on cancellation, without reading more input.
    fn finish(&mut self) -> Vec<SourcePoll> {
        Vec::new()
    }
}
pub fn open(config: &SourceConfig) -> Result<Box<dyn SignalSource>> {
    match config.mode.as_str() {
        "synthetic"=>Ok(Box::new(Synthetic::new(config.clone()))),
        "replay"=>Ok(Box::new(Replay::open(Path::new(&config.replay_path),config.replay_speed)?)),
        "serial" | "serial-test"=>Ok(Box::new(SerialSource::open(config)?)),
        "bitronics"=>Err("Use source.mode=serial with serial_protocol=csv or tagged-u8 for your Arduino firmware".into()),
        other=>Err(format!("Неизвестный источник {other}")),
    }
}
pub fn list_ports() -> Result<Vec<String>> {
    serialport::available_ports()
        .map(|ports| ports.into_iter().map(|p| p.port_name).collect())
        .map_err(|e| format!("serial enumeration: {e}"))
}

pub struct Synthetic {
    config: SourceConfig,
    random: u64,
    next_sample: u64,
    next_block: u64,
    origin: Instant,
    ended: bool,
}
impl Synthetic {
    pub fn new(config: SourceConfig) -> Self {
        Self {
            random: config.seed,
            next_sample: 0,
            next_block: 0,
            origin: Instant::now(),
            ended: false,
            config,
        }
    }
    fn noise(&mut self) -> f64 {
        // SplitMix64: deterministic including seed=0, no extra random dependency.
        self.random = self.random.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = self.random;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        z ^= z >> 31;
        (z >> 11) as f64 / ((1u64 << 53) as f64) * 2. - 1.
    }
    pub fn generate(&mut self) -> Option<RawSignalBlock> {
        if self
            .config
            .anomalies
            .disconnect_after_samples
            .is_some_and(|end| self.next_sample >= end)
        {
            return None;
        }
        let start = self.next_sample;
        let fs = self.config.sample_rate_hz;
        let mut channels: Vec<ChannelSamples> = (0..self.config.channels)
            .map(|i| ChannelSamples {
                id: format!("ch{}", i + 1),
                device: format!("synthetic-{}", i + 1),
                samples: Vec::new(),
            })
            .collect();
        for _ in 0..self.config.block_size {
            if self
                .config
                .anomalies
                .disconnect_after_samples
                .is_some_and(|end| self.next_sample >= end)
            {
                break;
            }
            let seq = self.next_sample;
            self.next_sample += 1;
            if self.config.anomalies.skip_every > 0
                && seq > 0
                && seq.is_multiple_of(self.config.anomalies.skip_every)
            {
                continue;
            }
            for (i, ch) in channels.iter_mut().enumerate() {
                let delay = if i == 1 {
                    self.config.anomalies.channel2_delay_ms / 1000.
                } else {
                    0.
                };
                let t = seq as f64 / fs;
                let phase = t - delay;
                let mut value = self
                    .config
                    .components
                    .iter()
                    .map(|c| c.amplitude * (std::f64::consts::TAU * c.hz * phase).sin())
                    .sum::<f64>()
                    * (1. - i as f64 * 0.15);
                value += self.config.noise * self.noise()
                    + self.config.mains * (std::f64::consts::TAU * 50. * phase).sin()
                    + self.config.drift * (std::f64::consts::TAU * 0.2 * phase).sin();
                if self.config.anomalies.spike_every > 0
                    && seq > 0
                    && seq.is_multiple_of(self.config.anomalies.spike_every)
                {
                    value += self.config.anomalies.spike_amplitude;
                }
                ch.samples.push(Sample {
                    sequence: seq,
                    timestamp_ns: ((t + delay) * 1e9).round() as u64,
                    value,
                    flags: Vec::new(),
                });
            }
        }
        let block = RawSignalBlock {
            sequence: self.next_block,
            started_at: (start as f64 / fs * 1e9).round() as u64,
            sample_rate_hz: fs,
            source: "synthetic".into(),
            synchronized_clock: true,
            channels,
            flags: Vec::new(),
        };
        self.next_block += 1;
        Some(block)
    }
}
impl SignalSource for Synthetic {
    fn poll(&mut self) -> Result<SourcePoll> {
        if self.ended {
            return Ok(SourcePoll::End);
        }
        let due = self.next_sample as f64 / self.config.sample_rate_hz;
        if self.origin.elapsed().as_secs_f64() < due {
            return Ok(SourcePoll::Pending);
        }
        match self.generate() {
            Some(b) => Ok(SourcePoll::Block(b)),
            None => {
                self.ended = true;
                Ok(SourcePoll::Event(Event {
                    timestamp_ns: (due * 1e9) as u64,
                    kind: "Disconnected".into(),
                    text: "Синтетический разрыв соединения".into(),
                }))
            }
        }
    }
}
pub struct Replay {
    reader: SessionReader,
    events: EventReader,
    next: Option<RawSignalBlock>,
    next_event: Option<Event>,
    load_block: bool,
    load_event: bool,
    origin: Instant,
    first_timestamp: u64,
    last_timestamp: Option<u64>,
    speed: f64,
}
impl Replay {
    pub fn open(path: &Path, speed: f64) -> Result<Self> {
        if !speed.is_finite() || !(0.01..=100.).contains(&speed) {
            return Err("replay speed: 0.01..100".into());
        }
        let mut reader = SessionReader::open(path)?;
        let mut events = EventReader::open(path)?;
        let next = reader.next_block()?;
        let next_event = events.next_event()?;
        let first_timestamp = next
            .as_ref()
            .map(|b| b.started_at)
            .into_iter()
            .chain(next_event.as_ref().map(|e| e.timestamp_ns))
            .min()
            .unwrap_or(0);
        Ok(Self {
            reader,
            events,
            next,
            next_event,
            load_block: false,
            load_event: false,
            origin: Instant::now(),
            first_timestamp,
            last_timestamp: None,
            speed,
        })
    }
}
impl SignalSource for Replay {
    fn poll(&mut self) -> Result<SourcePoll> {
        // Load on the following poll: a bad next record cannot discard an already decoded block.
        if self.load_block {
            self.next = self.reader.next_block()?;
            self.load_block = false;
        }
        if self.load_event {
            self.next_event = self.events.next_event()?;
            self.load_event = false;
        }
        let block_time = self.next.as_ref().map(|b| {
            b.channels
                .iter()
                .filter_map(|c| c.samples.last().map(|s| s.timestamp_ns))
                .max()
                .unwrap_or(b.started_at)
        });
        let event_time = self.next_event.as_ref().map(|e| e.timestamp_ns);
        let Some(time) = block_time.into_iter().chain(event_time).min() else {
            return Ok(SourcePoll::End);
        };
        if self.last_timestamp.is_some_and(|last| time < last) {
            return Err("replay: обратный ход времени".into());
        }
        let due = time.saturating_sub(self.first_timestamp) as f64 / 1e9 / self.speed;
        if self.origin.elapsed().as_secs_f64() < due {
            return Ok(SourcePoll::Pending);
        }
        self.last_timestamp = Some(time);
        if block_time == Some(time) {
            self.load_block = true;
            Ok(SourcePoll::Block(
                self.next.take().ok_or("replay: missing block")?,
            ))
        } else {
            self.load_event = true;
            let mut event = self.next_event.take().ok_or("replay: missing event")?;
            event.kind = format!("Replay/{}", event.kind);
            Ok(SourcePoll::Event(event))
        }
    }
}
/// Backwards-compatible name for the Serial CSV numeric parser.
pub use decoder::parse_csv_line as parse_test_line;
