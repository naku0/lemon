use super::{
    decoder::{decoder, DecodeResult, SerialDecoder},
    SignalSource, SourcePoll,
};
use core_types::*;
use std::{
    collections::VecDeque,
    fs::OpenOptions,
    io::{Read, Write},
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

fn open_port(name: &str, baud: u32) -> Result<Box<dyn serialport::SerialPort>> {
    serialport::new(name, baud)
        .data_bits(serialport::DataBits::Eight).parity(serialport::Parity::None)
        .stop_bits(serialport::StopBits::One).flow_control(serialport::FlowControl::None)
        .timeout(Duration::from_millis(10)).open()
        .map_err(|e| {
            let reason = match e.kind() {
                serialport::ErrorKind::NoDevice => "port not found or disconnected",
                serialport::ErrorKind::InvalidInput => "unsupported baud rate or port settings",
                serialport::ErrorKind::Io(std::io::ErrorKind::PermissionDenied) => "access denied; check permissions or another application holding the port",
                serialport::ErrorKind::Io(std::io::ErrorKind::NotFound) => "port not found",
                _ => "cannot open/configure port; it may be busy or not support the requested baud rate",
            };
            format!("serial/open {name} at {baud} baud (8N1): {reason}: {e}. Close Arduino Serial Monitor and other port users.")
        })
}

struct PortInput {
    port: Box<dyn serialport::SerialPort>,
    decoder: Box<dyn SerialDecoder>,
    pending: VecDeque<DecodeResult>,
    samples: Vec<Vec<Sample>>,
    sequence: u64,
    last_valid: Instant,
    received: bool,
    invalid: u64,
    diagnostic: String,
    last_diagnostic: Instant,
    flag_next: bool,
    lost: u64,
    duplicate_counters: u64,
    startup_complete: bool,
    startup_remaining: Option<usize>,
    startup_bytes: u64,
}
pub struct SerialSource {
    ports: Vec<PortInput>,
    config: SourceConfig,
    origin: Instant,
    block_sequence: u64,
    turn: usize,
    announced: bool,
    stopping: bool,
}
impl SerialSource {
    pub fn open(config: &SourceConfig) -> Result<Self> {
        if config.ports.is_empty()
            || config.ports.len() > 2
            || config.ports.len() > config.channels
            || !(1..=2).contains(&config.channels)
            || !(1..=4096).contains(&config.block_size)
            || config.serial_startup_delay_ms > 60000
            || !(100..=60000).contains(&config.serial_idle_timeout_ms)
            || !config.sample_rate_hz.is_finite()
            || config.sample_rate_hz <= 0.
        {
            return Err(
                "serial: ports, positive block_size and nominal sample_rate_hz are required".into(),
            );
        }
        if config.serial_protocol == "tagged-u8" {
            config.validate_serial_tags()?;
            if config.ports.len() != 1 {
                return Err("tagged-u8 requires one port".into());
            }
        }
        let mut ports = Vec::new();
        for name in &config.ports {
            let decoder = decoder(
                config,
                if config.ports.len() == 2 {
                    1
                } else {
                    config.channels
                },
            )?;
            ports.push(PortInput {
                port: open_port(name, config.baud_rate)?,
                decoder,
                pending: VecDeque::new(),
                samples: vec![Vec::new(); config.channels],
                sequence: 0,
                last_valid: Instant::now(),
                received: false,
                invalid: 0,
                diagnostic: String::new(),
                last_diagnostic: Instant::now(),
                flag_next: false,
                lost: 0,
                duplicate_counters: 0,
                startup_complete: config.serial_startup_delay_ms == 0,
                startup_remaining: None,
                startup_bytes: 0,
            });
        }
        Ok(Self {
            ports,
            config: config.clone(),
            origin: Instant::now(),
            block_sequence: 0,
            turn: 0,
            announced: false,
            stopping: false,
        })
    }
    fn block(&mut self, index: usize) -> SourcePoll {
        let dual = self.ports.len() == 2;
        let input = &mut self.ports[index];
        let channels: Vec<_> = (0..self.config.channels)
            .map(|ch| ChannelSamples {
                id: if self.config.serial_protocol == "tagged-u8" {
                    self.config.serial_channel_tags[ch].clone()
                } else {
                    format!("ch{}", ch + 1)
                },
                device: self.config.ports[if dual { ch } else { 0 }].clone(),
                samples: std::mem::take(&mut input.samples[ch]),
            })
            .collect();
        let started_at = channels
            .iter()
            .flat_map(|c| c.samples.first())
            .map(|s| s.timestamp_ns)
            .min()
            .unwrap_or(0);
        let block = RawSignalBlock {
            sequence: self.block_sequence,
            started_at,
            sample_rate_hz: self.config.sample_rate_hz,
            source: format!(
                "serial/{} (reconstructed host-side timestamps; nominal rate)",
                self.config.serial_protocol
            ),
            synchronized_clock: false,
            channels,
            flags: Vec::new(),
        };
        self.block_sequence += 1;
        SourcePoll::Block(block)
    }
}
impl SignalSource for SerialSource {
    fn finish(&mut self) -> Vec<SourcePoll> {
        self.stopping = true;
        let mut result = Vec::new();
        while self.ports.iter().any(|p| {
            !p.pending.is_empty()
                || p.invalid > 0
                || p.lost > 0
                || p.duplicate_counters > 0
                || p.samples.iter().any(|s| !s.is_empty())
        }) {
            match self.poll() {
                Ok(SourcePoll::Pending | SourcePoll::End) => {}
                Ok(item) => result.push(item),
                Err(_) => break,
            }
        }
        result
    }
    fn poll(&mut self) -> Result<SourcePoll> {
        if !self.announced {
            self.announced = true;
            return Ok(SourcePoll::Event(Event { timestamp_ns: 0, kind: "SerialTiming".into(), text: format!(
                "Protocol {}; nominal {} Hz; reconstructed host-side timestamps from decoded frame index plus inferred missing frames; synchronized_clock=false. No device timestamps or checksum. Counter: {:?}; with a counter losses are modulo 256 (resets/reordering and multiples of 256 are ambiguous); without a counter whole-frame loss cannot be counted. Channels are sampled sequentially; offset unknown.", self.config.serial_protocol, self.config.sample_rate_hz, self.config.serial_sequence_tag) }));
        }
        let index = self.turn % self.ports.len();
        self.turn = (self.turn + 1) % self.ports.len();
        let dual = self.ports.len() == 2;
        let input = &mut self.ports[index];
        if !self.stopping && !input.startup_complete {
            let grace_ended =
                self.origin.elapsed() >= Duration::from_millis(self.config.serial_startup_delay_ms);
            let mut capacity = 256;
            if grace_ended {
                if input.startup_remaining.is_none() {
                    input.startup_remaining = Some(
                        input
                            .port
                            .bytes_to_read()
                            .map_err(|e| format!("serial/startup queue: {e}"))?
                            as usize,
                    );
                }
                capacity = capacity.min(input.startup_remaining.unwrap_or(0));
                if capacity == 0 {
                    input.startup_complete = true;
                    input.last_valid = Instant::now();
                    input.decoder.reset();
                    return Ok(SourcePoll::Event(Event { timestamp_ns: 0, kind: "SerialStartup".into(), text: format!("{}: reset grace {} ms completed; {} startup bytes excluded before framing/counter tracking",self.config.ports[index],self.config.serial_startup_delay_ms,input.startup_bytes) }));
                }
            }
            let mut bytes = [0u8; 256];
            match input.port.read(&mut bytes[..capacity]) {
                Ok(n) => {
                    input.startup_bytes += n as u64;
                    if let Some(remaining) = input.startup_remaining.as_mut() {
                        *remaining = remaining.saturating_sub(n);
                    }
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                    ) =>
                {
                    if grace_ended {
                        input.startup_remaining = Some(0);
                    }
                }
                Err(e) => {
                    return Err(format!(
                        "serial/startup read {}: {e}",
                        self.config.ports[index]
                    ))
                }
            }
            return Ok(SourcePoll::Pending);
        }
        let timeout = input.last_valid.elapsed()
            > Duration::from_millis(self.config.serial_idle_timeout_ms)
            && self.origin.elapsed()
                > Duration::from_millis(
                    self.config.serial_startup_delay_ms + self.config.serial_idle_timeout_ms,
                );
        if (input.lost > 0 || input.duplicate_counters > 0)
            && (self.stopping
                || timeout
                || input.last_diagnostic.elapsed() >= Duration::from_secs(1))
        {
            let text = format!("counter {:?}: minimum missing frames={} (modulo 256); repeated counter={} (duplicate/reset/multiple-of-256 loss ambiguous)", self.config.serial_sequence_tag, input.lost, input.duplicate_counters);
            let kind = if input.lost > 0 {
                "LostSample"
            } else {
                "SequenceDiscontinuity"
            };
            input.lost = 0;
            input.duplicate_counters = 0;
            input.last_diagnostic = Instant::now();
            return Ok(SourcePoll::Event(Event {
                timestamp_ns: (input.sequence as f64 * 1e9 / self.config.sample_rate_hz).round()
                    as u64,
                kind: kind.into(),
                text,
            }));
        }
        if input.invalid > 0
            && (self.stopping
                || timeout
                || input.last_diagnostic.elapsed() >= Duration::from_secs(1))
        {
            let text = format!(
                "{}: {} decoding errors since last diagnostic; {}",
                self.config.ports[index], input.invalid, input.diagnostic
            );
            input.invalid = 0;
            input.last_diagnostic = Instant::now();
            return Ok(SourcePoll::Event(Event {
                timestamp_ns: (input.sequence as f64 * 1e9 / self.config.sample_rate_hz).round()
                    as u64,
                kind: "InvalidPacket".into(),
                text,
            }));
        }
        if input.pending.is_empty() {
            if self.stopping {
                return Ok(if input.samples.iter().any(|s| !s.is_empty()) {
                    self.block(index)
                } else {
                    SourcePoll::End
                });
            }
            if timeout {
                if input.samples.iter().any(|s| !s.is_empty()) {
                    return Ok(self.block(index));
                }
                return Err(format!(
                    "serial {}: {} for {} ms; check --protocol, wiring and nominal rate",
                    self.config.ports[index],
                    if input.received {
                        "bytes received but no valid frames"
                    } else {
                        "no incoming data"
                    },
                    self.config.serial_idle_timeout_ms
                ));
            }
            let mut bytes = [0u8; 256];
            match input.port.read(&mut bytes) {
                Ok(0) => {
                    if input.samples.iter().any(|s| !s.is_empty()) {
                        return Ok(self.block(index));
                    }
                    return Err(format!(
                        "serial {}: disconnected (EOF)",
                        self.config.ports[index]
                    ));
                }
                Ok(n) => {
                    input.received = true;
                    input.pending.extend(input.decoder.push(&bytes[..n]));
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                    ) =>
                {
                    return Ok(SourcePoll::Pending)
                }
                Err(e) => {
                    if input.samples.iter().any(|s| !s.is_empty()) {
                        return Ok(self.block(index));
                    }
                    return Err(format!("serial/read {}: {e}", self.config.ports[index]));
                }
            }
        }
        while let Some(result) = input.pending.pop_front() {
            match result {
                Err(e) => {
                    input.invalid += 1;
                    input.diagnostic = e;
                    input.flag_next = true;
                    // CSV line numbers retain legacy gap semantics; tagged frames have no loss counter.
                    if self.config.serial_protocol == "csv" {
                        input.sequence += 1;
                    }
                }
                Ok(frame) => {
                    input.last_valid = Instant::now();
                    input.sequence += frame.missing_frames;
                    input.lost += frame.missing_frames;
                    input.duplicate_counters += u64::from(frame.duplicate_counter);
                    let timestamp_ns =
                        (input.sequence as f64 * 1e9 / self.config.sample_rate_hz).round() as u64;
                    for (ch, value) in frame.channels.into_iter().enumerate() {
                        input.samples[if dual { index } else { ch }].push(Sample {
                            sequence: input.sequence,
                            timestamp_ns,
                            value,
                            flags: {
                                let mut flags = frame.flags.clone();
                                if input.flag_next && !flags.contains(&SignalFlag::InvalidPacket) {
                                    flags.push(SignalFlag::InvalidPacket);
                                }
                                flags
                            },
                        });
                    }
                    input.flag_next = false;
                    input.sequence += 1;
                    if input
                        .samples
                        .iter()
                        .any(|s| s.len() >= self.config.block_size)
                    {
                        return Ok(self.block(index));
                    }
                }
            }
        }
        Ok(SourcePoll::Pending)
    }
}

pub fn describe_ports() -> Result<Vec<String>> {
    serialport::available_ports()
        .map_err(|e| format!("serial enumeration: {e}"))
        .map(|ports| {
            ports
                .into_iter()
                .map(|p| match p.port_type {
                    serialport::SerialPortType::UsbPort(u) => format!(
                        "{} | USB {:04x}:{:04x} | serial={} | manufacturer={} | product={}",
                        p.port_name,
                        u.vid,
                        u.pid,
                        u.serial_number.as_deref().unwrap_or("unknown"),
                        u.manufacturer.as_deref().unwrap_or("unknown"),
                        u.product.as_deref().unwrap_or("unknown")
                    ),
                    other => format!("{} | {:?}", p.port_name, other),
                })
                .collect()
        })
}
pub struct ProbeOptions {
    pub port: String,
    pub baud: u32,
    pub duration: Duration,
    pub startup_delay: Duration,
    pub output: PathBuf,
    pub overwrite: bool,
    pub max_bytes: u64,
}
#[derive(Debug)]
pub struct ProbeSummary {
    pub bytes: u64,
    pub seconds: f64,
    pub frames: u64,
    pub newlines: u64,
    pub ascii_preview: String,
    pub hex_preview: String,
    pub limit_reached: bool,
    pub startup_seconds: f64,
    pub startup_bytes: u64,
    pub candidate: Option<super::TaggedCandidate>,
}
impl ProbeSummary {
    pub fn suggests_tagged_u8(&self) -> bool {
        self.candidate.as_ref().is_some_and(|c| {
            c.complete_frames >= 10
                && c.complete_frames * c.frame_size as u64 * 100 >= self.bytes * 90
        })
    }
}
pub fn probe(options: &ProbeOptions, cancelled: &AtomicBool) -> Result<ProbeSummary> {
    if options.duration.is_zero()
        || options.duration > Duration::from_secs(3600)
        || options.max_bytes == 0
        || options.max_bytes > 1_073_741_824
        || options.startup_delay > Duration::from_secs(60)
    {
        return Err(
            "probe: positive duration <=3600 s, startup delay 0..60 s and max bytes 1..1073741824 required"
                .into(),
        );
    }
    validate_probe_output(options)?;
    let port = open_port(&options.port, options.baud)?;
    capture(options, cancelled, port)
}
fn validate_probe_output(options: &ProbeOptions) -> Result<()> {
    match std::fs::symlink_metadata(&options.output) {
        Ok(m) if !m.is_file() || m.file_type().is_symlink() => Err(format!(
            "probe/output {}: must be a regular file, not a symlink or device",
            options.output.display()
        )),
        Ok(_) if !options.overwrite => Err(format!(
            "probe/output {}: file exists; pass --overwrite to replace it",
            options.output.display()
        )),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("probe/output {}: {e}", options.output.display())),
    }
}
fn capture(
    options: &ProbeOptions,
    cancelled: &AtomicBool,
    mut port: Box<dyn serialport::SerialPort>,
) -> Result<ProbeSummary> {
    validate_probe_output(options)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(!options.overwrite)
        .create(options.overwrite)
        .truncate(options.overwrite)
        .open(&options.output)
        .map_err(|e| {
            format!(
                "probe/output {}: {e}; use --overwrite only to replace this file",
                options.output.display()
            )
        })?;
    let startup = Instant::now();
    let mut startup_bytes = 0u64;
    // Drain reset/startup bytes by reading only; no writes, flush commands or device reconfiguration.
    while startup.elapsed() < options.startup_delay && !cancelled.load(Ordering::Relaxed) {
        let mut bytes = [0u8; 4096];
        match port.read(&mut bytes) {
            Ok(n) => startup_bytes += n as u64,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) => {}
            Err(e) => return Err(format!("probe/startup read: {e}")),
        }
    }
    if !options.startup_delay.is_zero() && !cancelled.load(Ordering::Relaxed) {
        let mut remaining = port
            .bytes_to_read()
            .map_err(|e| format!("probe/startup queue: {e}"))? as usize;
        while remaining > 0 {
            let mut bytes = [0u8; 4096];
            let len = remaining.min(bytes.len());
            let n = port
                .read(&mut bytes[..len])
                .map_err(|e| format!("probe/startup drain: {e}"))?;
            if n == 0 {
                break;
            }
            remaining -= n;
            startup_bytes += n as u64;
        }
    }
    let startup_seconds = startup.elapsed().as_secs_f64();
    let start = Instant::now();
    let mut summary = ProbeSummary {
        bytes: 0,
        seconds: 0.,
        frames: 0,
        newlines: 0,
        ascii_preview: String::new(),
        hex_preview: String::new(),
        limit_reached: false,
        startup_seconds,
        startup_bytes,
        candidate: None,
    };
    let mut preview = Vec::new();
    while start.elapsed() < options.duration && !cancelled.load(Ordering::Relaxed) {
        let mut bytes = [0u8; 4096];
        let capacity = (options.max_bytes - summary.bytes).min(bytes.len() as u64) as usize;
        if capacity == 0 {
            summary.limit_reached = true;
            break;
        }
        match port.read(&mut bytes[..capacity]) {
            Ok(0) => break,
            Ok(n) => {
                file.write_all(&bytes[..n])
                    .map_err(|e| format!("probe/write {}: {e}", options.output.display()))?;
                summary.bytes += n as u64;
                summary.newlines += bytes[..n].iter().filter(|&&b| b == b'\n').count() as u64;
                preview.extend(bytes[..n].iter().take(128 - preview.len()));
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) => {}
            Err(e) => {
                return Err(format!(
                    "probe/read {}: {e}; partial capture saved at {}",
                    options.port,
                    options.output.display()
                ))
            }
        }
    }
    file.flush().map_err(|e| format!("probe/flush: {e}"))?;
    summary.seconds = start.elapsed().as_secs_f64();
    summary.candidate = super::inspect_tagged_capture(&options.output)?;
    summary.frames = summary.candidate.as_ref().map_or(0, |c| c.complete_frames);
    summary.limit_reached |= summary.bytes == options.max_bytes;
    summary.ascii_preview = preview
        .iter()
        .map(|&b: &u8| {
            if b.is_ascii_graphic() || b == b' ' {
                b as char
            } else {
                '.'
            }
        })
        .collect();
    summary.hex_preview = preview
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ");
    Ok(summary)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use serialport::SerialPort;
    fn pair_source(
        protocol: &str,
        count: usize,
        block_size: usize,
    ) -> (SerialSource, Vec<serialport::TTYPort>) {
        let config = SourceConfig {
            mode: "serial".into(),
            channels: 2,
            serial_protocol: protocol.into(),
            ports: (0..count).map(|i| format!("virtual-{i}")).collect(),
            sample_rate_hz: 1000. / 3.,
            block_size,
            serial_idle_timeout_ms: 100,
            serial_startup_delay_ms: 0,
            ..SourceConfig::default()
        };
        let mut ports = Vec::new();
        let mut masters = Vec::new();
        for _ in 0..count {
            let (master, mut slave) = serialport::TTYPort::pair().expect("PTY pair");
            slave
                .set_timeout(Duration::from_millis(1))
                .expect("timeout");
            ports.push(PortInput {
                port: Box::new(slave),
                decoder: decoder(&config, if count == 2 { 1 } else { 2 }).expect("decoder"),
                pending: VecDeque::new(),
                samples: vec![Vec::new(); 2],
                sequence: 0,
                last_valid: Instant::now(),
                received: false,
                invalid: 0,
                diagnostic: String::new(),
                last_diagnostic: Instant::now(),
                flag_next: false,
                lost: 0,
                duplicate_counters: 0,
                startup_complete: true,
                startup_remaining: None,
                startup_bytes: 0,
            });
            masters.push(master);
        }
        (
            SerialSource {
                ports,
                config,
                origin: Instant::now(),
                block_sequence: 0,
                turn: 0,
                announced: false,
                stopping: false,
            },
            masters,
        )
    }
    fn next_block(s: &mut SerialSource) -> RawSignalBlock {
        let start = Instant::now();
        loop {
            if let SourcePoll::Block(b) = s.poll().expect("poll") {
                return b;
            }
            assert!(start.elapsed() < Duration::from_secs(1));
        }
    }
    #[test]
    fn tagged_transport_batches_uniform_timestamps_and_drains_on_stop() {
        let (mut s, mut masters) = pair_source("tagged-u8", 1, 3);
        masters[0]
            .write_all(b"A0\0A1\xffA0AA10A01A1AA0\x07A1\x08")
            .expect("write");
        let b = next_block(&mut s);
        assert_eq!(b.sample_rate_hz, 1000. / 3.);
        assert!(!b.synchronized_clock);
        assert_eq!(
            b.channels.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            ["A0", "A1"]
        );
        assert_eq!(
            b.channels[0]
                .samples
                .iter()
                .map(|s| s.value)
                .collect::<Vec<_>>(),
            [0., 65., 49.]
        );
        assert_eq!(
            b.channels[1]
                .samples
                .iter()
                .map(|s| s.value)
                .collect::<Vec<_>>(),
            [255., 48., 65.]
        );
        assert_eq!(
            b.channels[0]
                .samples
                .iter()
                .map(|s| s.timestamp_ns)
                .collect::<Vec<_>>(),
            [0, 3_000_000, 6_000_000]
        );
        let tail = s.finish();
        let SourcePoll::Block(b) = &tail[0] else {
            panic!("tail block")
        };
        assert_eq!(b.channels[0].samples[0].value, 7.);
        assert_eq!(b.channels[0].samples[0].timestamp_ns, 9_000_000);
        assert_eq!(b.channels[1].samples[0].sequence, 3);
        assert!(s.finish().is_empty());
        let metadata = storage::Metadata::new(Config {
            source: s.config.clone(),
            ..Config::default()
        });
        assert_eq!(metadata.units.as_deref(), Some("adc_counts_u8"));
        assert!(metadata.timestamp_basis.contains("nominal"));
        assert_eq!(metadata.channel_ids, ["A0", "A1"]);
    }
    #[test]
    fn selected_a0_counter_gaps_reach_raw_timestamps_and_events() {
        let (mut s, mut masters) = pair_source("tagged-u8", 1, 3);
        s.config.channels = 1;
        s.config.serial_channel_tags = vec!["A0".into()];
        s.config.serial_frame_tags = b"012345679"
            .iter()
            .map(|t| format!("A{}", *t as char))
            .collect();
        s.config.serial_sequence_tag = Some("A9".into());
        s.ports[0].decoder = decoder(&s.config, 1).expect("decoder");
        s.ports[0].samples = vec![Vec::new()];
        let bytes: Vec<u8> = [254, 255, 0, 3]
            .iter()
            .flat_map(|seq| {
                b"012345679"
                    .iter()
                    .flat_map(move |tag| [b'A', *tag, if *tag == b'9' { *seq } else { 123 }])
            })
            .collect();
        masters[0].write_all(&bytes).expect("write");
        let block = next_block(&mut s);
        assert_eq!(block.channels.len(), 1);
        assert_eq!(block.channels[0].id, "A0");
        assert_eq!(
            block.channels[0]
                .samples
                .iter()
                .map(|s| s.sequence)
                .collect::<Vec<_>>(),
            [0, 1, 2]
        );
        let tail = s.finish();
        let block = tail
            .iter()
            .find_map(|p| {
                if let SourcePoll::Block(b) = p {
                    Some(b)
                } else {
                    None
                }
            })
            .expect("tail");
        let sample = &block.channels[0].samples[0];
        assert_eq!(
            (sample.sequence, sample.timestamp_ns, sample.value),
            (5, 15_000_000, 123.)
        );
        assert!(sample.flags.contains(&SignalFlag::LostSample));
        assert!(tail.iter().any(|p| matches!(p,SourcePoll::Event(e) if e.kind=="LostSample" && e.text.contains("missing frames=2"))));
    }
    #[test]
    fn live_startup_discards_stale_counter_before_reset() {
        let (mut s, mut masters) = pair_source("tagged-u8", 1, 2);
        s.config.serial_frame_tags.push("A9".into());
        s.config.serial_sequence_tag = Some("A9".into());
        s.ports[0].decoder = decoder(&s.config, 2).expect("counter decoder");
        s.config.serial_startup_delay_ms = 20;
        s.ports[0].startup_complete = false;
        masters[0]
            .write_all(b"A0\x64A1\x65A9\x99")
            .expect("stale bytes");
        let start = Instant::now();
        loop {
            if let SourcePoll::Event(e) = s.poll().expect("startup") {
                if e.kind == "SerialStartup" {
                    assert!(e.text.contains("9 startup bytes"));
                    break;
                }
            }
            assert!(start.elapsed() < Duration::from_secs(1));
        }
        masters[0]
            .write_all(b"A0\x01A1\x02A9\x00A0\x03A1\x04A9\x01")
            .expect("post reset");
        let b = next_block(&mut s);
        assert_eq!(
            b.channels[0]
                .samples
                .iter()
                .map(|s| s.value)
                .collect::<Vec<_>>(),
            [1., 3.]
        );
        assert_eq!(b.channels[0].samples[0].sequence, 0);
        assert!(b.channels[0].samples.iter().all(|s| s.flags.is_empty()));
    }
    #[test]
    fn serial_csv_recovers_and_disconnects_without_unbounded_events() {
        let (mut s, mut masters) = pair_source("csv", 1, 2);
        masters[0].write_all(b"1,2\nbad\n").expect("write");
        masters[0].write_all(&[b'x'; 300]).expect("long");
        masters[0].write_all(b"\n3,4\n").expect("recover");
        let b = next_block(&mut s);
        assert_eq!(b.channels[1].samples[1].value, 4.);
        assert_eq!(b.channels[0].samples[1].sequence, 3);
        assert!(b.channels[0].samples[1]
            .flags
            .contains(&SignalFlag::InvalidPacket));
        drop(masters);
        let start = Instant::now();
        while s.poll().is_ok() {
            assert!(start.elapsed() < Duration::from_secs(1));
        }
        let events = s.finish();
        assert_eq!(events.len(), 1);
        let SourcePoll::Event(e) = &events[0] else {
            panic!("diagnostic")
        };
        assert!(e.text.contains("2 decoding errors"));
    }
    #[test]
    fn independent_ports_and_idle_timeout() {
        let (mut s, mut masters) = pair_source("csv", 2, 1);
        masters[0].write_all(b"12\n").expect("first");
        masters[1].write_all(b"34\n").expect("second");
        let a = next_block(&mut s);
        let b = next_block(&mut s);
        assert!(!a.synchronized_clock);
        assert_eq!(a.channels[0].samples[0].value, 12.);
        assert!(a.channels[1].samples.is_empty());
        assert_eq!(b.channels[1].samples[0].value, 34.);
        s.ports[0].last_valid = Instant::now() - Duration::from_secs(1);
        s.origin = Instant::now() - Duration::from_secs(1);
        s.turn = 0;
        assert!(s.poll().err().expect("timeout").contains("no valid frames"));
        let (mut silent, _masters) = pair_source("csv", 1, 1);
        silent.announced = true;
        silent.origin = Instant::now() - Duration::from_secs(1);
        silent.ports[0].last_valid = silent.origin;
        assert!(silent
            .poll()
            .err()
            .expect("silent timeout")
            .contains("no incoming data"));
    }
    #[test]
    fn probe_preserves_bytes_limits_size_and_refuses_overwrite() {
        let (mut master, mut slave) = serialport::TTYPort::pair().expect("pair");
        slave
            .set_timeout(Duration::from_millis(1))
            .expect("timeout");
        let output =
            std::env::temp_dir().join(format!("lemon-probe-{}-данные.bin", std::process::id()));
        let options = ProbeOptions {
            port: "virtual".into(),
            baud: 115200,
            duration: Duration::from_millis(50),
            startup_delay: Duration::ZERO,
            output: output.clone(),
            overwrite: false,
            max_bytes: 60,
        };
        let bytes = b"A0\0A1\xff".repeat(12);
        master.write_all(&bytes).expect("bytes");
        let summary = capture(
            &options,
            &AtomicBool::new(false),
            Box::new(slave.try_clone_native().expect("clone")),
        )
        .expect("capture");
        assert_eq!(summary.bytes, 60);
        assert!(summary.limit_reached && summary.suggests_tagged_u8());
        assert_eq!(std::fs::read(&output).expect("dump"), bytes[..60]);
        assert!(capture(&options, &AtomicBool::new(false), Box::new(slave)).is_err());
        std::fs::remove_file(output).expect("cleanup");
        let (_master, slave) = serialport::TTYPort::pair().expect("cancel pair");
        let cancelled = capture(&options, &AtomicBool::new(true), Box::new(slave)).expect("cancel");
        assert_eq!(cancelled.bytes, 0);
        assert!(std::fs::read(&options.output)
            .expect("cancelled dump")
            .is_empty());
        std::fs::remove_file(&options.output).expect("cleanup cancellation");
    }
    #[test]
    fn probe_startup_bytes_are_excluded_from_measurement() {
        use serialport::SerialPort;
        let (mut master, mut slave) = serialport::TTYPort::pair().expect("pair");
        slave
            .set_timeout(Duration::from_millis(1))
            .expect("timeout");
        master.write_all(b"startup-only").expect("startup");
        let output =
            std::env::temp_dir().join(format!("lemon-probe-startup-{}.bin", std::process::id()));
        let options = ProbeOptions {
            port: "virtual".into(),
            baud: 115200,
            duration: Duration::from_millis(30),
            startup_delay: Duration::from_millis(20),
            output: output.clone(),
            overwrite: false,
            max_bytes: 1000,
        };
        let summary = capture(&options, &AtomicBool::new(false), Box::new(slave)).expect("capture");
        assert_eq!(summary.startup_bytes, 12);
        assert_eq!(summary.bytes, 0);
        assert!(summary.startup_seconds >= 0.020);
        assert!(summary.seconds >= 0.030 && summary.seconds < 0.5);
        assert!(std::fs::read(&output).expect("dump").is_empty());
        std::fs::remove_file(output).expect("cleanup");
    }
}
