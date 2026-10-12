use crate::settings::*;
use core_types::{Config, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Fps,
    StartTab,
    Confirm,
    Unicode,
    Compact,
    Directory,
    SessionName,
    CreateDir,
    Summary,
    Theme,
    Raw,
    Filtered,
    Channel1,
    Channel2,
    Theta,
    Alpha,
    Beta,
    Legends,
    Borders,
    WaveMode,
    WaveStyle,
    RawBrightness,
    FilteredBrightness,
    EnvelopeThreshold,
    GapTolerance,
    Window,
    AutoScale,
    FixedScale,
    Channels,
    LogScale,
    Power,
    VisibleBands,
    Smoothed,
    History,
    HighPass,
    LowPass,
    Notch,
    NotchHz,
    FftWindow,
    FftHop,
    BandBounds,
    Source,
    AcquisitionChannels,
    SerialReplay,
    SessionMetadata,
    Quality,
    Diagnostics,
    Reset,
}
pub const FIELDS: [Field; 48] = [
    Field::Fps,
    Field::StartTab,
    Field::Confirm,
    Field::Unicode,
    Field::Compact,
    Field::Directory,
    Field::SessionName,
    Field::CreateDir,
    Field::Summary,
    Field::Theme,
    Field::Raw,
    Field::Filtered,
    Field::Channel1,
    Field::Channel2,
    Field::Theta,
    Field::Alpha,
    Field::Beta,
    Field::Legends,
    Field::Borders,
    Field::WaveMode,
    Field::WaveStyle,
    Field::RawBrightness,
    Field::FilteredBrightness,
    Field::EnvelopeThreshold,
    Field::GapTolerance,
    Field::Window,
    Field::AutoScale,
    Field::FixedScale,
    Field::Channels,
    Field::LogScale,
    Field::Power,
    Field::VisibleBands,
    Field::Smoothed,
    Field::History,
    Field::HighPass,
    Field::LowPass,
    Field::Notch,
    Field::NotchHz,
    Field::FftWindow,
    Field::FftHop,
    Field::BandBounds,
    Field::Source,
    Field::AcquisitionChannels,
    Field::SerialReplay,
    Field::SessionMetadata,
    Field::Quality,
    Field::Diagnostics,
    Field::Reset,
];
pub const CATEGORIES: [&str; 5] = [
    "Appearance",
    "Recording",
    "Acquisition",
    "Processing",
    "Advanced",
];
pub const GROUPS: [&str; 6] = [
    "General",
    "Storage",
    "Appearance",
    "Waveform",
    "Spectrum and Bands",
    "Advanced (read only)",
];
impl Field {
    pub fn category(self) -> usize {
        match self {
            Self::Directory
            | Self::SessionName
            | Self::CreateDir
            | Self::Summary
            | Self::SessionMetadata => 1,
            Self::Source | Self::AcquisitionChannels | Self::SerialReplay => 2,
            Self::HighPass
            | Self::LowPass
            | Self::Notch
            | Self::NotchHz
            | Self::FftWindow
            | Self::FftHop
            | Self::BandBounds
            | Self::Quality => 3,
            Self::Diagnostics | Self::Reset => 4,
            _ => 0,
        }
    }
    pub fn section(self) -> &'static str {
        match self {
            Self::SessionMetadata | Self::SessionName => "Session metadata",
            Self::Source | Self::AcquisitionChannels | Self::SerialReplay => {
                "Source / channels / transport"
            }
            Self::Quality => "Quality thresholds (engineering heuristic)",
            Self::Diagnostics | Self::Reset => "Diagnostics / reset",
            _ => match self.group() {
                0 => "Interface",
                2 => "Theme / colors",
                3 => "Charts / Waveform",
                4 => "Charts / Spectrum and Bands",
                5 => "Filters / Spectrum / Bands (read only)",
                n => GROUPS[n],
            },
        }
    }
    pub fn navigate(self, delta: isize) -> usize {
        let indices: Vec<_> = FIELDS
            .iter()
            .enumerate()
            .filter(|(_, f)| f.category() == self.category())
            .map(|(i, _)| i)
            .collect();
        let pos = indices.iter().position(|&i| FIELDS[i] == self).unwrap_or(0);
        indices[pos.saturating_add_signed(delta).min(indices.len() - 1)]
    }
    pub fn next_category(self, delta: isize) -> usize {
        let category =
            (self.category() as isize + delta).rem_euclid(CATEGORIES.len() as isize) as usize;
        FIELDS
            .iter()
            .position(|f| f.category() == category)
            .unwrap_or(0)
    }
    pub fn group(self) -> usize {
        match self {
            Self::Fps | Self::StartTab | Self::Confirm | Self::Unicode | Self::Compact => 0,
            Self::Directory | Self::SessionName | Self::CreateDir | Self::Summary => 1,
            Self::Theme
            | Self::Raw
            | Self::Filtered
            | Self::Channel1
            | Self::Channel2
            | Self::Theta
            | Self::Alpha
            | Self::Beta
            | Self::Legends
            | Self::Borders => 2,
            Self::WaveMode
            | Self::WaveStyle
            | Self::RawBrightness
            | Self::FilteredBrightness
            | Self::EnvelopeThreshold
            | Self::GapTolerance
            | Self::Window
            | Self::AutoScale
            | Self::FixedScale
            | Self::Channels => 3,
            Self::LogScale | Self::Power | Self::VisibleBands | Self::Smoothed | Self::History => 4,
            _ => 5,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Fps => "TUI FPS",
            Self::StartTab => "Startup tab (1-6)",
            Self::Confirm => "Confirm exit (always when REC)",
            Self::Unicode => "Unicode",
            Self::Compact => "Compact layout",
            Self::Directory => "Default recording directory",
            Self::SessionName => "Default session name",
            Self::CreateDir => "Auto create directory",
            Self::Summary => "Show recording summary",
            Self::Theme => "Theme",
            Self::Raw => "Raw color (Custom)",
            Self::Filtered => "Filtered color (Custom)",
            Self::Channel1 => "Channel 1 color (Custom)",
            Self::Channel2 => "Channel 2 color (Custom)",
            Self::Theta => "Theta color (Custom)",
            Self::Alpha => "Alpha/Mu color (Custom)",
            Self::Beta => "Beta color (Custom)",
            Self::Legends => "Legends",
            Self::Borders => "Borders",
            Self::WaveMode => "Waveform view",
            Self::WaveStyle => "Waveform style",
            Self::RawBrightness => "Raw brightness (0.1-1)",
            Self::FilteredBrightness => "Filtered brightness (0.1-1)",
            Self::EnvelopeThreshold => "Auto envelope samples/column (0=off)",
            Self::GapTolerance => "Gap tolerance (sample periods)",
            Self::Window => "Visible window, s",
            Self::AutoScale => "Auto amplitude scale",
            Self::FixedScale => "Fixed amplitude +/-",
            Self::Channels => "Channels: 0=all, 1, 2",
            Self::LogScale => "Logarithmic PSD (dB)",
            Self::Power => "Band power view",
            Self::VisibleBands => "Visible bands (comma list; empty=all)",
            Self::Smoothed => "Show existing EMA (absolute only)",
            Self::History => "Band history, s (max 10000 points)",
            Self::HighPass => "High-pass, Hz",
            Self::LowPass => "Low-pass, Hz",
            Self::Notch => "Notch enabled",
            Self::NotchHz => "Notch frequency, Hz",
            Self::FftWindow => "FFT window, samples",
            Self::FftHop => "FFT hop, samples",
            Self::BandBounds => "Frequency bands, Hz",
            Self::Source => "Source / nominal sample rate",
            Self::AcquisitionChannels => "Input channels / units",
            Self::SerialReplay => "Serial / replay options",
            Self::SessionMetadata => "Session metadata",
            Self::Quality => "Quality thresholds",
            Self::Diagnostics => "Version / block size",
            Self::Reset => "Reset preferences",
        }
    }
    pub fn text(self) -> bool {
        matches!(
            self,
            Self::Directory
                | Self::SessionName
                | Self::FixedScale
                | Self::Window
                | Self::History
                | Self::Fps
                | Self::VisibleBands
                | Self::RawBrightness
                | Self::FilteredBrightness
                | Self::EnvelopeThreshold
                | Self::GapTolerance
        )
    }
    pub fn hint(self) -> &'static str {
        match self {
            Self::Reset => "Ctrl+R opens reset confirmation; Esc cancels. S saves preferences.",
            Self::Diagnostics => "Read only; effective launch configuration. No acquisition settings are modified here.",
            Self::Source | Self::AcquisitionChannels | Self::SerialReplay => "Read only; change CLI/experiment JSON on next launch. Active acquisition is unchanged.",
            Self::SessionMetadata => "Read only here; edit the Recording screen for the next recording.",
            Self::StartTab => "Next launch; save with S",
            Self::GapTolerance => "New incoming samples; display only, does not change DSP",
            Self::WaveStyle => "Immediate, display only: Line/Points/Envelope; manual selection disables auto envelope",
            Self::Directory | Self::SessionName | Self::CreateDir => {
                "Next recording; active session is unchanged"
            }
            _ if self.group() == 5 => "Read only; edit experiment JSON and restart to change DSP",
            _ => "Applies immediately; S saves user preferences only",
        }
    }
    pub fn value(self, s: &Settings, c: &Config) -> String {
        match self {
            Self::Fps => s.general.fps.to_string(),
            Self::StartTab => (s.general.start_tab + 1).to_string(),
            Self::Confirm => s.general.confirm_exit.to_string(),
            Self::Unicode => s.general.unicode.to_string(),
            Self::Compact => s.general.compact.to_string(),
            Self::Directory => s.storage.directory.clone().unwrap_or_default(),
            Self::SessionName => s.storage.session_name.clone().unwrap_or_default(),
            Self::CreateDir => s.storage.create_directory.to_string(),
            Self::Summary => s.storage.show_summary.to_string(),
            Self::Theme => format!("{:?}", s.appearance.theme),
            Self::Raw => s.appearance.raw.clone(),
            Self::Filtered => s.appearance.filtered.clone(),
            Self::Channel1 => s.appearance.channels[0].clone(),
            Self::Channel2 => s.appearance.channels[1].clone(),
            Self::Theta => s.appearance.bands[0].clone(),
            Self::Alpha => s.appearance.bands[1].clone(),
            Self::Beta => s.appearance.bands[2].clone(),
            Self::Legends => s.appearance.legends.to_string(),
            Self::Borders => s.appearance.borders.to_string(),
            Self::WaveMode => format!("{:?}", s.waveform.mode),
            Self::Window => s.waveform.seconds.to_string(),
            Self::WaveStyle => format!("{:?}", s.waveform.style),
            Self::RawBrightness => s.waveform.raw_brightness.to_string(),
            Self::FilteredBrightness => s.waveform.filtered_brightness.to_string(),
            Self::EnvelopeThreshold => s.waveform.auto_envelope_threshold.to_string(),
            Self::GapTolerance => s.waveform.gap_tolerance.to_string(),
            Self::AutoScale => s.waveform.auto_scale.to_string(),
            Self::FixedScale => s.waveform.fixed_max.to_string(),
            Self::Channels => s.waveform.channels.to_string(),
            Self::LogScale => s.spectral.log_scale.to_string(),
            Self::Power => format!("{:?}", s.spectral.power),
            Self::VisibleBands => s.spectral.visible_bands.join(", "),
            Self::Smoothed => s.spectral.smoothed.to_string(),
            Self::History => s.spectral.history_seconds.to_string(),
            Self::HighPass => c.processing.high_pass_hz.to_string(),
            Self::LowPass => c.processing.low_pass_hz.to_string(),
            Self::Notch => c.processing.notch.to_string(),
            Self::NotchHz => c.processing.notch_hz.to_string(),
            Self::FftWindow => c.processing.window_samples.to_string(),
            Self::FftHop => c.processing.hop_samples.to_string(),
            Self::BandBounds => c
                .processing
                .bands
                .iter()
                .map(|b| format!("{} {}-{}", b.name, b.low_hz, b.high_hz))
                .collect::<Vec<_>>()
                .join("; "),
            Self::Source => format!("{} / {} Hz", c.source.mode, c.source.sample_rate_hz),
            Self::AcquisitionChannels => format!(
                "{} / {}",
                c.source.channels,
                c.source.units.as_deref().unwrap_or("a.u.")
            ),
            Self::SerialReplay => format!(
                "{} @ {} baud; ports {}; replay {} x{}",
                c.source.serial_protocol,
                c.source.baud_rate,
                c.source.ports.join(", "),
                c.source.replay_path,
                c.source.replay_speed
            ),
            Self::SessionMetadata => format!(
                "{}; electrodes {}; {}",
                c.session.name,
                c.session.electrodes.join(", "),
                c.session.notes
            ),
            Self::Quality => format!(
                "abs max {}; step {}; flat std {}; mains ratio {}; sync {} ms",
                c.processing.expected_abs_max,
                c.processing.outlier_step,
                c.processing.flat_stddev,
                c.processing.mains_ratio_limit,
                c.processing.sync_tolerance_ms
            ),
            Self::Diagnostics => format!(
                "LEMON {}; {} samples/block",
                env!("CARGO_PKG_VERSION"),
                c.source.block_size
            ),
            Self::Reset => "R: current preference group; Ctrl+R: all (confirm)".into(),
        }
    }
    pub fn set(self, s: &mut Settings, text: &str) -> Result<()> {
        let mut next = s.clone();
        let number = || {
            text.parse::<f64>()
                .map_err(|_| "Enter a finite number".to_string())
        };
        match self {
            Self::Fps => next.general.fps = text.parse().map_err(|_| "FPS: integer 1..60")?,
            Self::Window => next.waveform.seconds = number()?,
            Self::RawBrightness => next.waveform.raw_brightness = number()?,
            Self::FilteredBrightness => next.waveform.filtered_brightness = number()?,
            Self::EnvelopeThreshold => next.waveform.auto_envelope_threshold = number()?,
            Self::GapTolerance => next.waveform.gap_tolerance = number()?,
            Self::FixedScale => next.waveform.fixed_max = number()?,
            Self::History => next.spectral.history_seconds = number()?,
            Self::Directory => next.storage.directory = (!text.is_empty()).then(|| text.into()),
            Self::SessionName => {
                next.storage.session_name = (!text.is_empty()).then(|| text.into())
            }
            Self::VisibleBands => {
                next.spectral.visible_bands = text
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect()
            }
            _ => return Err("Use Left/Right to change this value".into()),
        }
        next.validate()?;
        *s = next;
        Ok(())
    }
    pub fn adjust(self, s: &mut Settings, delta: i32) {
        fn index(i: usize, delta: i32, len: usize) -> usize {
            (i as i32 + delta).rem_euclid(len as i32) as usize
        }
        fn color(value: &mut String, delta: i32) {
            let i = COLORS.iter().position(|c| *c == value).unwrap_or(0);
            *value = COLORS[index(i, delta, COLORS.len())].into();
        }
        match self {
            Self::Fps => s.general.fps = (s.general.fps as i32 + delta).clamp(1, 60) as u32,
            Self::StartTab => s.general.start_tab = index(s.general.start_tab, delta, 6),
            Self::Confirm => s.general.confirm_exit = !s.general.confirm_exit,
            Self::Unicode => s.general.unicode = !s.general.unicode,
            Self::Compact => s.general.compact = !s.general.compact,
            Self::CreateDir => s.storage.create_directory = !s.storage.create_directory,
            Self::Summary => s.storage.show_summary = !s.storage.show_summary,
            Self::Theme => {
                let themes = [
                    Theme::Dark,
                    Theme::Light,
                    Theme::HighContrast,
                    Theme::Monochrome,
                    Theme::Custom,
                ];
                let i = themes
                    .iter()
                    .position(|t| *t == s.appearance.theme)
                    .unwrap_or(0);
                s.appearance.theme = themes[index(i, delta, 5)];
            }
            Self::Raw
            | Self::Filtered
            | Self::Channel1
            | Self::Channel2
            | Self::Theta
            | Self::Alpha
            | Self::Beta => {
                s.appearance.customize();
                let value = match self {
                    Self::Raw => &mut s.appearance.raw,
                    Self::Filtered => &mut s.appearance.filtered,
                    Self::Channel1 => &mut s.appearance.channels[0],
                    Self::Channel2 => &mut s.appearance.channels[1],
                    Self::Theta => &mut s.appearance.bands[0],
                    Self::Alpha => &mut s.appearance.bands[1],
                    _ => &mut s.appearance.bands[2],
                };
                color(value, delta);
            }
            Self::Legends => s.appearance.legends = !s.appearance.legends,
            Self::Borders => s.appearance.borders = !s.appearance.borders,
            Self::WaveMode => {
                let modes = [WaveMode::Raw, WaveMode::Filtered, WaveMode::Both];
                let i = modes
                    .iter()
                    .position(|m| *m == s.waveform.mode)
                    .unwrap_or(0);
                s.waveform.mode = modes[index(i, delta, 3)];
            }
            Self::WaveStyle => {
                let styles = [WaveStyle::Line, WaveStyle::Points, WaveStyle::Envelope];
                let i = styles
                    .iter()
                    .position(|v| *v == s.waveform.style)
                    .unwrap_or(0);
                s.waveform.style = styles[index(i, delta, 3)];
                s.waveform.auto_envelope_threshold = 0.;
            }
            Self::RawBrightness => {
                s.waveform.raw_brightness =
                    (s.waveform.raw_brightness + delta as f64 * 0.1).clamp(0.1, 1.)
            }
            Self::FilteredBrightness => {
                s.waveform.filtered_brightness =
                    (s.waveform.filtered_brightness + delta as f64 * 0.1).clamp(0.1, 1.)
            }
            Self::EnvelopeThreshold => {
                s.waveform.auto_envelope_threshold =
                    (s.waveform.auto_envelope_threshold + delta as f64).clamp(0., 1000.)
            }
            Self::GapTolerance => {
                s.waveform.gap_tolerance =
                    (s.waveform.gap_tolerance + delta as f64 * 0.5).clamp(1., 100.)
            }
            Self::Window => s.waveform.seconds = (s.waveform.seconds + delta as f64).clamp(1., 10.),
            Self::AutoScale => s.waveform.auto_scale = !s.waveform.auto_scale,
            Self::FixedScale => {
                s.waveform.fixed_max = (s.waveform.fixed_max + delta as f64 * 10.).clamp(0.001, 1e9)
            }
            Self::Channels => s.waveform.channels = index(s.waveform.channels, delta, 3),
            Self::LogScale => s.spectral.log_scale = !s.spectral.log_scale,
            Self::Power => {
                let modes = [
                    PowerMode::Absolute,
                    PowerMode::Relative,
                    PowerMode::Baseline,
                ];
                let i = modes
                    .iter()
                    .position(|m| *m == s.spectral.power)
                    .unwrap_or(0);
                s.spectral.power = modes[index(i, delta, 3)];
            }
            Self::Smoothed => s.spectral.smoothed = !s.spectral.smoothed,
            Self::History => {
                s.spectral.history_seconds =
                    (s.spectral.history_seconds + delta as f64 * 10.).clamp(1., 3600.)
            }
            _ => {}
        }
    }
}
