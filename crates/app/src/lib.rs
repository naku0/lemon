//! Application orchestration. Acquisition, DSP/storage, test consumer and TUI are independent.
pub mod cli;
use core_types::*;
use processing::Processor;
use source::{SignalSource, SourcePoll};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use storage::{Metadata, SessionWriter};

pub struct Runtime {
    pub stop: Arc<AtomicBool>,
    pub status: Arc<Mutex<RuntimeStatus>>,
    pub commands: SyncSender<Command>,
    pub subscription: Option<Subscription<ProcessedSignalBlock>>,
    workers: Vec<JoinHandle<()>>,
    consumer_count: Arc<AtomicU64>,
    consumer_dropped: Arc<AtomicU64>,
}
impl Runtime {
    pub fn start(config: Config, source: Box<dyn SignalSource>, record: bool) -> Result<Self> {
        Self::start_with_bus(config, source, record, Broadcast::default())
    }
    /// Attach additional bounded subscribers to `bus` before passing it here.
    pub fn start_with_bus(
        config: Config,
        mut source: Box<dyn SignalSource>,
        record: bool,
        mut broadcast: Broadcast<ProcessedSignalBlock>,
    ) -> Result<Self> {
        let processor = Processor::new(&config)?;
        // Fail before starting threads if recording cannot be opened.
        let writer = if record {
            Some(SessionWriter::create(
                Path::new(&config.recording_dir),
                &Metadata::new(config.clone()),
            )?)
        } else {
            None
        };
        let stop = Arc::new(AtomicBool::new(false));
        let disconnect = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new(RuntimeStatus::default()));
        let (commands, command_rx) = mpsc::sync_channel(32);
        let (source_tx, source_rx) = mpsc::sync_channel::<Result<SourcePoll>>(128);
        let subscription = broadcast.subscribe(64);
        let consumer = broadcast.subscribe(64);
        let consumer_count = Arc::new(AtomicU64::new(0));
        let consumer_dropped = consumer.dropped.clone();
        let count = consumer_count.clone();
        let audit = thread::spawn(move || {
            while let Ok(block) = consumer.receiver.recv() {
                let _sequence = block.sequence;
                count.fetch_add(1, Ordering::Relaxed);
            }
        });
        let source_stop = stop.clone();
        let source_disconnect = disconnect.clone();
        let serial = config.source.mode == "serial-test";
        let acquisition = thread::spawn(move || {
            while !source_stop.load(Ordering::Relaxed) && !source_disconnect.load(Ordering::Relaxed)
            {
                let item = source.poll();
                if matches!(item, Ok(SourcePoll::Pending)) {
                    if !serial {
                        thread::sleep(Duration::from_millis(1));
                    }
                    continue;
                }
                let finished = matches!(item, Ok(SourcePoll::End) | Err(_));
                match source_tx.try_send(item) {
                    Ok(()) => {}
                    Err(TrySendError::Disconnected(_)) => break,
                    Err(TrySendError::Full(_)) => {
                        let _=source_tx.send(Err("source: очередь приёма переполнена; поток остановлен, потеря блока зарегистрирована".into()));
                        break;
                    }
                }
                if finished {
                    break;
                }
            }
        });
        let worker_stop = stop.clone();
        let worker_status = status.clone();
        let final_count = consumer_count.clone();
        let final_dropped = consumer_dropped.clone();
        let processing = thread::spawn(move || {
            let mut worker = Worker {
                config,
                processor,
                writer,
                status: worker_status,
                broadcast,
                disconnect,
                consumer_count,
                consumer_dropped,
                last_flush: Instant::now(),
                rate_started: Instant::now(),
                rate_count: 0,
                recording_baseline: (0, 0, 0),
            };
            worker.run(source_rx, command_rx, worker_stop);
        });
        Ok(Self {
            stop,
            status,
            commands,
            subscription: Some(subscription),
            workers: vec![acquisition, processing, audit],
            consumer_count: final_count,
            consumer_dropped: final_dropped,
        })
    }
    pub fn shutdown(mut self) -> Result<RuntimeStatus> {
        self.stop.store(true, Ordering::Relaxed);
        for worker in self.workers.drain(..) {
            worker
                .join()
                .map_err(|_| "Поток приложения аварийно завершился")?;
        }
        let mut status = self
            .status
            .lock()
            .map_err(|_| "runtime status poisoned")?
            .clone();
        status.consumer_blocks = self.consumer_count.load(Ordering::Relaxed);
        status.consumer_dropped = self.consumer_dropped.load(Ordering::Relaxed);
        Ok(status)
    }
}
struct Worker {
    config: Config,
    processor: Processor,
    writer: Option<SessionWriter>,
    status: Arc<Mutex<RuntimeStatus>>,
    broadcast: Broadcast<ProcessedSignalBlock>,
    disconnect: Arc<AtomicBool>,
    consumer_count: Arc<AtomicU64>,
    consumer_dropped: Arc<AtomicU64>,
    last_flush: Instant,
    rate_started: Instant,
    rate_count: u64,
    recording_baseline: (u64, u64, u64),
}
impl Worker {
    fn update(&self, f: impl FnOnce(&mut RuntimeStatus)) {
        f(&mut self.status.lock().unwrap_or_else(|p| p.into_inner()));
    }
    fn now(&self) -> u64 {
        self.status.lock().unwrap_or_else(|p| p.into_inner()).now_ns
    }
    fn event(&mut self, kind: &str, text: impl Into<String>) -> Result<()> {
        let event = Event {
            timestamp_ns: self.now(),
            kind: kind.into(),
            text: text.into(),
        };
        if let Some(writer) = &mut self.writer {
            writer.write_event(&event)?;
        }
        self.update(|s| s.events.push(event));
        Ok(())
    }
    fn fatal(&mut self, domain: &str, error: String) {
        let text = format!("{domain}: {error}");
        self.update(|s| {
            s.errors += 1;
            s.fatal_error = Some(text.clone());
        });
        let _ = self.event(domain, text);
        self.disconnect.store(true, Ordering::Relaxed);
    }
    fn announce_recording(&mut self) {
        let path = self
            .writer
            .as_ref()
            .map(|w| w.directory.join("raw.csv").display().to_string());
        let now = self.now();
        if path.is_some() {
            let s = self.status.lock().unwrap_or_else(|p| p.into_inner());
            self.recording_baseline = (s.received_samples, s.lost_samples, s.errors);
        }
        self.update(|s| {
            s.recording_active = path.is_some();
            s.recording_started_ns = path.as_ref().map(|_| now);
            if path.is_some() {
                s.recording = path;
                s.recording_samples = 0;
            }
        });
    }
    fn summarize_recording(&self) {
        self.update(|s| {
            if let (Some(start), Some(path)) = (s.recording_started_ns, s.recording.clone()) {
                s.recording_summary = Some(RecordingSummary {
                    duration_ns: s.now_ns.saturating_sub(start),
                    samples: s.recording_samples,
                    lost: s.lost_samples.saturating_sub(self.recording_baseline.1),
                    errors: s.errors.saturating_sub(self.recording_baseline.2),
                    path,
                });
            }
        });
    }
    fn command(&mut self, command: Command) -> Result<()> {
        match command {
            Command::StartRecording(request) => {
                if self.writer.is_some() {
                    return self.event(
                        "RecordingError",
                        "Запись уже активна; поля сеанса заблокированы",
                    );
                }
                let mut next = self.config.clone();
                next.session = request.session;
                next.recording_dir = request.directory;
                if let Err(e) = next.validate() {
                    return self.event("RecordingError", e);
                }
                if !request.create_directory && !Path::new(&next.recording_dir).is_dir() {
                    return self.event("RecordingError","Каталог не существует; создайте его или включите Auto create directory в Settings");
                }
                match SessionWriter::create(
                    Path::new(&next.recording_dir),
                    &Metadata::new(next.clone()),
                ) {
                    Ok(writer) => {
                        self.config = next;
                        self.writer = Some(writer);
                        self.announce_recording();
                        self.event("RecordingStarted", "Запись начата")?;
                        self.event("Connected", "Источник уже подключён")?;
                    }
                    Err(e) => {
                        return self
                            .event("RecordingError", format!("Не удалось начать запись: {e}"))
                    }
                }
            }
            Command::ToggleRecording => {
                if self.writer.is_some() {
                    self.event("RecordingStopped", "Запись остановлена пользователем")?;
                    if let Some(mut writer) = self.writer.take() {
                        writer.flush()?;
                    }
                    self.summarize_recording();
                } else {
                    self.writer = Some(SessionWriter::create(
                        Path::new(&self.config.recording_dir),
                        &Metadata::new(self.config.clone()),
                    )?);
                    self.event("RecordingStarted", "Запись начата")?;
                    self.event("Connected", "Источник уже подключён")?;
                }
                self.announce_recording();
            }
            Command::Marker(text) => {
                if self.writer.is_some() {
                    self.event("Marker", text)?;
                } else {
                    self.event("Notice", "Метка не записана: запись выключена")?;
                }
            }
            Command::Disconnect => {
                self.disconnect.store(true, Ordering::Relaxed);
                self.update(|s| s.connection = "Disconnected".into());
                self.event("Disconnected", "Источник отключён пользователем")?;
            }
        }
        Ok(())
    }
    fn run(
        &mut self,
        source: Receiver<Result<SourcePoll>>,
        commands: Receiver<Command>,
        stop: Arc<AtomicBool>,
    ) {
        self.update(|s| s.connection = "Connected".into());
        self.announce_recording();
        if let Err(e) = self.event("Connected", format!("{} source", self.config.source.mode)) {
            self.fatal("StorageError", e);
        }
        loop {
            for _ in 0..32 {
                match commands.try_recv() {
                    Ok(cmd) => {
                        if let Err(e) = self.command(cmd) {
                            self.fatal("StorageError", e)
                        }
                    }
                    Err(_) => break,
                }
            }
            match source.recv_timeout(Duration::from_millis(10)) {
                Ok(Ok(SourcePoll::Block(block))) => {
                    let now = block
                        .channels
                        .iter()
                        .filter_map(|c| c.samples.last().map(|s| s.timestamp_ns))
                        .max()
                        .unwrap_or(block.started_at);
                    let count = block
                        .channels
                        .iter()
                        .map(|c| c.samples.len() as u64)
                        .sum::<u64>();
                    self.update(|s| {
                        s.now_ns = s.now_ns.max(now);
                        s.received_samples += count;
                    });
                    self.rate_count += count;
                    // Persist originals before DSP. Malformed packet diagnostics live in events.csv.
                    if let Some(writer) = &mut self.writer {
                        if let Err(e) = writer.write_block(&block) {
                            self.fatal("StorageError", e);
                        } else {
                            self.update(|s| s.recording_samples += count);
                        }
                    }
                    match self.processor.process(block) {
                        Ok(processed) => {
                            self.update(|s| {
                                s.processed_blocks += 1;
                                s.lost_samples += processed.lost_samples;
                                s.errors += processed.invalid_samples;
                                s.outliers += processed.outliers;
                            });
                            for flag in &processed.flags {
                                if *flag != SignalFlag::InsufficientData
                                    && *flag != SignalFlag::LowQuality
                                {
                                    if let Err(e) = self.event(
                                        &format!("{flag:?}"),
                                        format!(
                                            "block={} lost={} invalid={} outliers={}",
                                            processed.sequence,
                                            processed.lost_samples,
                                            processed.invalid_samples,
                                            processed.outliers
                                        ),
                                    ) {
                                        self.fatal("StorageError", e);
                                        break;
                                    }
                                }
                            }
                            self.broadcast.publish(processed);
                        }
                        Err(e) => {
                            self.update(|s| s.errors += 1);
                            if let Err(err) = self.event("ProcessingError", e) {
                                self.fatal("StorageError", err);
                            }
                        }
                    }
                }
                Ok(Ok(SourcePoll::Event(event))) => {
                    self.update(|s| {
                        s.now_ns = s.now_ns.max(event.timestamp_ns);
                        if event.kind == "InvalidPacket" {
                            s.errors += 1;
                        }
                        if event.kind == "Disconnected" {
                            s.connection = "Disconnected".into();
                        }
                    });
                    if let Err(e) = self.event(&event.kind, event.text) {
                        self.fatal("StorageError", e);
                    }
                }
                Ok(Ok(SourcePoll::End)) => {
                    break;
                }
                Ok(Ok(SourcePoll::Pending)) => {}
                Ok(Err(e)) => {
                    self.update(|s| s.connection = "Disconnected".into());
                    self.fatal("SourceError", e);
                }
                Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => {}
            }
            if self.rate_started.elapsed() >= Duration::from_secs(1) {
                let rate = self.rate_count as f64
                    / self.config.source.channels as f64
                    / self.rate_started.elapsed().as_secs_f64();
                self.update(|s| s.rate_hz = rate);
                self.rate_count = 0;
                self.rate_started = Instant::now();
            }
            self.update(|s| {
                s.consumer_blocks = self.consumer_count.load(Ordering::Relaxed);
                s.consumer_dropped = self.consumer_dropped.load(Ordering::Relaxed);
            });
            if self.last_flush.elapsed() >= Duration::from_secs(1) {
                if let Some(writer) = &mut self.writer {
                    if let Err(e) = writer.flush() {
                        self.fatal("StorageError", e);
                    }
                }
                self.last_flush = Instant::now();
            }
            // On cancellation acquisition stops; continue draining queued blocks before flush.
            if stop.load(Ordering::Relaxed) {
                self.disconnect.store(true, Ordering::Relaxed);
            }
        }
        if let Err(e) = self.event("Disconnected", "Источник завершён; очередь обработана")
        {
            self.fatal("StorageError", e);
        }
        if let Some(writer) = &mut self.writer {
            if let Err(e) = writer.flush() {
                self.fatal("StorageError", e);
            }
        }
        self.update(|s| {
            s.finished = true;
            s.recording_active = false;
            s.connection = "Disconnected".into();
        });
        self.summarize_recording();
    }
}
