//! Sources deliver blocks; protocol framing is separate from serial I/O.
use core_types::*;
use std::{
    collections::VecDeque,
    io::Read,
    path::Path,
    time::{Duration, Instant},
};
use storage::{EventReader, SessionReader};

pub enum SourcePoll {
    Block(RawSignalBlock),
    Event(Event),
    Pending,
    End,
}
/// Poll must return within a bounded time so cancellation can be observed.
pub trait SignalSource: Send {
    fn poll(&mut self) -> Result<SourcePoll>;
}
pub fn open(config: &SourceConfig) -> Result<Box<dyn SignalSource>> {
    match config.mode.as_str() {
        "synthetic"=>Ok(Box::new(Synthetic::new(config.clone()))),
        "replay"=>Ok(Box::new(Replay::open(Path::new(&config.replay_path),config.replay_speed)?)),
        "serial-test"=>Ok(Box::new(SerialSource::open(config)?)),
        "bitronics"=>Err("Bitronics: протокол не подтверждён. Для адаптера требуется штатный Arduino-скетч; serial-test — другой, тестовый протокол".into()),
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
/// TEST ONLY. One finite numeric value per channel, comma-separated, newline-delimited.
/// No hardware sequence counters: serial-test can only flag malformed lines and timeouts.
pub fn parse_test_line(line: &str, channels: usize) -> Result<Vec<f64>> {
    if !(1..=2).contains(&channels) {
        return Err("test protocol: channels must be 1 or 2".into());
    }
    if line.len() > 256 {
        return Err("test protocol: строка длиннее 256 байт".into());
    }
    let fields: Vec<_> = line.trim().split(',').collect();
    if fields.len() != channels {
        return Err(format!(
            "test protocol: ожидалось {channels} значений, получено {}",
            fields.len()
        ));
    }
    fields
        .iter()
        .map(|s| {
            s.trim()
                .parse::<f64>()
                .ok()
                .filter(|v| v.is_finite())
                .ok_or_else(|| "test protocol: некорректное или неконечное число".into())
        })
        .collect()
}
struct PortInput {
    port: Box<dyn serialport::SerialPort>,
    name: String,
    bytes: Vec<u8>,
    incoming: VecDeque<u8>,
    overflow: bool,
    sequence: u64,
    last_valid: Instant,
}
pub struct SerialSource {
    ports: Vec<PortInput>,
    config: SourceConfig,
    origin: Instant,
    block_sequence: u64,
    turn: usize,
    pending_flags: Vec<SignalFlag>,
}
impl SerialSource {
    pub fn open(config: &SourceConfig) -> Result<Self> {
        let mut ports = Vec::new();
        for name in &config.ports {
            let port = serialport::new(name, config.baud_rate)
                .timeout(Duration::from_millis(1))
                .open()
                .map_err(|e| format!("serial {name}: {e}"))?;
            ports.push(PortInput {
                port,
                name: name.clone(),
                bytes: Vec::with_capacity(256),
                incoming: VecDeque::new(),
                overflow: false,
                sequence: 0,
                last_valid: Instant::now(),
            });
        }
        if ports.is_empty() {
            return Err("serial: порты не заданы".into());
        }
        Ok(Self {
            ports,
            config: config.clone(),
            origin: Instant::now(),
            block_sequence: 0,
            turn: 0,
            pending_flags: Vec::new(),
        })
    }
}
impl SignalSource for SerialSource {
    fn poll(&mut self) -> Result<SourcePoll> {
        let index = self.turn % self.ports.len();
        self.turn += 1;
        let dual = self.ports.len() == 2;
        let input = &mut self.ports[index];
        if input.last_valid.elapsed() > Duration::from_millis(self.config.serial_idle_timeout_ms) {
            return Err(format!(
                "serial {}: нет корректных пакетов {} мс; соединение остановлено",
                input.name, self.config.serial_idle_timeout_ms
            ));
        }
        if input.incoming.is_empty() {
            let mut bytes = [0u8; 256];
            match input.port.read(&mut bytes) {
                Ok(0) => return Err(format!("serial {}: EOF", input.name)),
                Ok(n) => input.incoming.extend(&bytes[..n]),
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                    ) =>
                {
                    return Ok(SourcePoll::Pending)
                }
                Err(e) => return Err(format!("serial {}: {e}", input.name)),
            }
        }
        let mut complete = false;
        while let Some(byte) = input.incoming.pop_front() {
            if byte == b'\n' {
                complete = true;
                break;
            }
            if input.bytes.len() < 256 && !input.overflow {
                input.bytes.push(byte);
            } else {
                input.overflow = true;
                input.bytes.clear();
            }
        }
        if !complete {
            return Ok(SourcePoll::Pending);
        }
        let seq = input.sequence;
        input.sequence += 1;
        let result = if input.overflow {
            Err("test protocol: строка длиннее 256 байт".into())
        } else {
            std::str::from_utf8(&input.bytes)
                .map_err(|_| "test protocol: invalid UTF-8".into())
                .and_then(|line| parse_test_line(line, if dual { 1 } else { self.config.channels }))
        };
        input.bytes.clear();
        input.overflow = false;
        let timestamp_ns = self.origin.elapsed().as_nanos().min(u64::MAX as u128) as u64;
        match result {
            Err(e) => {
                self.pending_flags = vec![SignalFlag::InvalidPacket];
                Ok(SourcePoll::Event(Event {
                    timestamp_ns,
                    kind: "InvalidPacket".into(),
                    text: format!("{}: {e}", input.name),
                }))
            }
            Ok(values) => {
                input.last_valid = Instant::now();
                let channels = (0..self.config.channels)
                    .map(|ch| {
                        let value = if dual {
                            if ch == index {
                                values.first().copied()
                            } else {
                                None
                            }
                        } else {
                            values.get(ch).copied()
                        };
                        let device = if dual {
                            self.config.ports[ch].clone()
                        } else {
                            self.config.ports[0].clone()
                        };
                        ChannelSamples {
                            id: format!("ch{}", ch + 1),
                            device,
                            samples: value
                                .into_iter()
                                .map(|value| Sample {
                                    sequence: seq,
                                    timestamp_ns,
                                    value,
                                    flags: Vec::new(),
                                })
                                .collect(),
                        }
                    })
                    .collect();
                let block = RawSignalBlock {
                    sequence: self.block_sequence,
                    started_at: timestamp_ns,
                    sample_rate_hz: self.config.sample_rate_hz,
                    source: "serial-test (host timestamps)".into(),
                    synchronized_clock: false,
                    channels,
                    flags: std::mem::take(&mut self.pending_flags),
                };
                self.block_sequence += 1;
                Ok(SourcePoll::Block(block))
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use serialport::SerialPort;
    use std::io::Write;

    fn pair_source(channels: usize, ports: usize) -> (SerialSource, Vec<serialport::TTYPort>) {
        let mut config = SourceConfig {
            mode: "serial-test".into(),
            channels,
            serial_idle_timeout_ms: 100,
            ..SourceConfig::default()
        };
        let mut inputs = Vec::new();
        let mut masters = Vec::new();
        for i in 0..ports {
            let (master, mut slave) = serialport::TTYPort::pair().expect("PTY pair");
            slave
                .set_timeout(Duration::from_millis(1))
                .expect("timeout");
            let name = format!("test-port-{i}");
            config.ports.push(name.clone());
            inputs.push(PortInput {
                port: Box::new(slave),
                name,
                bytes: Vec::new(),
                incoming: VecDeque::new(),
                overflow: false,
                sequence: 0,
                last_valid: Instant::now(),
            });
            masters.push(master);
        }
        (
            SerialSource {
                ports: inputs,
                config,
                origin: Instant::now(),
                block_sequence: 0,
                turn: 0,
                pending_flags: Vec::new(),
            },
            masters,
        )
    }
    #[test]
    fn serial_transport_recovers_after_bad_and_oversized_lines_then_disconnects() {
        let (mut source, mut masters) = pair_source(2, 1);
        masters[0].write_all(b"1,2\nbad\n").expect("write");
        masters[0].write_all(&vec![b'x'; 300]).expect("oversize");
        masters[0].write_all(b"\n3,4\n").expect("valid after bad");
        let mut blocks = Vec::new();
        let mut errors = 0;
        let start = Instant::now();
        while blocks.len() < 2 {
            match source.poll().expect("poll") {
                SourcePoll::Block(b) => blocks.push(b),
                SourcePoll::Event(e) => {
                    assert_eq!(e.kind, "InvalidPacket");
                    errors += 1;
                }
                _ => {}
            }
            assert!(start.elapsed() < Duration::from_secs(1));
        }
        assert_eq!(errors, 2);
        assert_eq!(blocks[1].channels[1].samples[0].value, 4.);
        assert_eq!(blocks[1].channels[0].samples[0].sequence, 3);
        drop(masters);
        let start = Instant::now();
        while source.poll().is_ok() {
            assert!(start.elapsed() < Duration::from_secs(1));
        }
    }
    #[test]
    fn separate_serial_ports_do_not_claim_a_shared_clock() {
        let (mut source, mut masters) = pair_source(2, 2);
        masters[0].write_all(b"12\n").expect("first");
        masters[1].write_all(b"34\n").expect("second");
        let mut blocks = Vec::new();
        let start = Instant::now();
        while blocks.len() < 2 {
            if let SourcePoll::Block(b) = source.poll().expect("poll") {
                blocks.push(b);
            }
            assert!(start.elapsed() < Duration::from_secs(1));
        }
        assert!(!blocks[0].synchronized_clock);
        assert_eq!(blocks[0].channels[0].samples[0].value, 12.);
        assert!(blocks[0].channels[1].samples.is_empty());
        assert_eq!(blocks[1].channels[1].samples[0].value, 34.);
    }
}
