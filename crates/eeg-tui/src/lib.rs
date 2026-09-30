//! Presentation, bounded result histories and keyboard. No signal processing here.
pub mod history;
mod render;
pub mod settings;
mod settings_ui;
use crossterm::{
    cursor,
    event::{self, Event as TerminalEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use eeg_core::*;
use history::BandHistory;
use ratatui::{backend::CrosstermBackend, Terminal};
use settings::{Settings, WaveMode};
use settings_ui::{Field, FIELDS};
use std::{
    io::{self, IsTerminal},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::SyncSender,
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

pub struct UiOptions {
    pub settings: Settings,
    pub settings_path: PathBuf,
    pub interrupt: Arc<AtomicBool>,
    pub data_directory_locked: bool,
    pub experiment_directory: String,
    pub experiment_session_name: String,
}
struct TerminalGuard;
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore();
    }
}
fn restore() {
    let _ = terminal::disable_raw_mode();
    let _ = execute!(io::stdout(), LeaveAlternateScreen, cursor::Show);
}
#[derive(Clone)]
enum EditTarget {
    Setting(Field),
    Session(usize),
    Marker,
}
#[derive(Clone)]
enum Overlay {
    Help,
    Quit,
    Reset,
    Edit(EditTarget, String),
    Summary(RecordingSummary),
}
struct View {
    page: usize,
    paused: bool,
    settings: Settings,
    settings_path: PathBuf,
    dirty: bool,
    history: Vec<RingBuffer<(f64, f64, Option<f64>)>>,
    band_history: BandHistory,
    latest: Option<Arc<ProcessedSignalBlock>>,
    spectrum: Option<SpectrumSnapshot>,
    bands: Vec<BandPower>,
    overlay: Option<Overlay>,
    message: String,
    selection: usize,
    session_selection: usize,
    session: SessionConfig,
    directory: String,
    directory_locked: bool,
    experiment_directory: String,
    experiment_session_name: String,
    record_requested: bool,
    seen_summary: Option<(String, u64)>,
    last_event: Option<(u64, String, String)>,
    source_finished_seen: bool,
}
impl View {
    fn new(config: &Config, options: UiOptions) -> Self {
        Self {
            page: options.settings.general.start_tab,
            paused: false,
            settings: options.settings,
            settings_path: options.settings_path,
            dirty: false,
            history: (0..config.source.channels)
                .map(|_| {
                    RingBuffer::new(
                        (config.source.sample_rate_hz * 10.).ceil().min(100_000.) as usize
                    )
                })
                .collect(),
            band_history: BandHistory::default(),
            latest: None,
            spectrum: None,
            bands: Vec::new(),
            overlay: None,
            message: String::new(),
            selection: 0,
            session_selection: 0,
            session: config.session.clone(),
            directory: config.recording_dir.clone(),
            directory_locked: options.data_directory_locked,
            experiment_directory: options.experiment_directory,
            experiment_session_name: options.experiment_session_name,
            record_requested: false,
            seen_summary: None,
            last_event: None,
            source_finished_seen: false,
        }
    }
    fn accept(&mut self, b: Arc<ProcessedSignalBlock>) {
        if self.paused {
            return;
        }
        let discontinuity = self
            .latest
            .as_ref()
            .is_some_and(|last| b.sequence != last.sequence.saturating_add(1))
            || b.flags.iter().any(|f| {
                matches!(
                    f,
                    SignalFlag::InsufficientData
                        | SignalFlag::LostSample
                        | SignalFlag::InvalidPacket
                        | SignalFlag::Disconnected
                )
            });
        if discontinuity {
            self.spectrum = None;
            self.bands.clear();
        }
        for (i, c) in b.raw_channels.iter().enumerate() {
            if let Some(history) = self.history.get_mut(i) {
                for (j, s) in c.samples.iter().enumerate() {
                    history.push((
                        s.timestamp_ns as f64 / 1e9,
                        s.value,
                        b.filtered_channels
                            .get(i)
                            .and_then(|c| c.samples.get(j))
                            .copied()
                            .flatten(),
                    ));
                }
            }
        }
        self.band_history
            .accept(&b, self.settings.spectral.history_seconds);
        if let Some(s) = &b.spectrum {
            self.spectrum = Some(s.clone());
        }
        if let Some(p) = &b.band_power {
            self.bands = p.clone();
        }
        if b.flags.contains(&SignalFlag::ChannelMismatch) {
            for band in &mut self.bands {
                band.asymmetry = None;
            }
        }
        self.latest = Some(b);
    }
    fn selected(&self, i: usize) -> bool {
        self.settings.waveform.channels == 0 || self.settings.waveform.channels == i + 1
    }
    fn request_quit(&mut self, status: &RuntimeStatus) -> bool {
        if status.recording_active || self.record_requested || self.settings.general.confirm_exit {
            self.overlay = Some(Overlay::Quit);
            false
        } else {
            true
        }
    }
    fn send(&mut self, tx: &SyncSender<Command>, cmd: Command) {
        if let Err(e) = tx.try_send(cmd) {
            self.message = format!("Command rejected: {e}");
            self.record_requested = false;
        }
    }
    fn notice_status(&mut self, status: &RuntimeStatus) {
        if status.finished && !self.source_finished_seen {
            self.source_finished_seen = true;
            self.spectrum = None;
            self.bands.clear();
            self.band_history.break_line();
        }
        if status.recording_active || status.finished {
            self.record_requested = false;
        }
        if let Some(e) = status
            .events
            .iter()
            .filter(|e| matches!(e.kind.as_str(), "Marker" | "RecordingError"))
            .last()
        {
            let key = (e.timestamp_ns, e.kind.clone(), e.text.clone());
            if self.last_event.as_ref() != Some(&key) {
                if e.kind == "Marker" {
                    self.message = format!("Marker saved: {}", e.text);
                } else if e.kind == "RecordingError" {
                    self.record_requested = false;
                    self.message = e.text.clone();
                }
                self.last_event = Some(key);
            }
        }
        if let Some(summary) = &status.recording_summary {
            let key = (summary.path.clone(), summary.duration_ns);
            if self.seen_summary.as_ref() != Some(&key) {
                self.seen_summary = Some(key);
                if self.settings.storage.show_summary && self.overlay.is_none() {
                    self.overlay = Some(Overlay::Summary(summary.clone()));
                }
            }
        }
    }
    fn recording(&mut self, tx: &SyncSender<Command>, status: &RuntimeStatus) {
        if status.finished {
            self.message = "Source finished. Restart LEMON for a new connection.".into();
            return;
        }
        if status.recording_active {
            self.send(tx, Command::ToggleRecording);
        } else if !self.record_requested {
            self.record_requested = true;
            self.send(
                tx,
                Command::StartRecording(RecordingRequest {
                    session: self.session.clone(),
                    directory: self.directory.clone(),
                    create_directory: self.settings.storage.create_directory,
                }),
            );
        }
    }
    fn apply_settings(&mut self, previous: &Settings) {
        self.dirty = true;
        if !self.directory_locked && previous.storage.directory != self.settings.storage.directory {
            self.directory = self
                .settings
                .storage
                .directory
                .clone()
                .unwrap_or_else(|| self.experiment_directory.clone());
        }
        if previous.storage.session_name != self.settings.storage.session_name {
            self.session.name = self
                .settings
                .storage
                .session_name
                .clone()
                .unwrap_or_else(|| self.experiment_session_name.clone());
        }
        if let Some(time) = self.band_history.latest_time() {
            self.band_history
                .trim(time, self.settings.spectral.history_seconds);
        }
    }
    fn edit_value(&self, index: usize) -> String {
        match index {
            0 => self.session.name.clone(),
            1 => self.session.user_id.clone(),
            2 | 3 => self
                .session
                .electrodes
                .get(index - 2)
                .cloned()
                .unwrap_or_default(),
            4 => self.session.notes.clone(),
            _ => self.directory.clone(),
        }
    }
    fn save_edit(
        &mut self,
        target: EditTarget,
        text: String,
        status: &RuntimeStatus,
        tx: &SyncSender<Command>,
    ) -> Result<()> {
        match target {
            EditTarget::Setting(field) => {
                let previous = self.settings.clone();
                field.set(&mut self.settings, &text)?;
                self.apply_settings(&previous);
            }
            EditTarget::Marker => {
                if text.trim().is_empty() {
                    return Err("Marker cannot be empty".into());
                }
                self.send(tx, Command::Marker(text));
                self.message = "Saving marker...".into();
            }
            EditTarget::Session(index) => {
                if status.recording_active || self.record_requested {
                    return Err("Session fields are locked while recording".into());
                }
                if text.len() > if index >= 4 { 4096 } else { 256 } {
                    return Err("Value is too long".into());
                }
                if matches!(index, 0 | 5) && text.trim().is_empty() {
                    return Err("Name and directory cannot be empty".into());
                }
                match index {
                    0 => self.session.name = text,
                    1 => self.session.user_id = text,
                    2 | 3 => {
                        self.session.electrodes.resize(2, String::new());
                        self.session.electrodes[index - 2] = text;
                    }
                    4 => self.session.notes = text,
                    _ => self.directory = text,
                }
            }
        }
        Ok(())
    }
    fn key(
        &mut self,
        key: KeyEvent,
        status: &RuntimeStatus,
        tx: &SyncSender<Command>,
        config: &Config,
    ) -> bool {
        if key.kind != KeyEventKind::Press {
            return false;
        }
        if let Some(overlay) = self.overlay.take() {
            match overlay {
                Overlay::Help => {
                    if !matches!(key.code, KeyCode::Esc | KeyCode::Char('?')) {
                        self.overlay = Some(Overlay::Help);
                    }
                }
                Overlay::Quit => match key.code {
                    KeyCode::Char('y' | 'Y') => return true,
                    KeyCode::Char('n' | 'N') | KeyCode::Esc => {}
                    _ => self.overlay = Some(Overlay::Quit),
                },
                Overlay::Reset => {
                    if matches!(key.code, KeyCode::Char('y' | 'Y')) {
                        let previous = self.settings.clone();
                        self.settings = Settings::default();
                        self.apply_settings(&previous);
                    } else if !matches!(key.code, KeyCode::Char('n' | 'N') | KeyCode::Esc) {
                        self.overlay = Some(Overlay::Reset);
                    }
                }
                Overlay::Summary(summary) => {
                    if !matches!(key.code, KeyCode::Esc | KeyCode::Enter) {
                        self.overlay = Some(Overlay::Summary(summary));
                    }
                }
                Overlay::Edit(target, mut text) => match key.code {
                    KeyCode::Esc => {}
                    KeyCode::Enter => {
                        if let Err(e) = self.save_edit(target.clone(), text.clone(), status, tx) {
                            self.message = e;
                            self.overlay = Some(Overlay::Edit(target, text));
                        }
                    }
                    KeyCode::Backspace => {
                        text.pop();
                        self.overlay = Some(Overlay::Edit(target, text));
                    }
                    KeyCode::Char(c)
                        if !key.modifiers.contains(KeyModifiers::CONTROL) && text.len() < 4096 =>
                    {
                        text.push(c);
                        self.overlay = Some(Overlay::Edit(target, text));
                    }
                    _ => self.overlay = Some(Overlay::Edit(target, text)),
                },
            }
            return false;
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return self.request_quit(status);
        }
        if self.page == 5 {
            let field = FIELDS[self.selection];
            let previous = self.settings.clone();
            match key.code {
                KeyCode::Up => self.selection = self.selection.saturating_sub(1),
                KeyCode::Down => self.selection = (self.selection + 1).min(FIELDS.len() - 1),
                KeyCode::Left => field.adjust(&mut self.settings, -1),
                KeyCode::Right | KeyCode::Char(' ') => field.adjust(&mut self.settings, 1),
                KeyCode::Enter => {
                    if field.group() == 5 {
                        self.message = field.hint().into();
                    } else if field.text() {
                        self.overlay = Some(Overlay::Edit(
                            EditTarget::Setting(field),
                            field.value(&self.settings, config),
                        ));
                    } else {
                        field.adjust(&mut self.settings, 1);
                    }
                }
                KeyCode::Char('s' | 'S') => match self.settings.save(&self.settings_path) {
                    Ok(()) => {
                        self.dirty = false;
                        self.message = format!("Settings saved: {}", self.settings_path.display());
                    }
                    Err(e) => self.message = e,
                },
                KeyCode::Char('r' | 'R') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.overlay = Some(Overlay::Reset)
                }
                KeyCode::Char('r' | 'R') => self.settings.reset_group(field.group()),
                _ => return self.global_key(key, status, tx, config),
            }
            if previous != self.settings {
                self.apply_settings(&previous);
            }
            return false;
        }
        if self.page == 4 {
            match key.code {
                KeyCode::Up => {
                    self.session_selection = self.session_selection.saturating_sub(1);
                    return false;
                }
                KeyCode::Down => {
                    self.session_selection = (self.session_selection + 1).min(5);
                    return false;
                }
                KeyCode::Enter => {
                    if status.recording_active || self.record_requested {
                        self.message = "Session fields are locked while recording".into();
                    } else {
                        self.overlay = Some(Overlay::Edit(
                            EditTarget::Session(self.session_selection),
                            self.edit_value(self.session_selection),
                        ));
                    }
                    return false;
                }
                _ => {}
            }
        }
        self.global_key(key, status, tx, config)
    }
    fn global_key(
        &mut self,
        key: KeyEvent,
        status: &RuntimeStatus,
        tx: &SyncSender<Command>,
        config: &Config,
    ) -> bool {
        match key.code {
            KeyCode::Char('q' | 'Q') | KeyCode::Esc => return self.request_quit(status),
            KeyCode::Char('?') => self.overlay = Some(Overlay::Help),
            KeyCode::Tab => self.page = (self.page + 1) % 6,
            KeyCode::BackTab => self.page = (self.page + 5) % 6,
            KeyCode::Char(c @ '1'..='6') => self.page = c as usize - '1' as usize,
            KeyCode::Char('r' | 'R') => self.recording(tx, status),
            KeyCode::Char('m' | 'M') => {
                if status.recording_active {
                    self.overlay = Some(Overlay::Edit(EditTarget::Marker, String::new()));
                } else {
                    self.message = "Start recording before adding a marker".into();
                }
            }
            KeyCode::Char('d' | 'D') if self.page == 0 => self.send(tx, Command::Disconnect),
            KeyCode::Char(' ') => {
                self.paused = !self.paused;
                if !self.paused {
                    self.band_history.break_line();
                    self.spectrum = None;
                    self.bands.clear();
                }
            }
            KeyCode::Char('v' | 'V') if self.page == 1 => {
                self.settings.waveform.mode = match self.settings.waveform.mode {
                    WaveMode::Raw => WaveMode::Filtered,
                    WaveMode::Filtered => WaveMode::Both,
                    WaveMode::Both => WaveMode::Raw,
                };
                self.dirty = true;
            }
            KeyCode::Char('c' | 'C') if matches!(self.page, 1..=3) => {
                self.settings.waveform.channels =
                    (self.settings.waveform.channels + 1) % (config.source.channels + 1);
                self.dirty = true;
            }
            KeyCode::Char('a' | 'A') if self.page == 1 => {
                self.settings.waveform.auto_scale = !self.settings.waveform.auto_scale;
                self.dirty = true;
            }
            KeyCode::Char('p' | 'P') if self.page == 3 => {
                Field::Power.adjust(&mut self.settings, 1);
                self.dirty = true;
            }
            KeyCode::Char('+') | KeyCode::Char('=') if self.page == 1 => {
                self.settings.waveform.seconds = (self.settings.waveform.seconds + 1.).min(10.);
                self.dirty = true;
            }
            KeyCode::Char('-') if self.page == 1 => {
                self.settings.waveform.seconds = (self.settings.waveform.seconds - 1.).max(1.);
                self.dirty = true;
            }
            _ => {}
        }
        false
    }
}

pub fn run(
    config: &Config,
    ports: &[String],
    subscription: Subscription<ProcessedSignalBlock>,
    commands: SyncSender<Command>,
    status: Arc<Mutex<RuntimeStatus>>,
    stop: Arc<AtomicBool>,
    options: UiOptions,
) -> Result<Settings> {
    if !io::stdout().is_terminal() || !io::stdin().is_terminal() {
        return Err("TUI requires a terminal; use lemon demo --headless --duration 10".into());
    }
    let old_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore();
        old_hook(info);
    }));
    let _guard = TerminalGuard;
    terminal::enable_raw_mode().map_err(|e| e.to_string())?;
    execute!(io::stdout(), EnterAlternateScreen, cursor::Hide).map_err(|e| e.to_string())?;
    let mut terminal =
        Terminal::new(CrosstermBackend::new(io::stdout())).map_err(|e| e.to_string())?;
    let interrupt = options.interrupt.clone();
    let mut view = View::new(config, options);
    let mut redraw = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        for _ in 0..128 {
            match subscription.receiver.try_recv() {
                Ok(b) => view.accept(b),
                Err(_) => break,
            }
        }
        let snapshot = status
            .lock()
            .map_err(|_| "runtime status poisoned")?
            .clone();
        view.notice_status(&snapshot);
        if interrupt.swap(false, Ordering::Relaxed) && view.request_quit(&snapshot) {
            stop.store(true, Ordering::Relaxed);
            break;
        }
        if redraw.elapsed() >= Duration::from_secs_f64(1. / view.settings.general.fps as f64) {
            terminal
                .draw(|f| {
                    render::draw(
                        f,
                        config,
                        ports,
                        &view,
                        &snapshot,
                        subscription.dropped.load(Ordering::Relaxed),
                    )
                })
                .map_err(|e| e.to_string())?;
            redraw = Instant::now();
        }
        if event::poll(Duration::from_millis(10)).map_err(|e| e.to_string())? {
            if let TerminalEvent::Key(key) = event::read().map_err(|e| e.to_string())? {
                if view.key(key, &snapshot, &commands, config) {
                    stop.store(true, Ordering::Relaxed);
                    break;
                }
            }
        }
    }
    Ok(view.settings)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn view() -> View {
        View::new(
            &Config::default(),
            UiOptions {
                settings: Settings::default(),
                settings_path: PathBuf::from("unused"),
                interrupt: Arc::new(AtomicBool::new(false)),
                data_directory_locked: false,
                experiment_directory: Config::default().recording_dir,
                experiment_session_name: Config::default().session.name,
            },
        )
    }
    #[test]
    fn storage_reset_restores_experiment_defaults_and_respects_cli_directory() {
        let mut v = view();
        v.settings.storage.directory = Some("user-dir".into());
        v.settings.storage.session_name = Some("user-session".into());
        v.apply_settings(&Settings::default());
        assert_eq!(v.directory, "user-dir");
        let before = v.settings.clone();
        v.settings.reset_group(1);
        v.apply_settings(&before);
        assert_eq!(v.directory, v.experiment_directory);
        assert_eq!(v.session.name, v.experiment_session_name);
        v.directory_locked = true;
        v.directory = "cli-dir".into();
        v.settings.storage.directory = Some("different".into());
        v.apply_settings(&Settings::default());
        assert_eq!(v.directory, "cli-dir");
    }
    #[test]
    fn active_recording_always_confirms_quit() {
        let mut v = view();
        let mut status = RuntimeStatus {
            recording_active: true,
            ..RuntimeStatus::default()
        };
        assert!(!v.request_quit(&status));
        assert!(matches!(v.overlay, Some(Overlay::Quit)));
        let (tx, _) = std::sync::mpsc::sync_channel(1);
        assert!(!v.key(
            KeyEvent::from(KeyCode::Char('n')),
            &status,
            &tx,
            &Config::default()
        ));
        assert!(v.overlay.is_none());
        status.recording_active = false;
        assert!(v.request_quit(&status));
    }
    #[test]
    fn escape_closes_help_without_quitting() {
        let mut v = view();
        v.overlay = Some(Overlay::Help);
        let (tx, _) = std::sync::mpsc::sync_channel(1);
        assert!(!v.key(
            KeyEvent::from(KeyCode::Esc),
            &RuntimeStatus::default(),
            &tx,
            &Config::default()
        ));
        assert!(v.overlay.is_none());
    }
    #[test]
    fn advanced_does_not_change_dsp() {
        let mut s = Settings::default();
        let before = s.clone();
        for f in FIELDS.iter().filter(|f| f.group() == 5) {
            f.adjust(&mut s, 1);
        }
        assert_eq!(s, before);
    }
}
