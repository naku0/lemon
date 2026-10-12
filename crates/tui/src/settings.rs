//! Presentation preferences. Never serialized into experiment metadata.
use core_types::Result;
use ratatui::style::Color;
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Theme {
    #[default]
    Dark,
    Light,
    HighContrast,
    Monochrome,
    Custom,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum WaveMode {
    Raw,
    Filtered,
    #[default]
    Both,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PowerMode {
    #[default]
    Absolute,
    Relative,
    Baseline,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct General {
    pub fps: u32,
    pub start_tab: usize,
    pub confirm_exit: bool,
    pub unicode: bool,
    pub compact: bool,
}
impl Default for General {
    fn default() -> Self {
        Self {
            fps: 15,
            start_tab: 1,
            confirm_exit: false,
            unicode: true,
            compact: false,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Storage {
    pub directory: Option<String>,
    pub session_name: Option<String>,
    pub create_directory: bool,
    pub show_summary: bool,
}
impl Default for Storage {
    fn default() -> Self {
        Self {
            directory: None,
            session_name: None,
            create_directory: true,
            show_summary: true,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Appearance {
    pub theme: Theme,
    pub raw: String,
    pub filtered: String,
    pub channels: [String; 2],
    pub bands: [String; 3],
    pub legends: bool,
    pub borders: bool,
}
impl Default for Appearance {
    fn default() -> Self {
        Self {
            theme: Theme::Dark,
            raw: "gray".into(),
            filtered: "cyan".into(),
            channels: ["cyan".into(), "magenta".into()],
            bands: ["green".into(), "magenta".into(), "yellow".into()],
            legends: true,
            borders: true,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Waveform {
    pub mode: WaveMode,
    pub style: WaveStyle,
    pub raw_brightness: f64,
    pub filtered_brightness: f64,
    pub auto_envelope_threshold: f64,
    pub gap_tolerance: f64,
    pub seconds: f64,
    pub auto_scale: bool,
    pub fixed_max: f64,
    pub channels: usize,
}
impl Default for Waveform {
    fn default() -> Self {
        Self {
            mode: WaveMode::Both,
            style: WaveStyle::Line,
            raw_brightness: 0.65,
            filtered_brightness: 1.0,
            auto_envelope_threshold: 0.0,
            gap_tolerance: 2.5,
            seconds: 5.,
            auto_scale: true,
            fixed_max: 100.,
            channels: 0,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WaveStyle {
    Line,
    Points,
    Envelope,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Spectral {
    pub log_scale: bool,
    pub power: PowerMode,
    pub visible_bands: Vec<String>,
    pub smoothed: bool,
    pub history_seconds: f64,
}
impl Default for Spectral {
    fn default() -> Self {
        Self {
            log_scale: false,
            power: PowerMode::Absolute,
            visible_bands: Vec::new(),
            smoothed: false,
            history_seconds: 60.,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub general: General,
    pub storage: Storage,
    pub appearance: Appearance,
    pub waveform: Waveform,
    pub spectral: Spectral,
}
impl Settings {
    pub fn validate(&self) -> Result<()> {
        if [
            self.waveform.raw_brightness,
            self.waveform.filtered_brightness,
        ]
        .iter()
        .any(|v| !v.is_finite() || !(0.1..=1.0).contains(v))
            || !self.waveform.gap_tolerance.is_finite()
            || !(1.0..=100.).contains(&self.waveform.gap_tolerance)
            || !self.waveform.auto_envelope_threshold.is_finite()
            || !(0.0..=1000.).contains(&self.waveform.auto_envelope_threshold)
        {
            return Err("Settings: brightness 0.1..1, gap tolerance 1..100 periods, auto envelope 0..1000 samples/column (0=off)".into());
        }
        if !(1..=60).contains(&self.general.fps) || self.general.start_tab > 5 {
            return Err("Settings: FPS 1..60; start_tab 0..5".into());
        }
        if !self.waveform.seconds.is_finite()
            || !(1.0..=10.).contains(&self.waveform.seconds)
            || !self.waveform.fixed_max.is_finite()
            || !(0.001..=1e9).contains(&self.waveform.fixed_max)
            || self.waveform.channels > 2
        {
            return Err("Settings: окно 1..10 s, fixed_max 0.001..1e9, channels 0..2".into());
        }
        if !self.spectral.history_seconds.is_finite()
            || !(1.0..=3600.).contains(&self.spectral.history_seconds)
        {
            return Err("Settings: история 1..3600 s".into());
        }
        if self.spectral.visible_bands.len() > 16
            || self.spectral.visible_bands.iter().any(|v| v.len() > 64)
        {
            return Err("Settings: максимум 16 имён диапазонов по 64 байта".into());
        }
        for color in [&self.appearance.raw, &self.appearance.filtered]
            .into_iter()
            .chain(self.appearance.channels.iter())
            .chain(self.appearance.bands.iter())
        {
            parse_color(color)?;
        }
        if self
            .storage
            .directory
            .as_ref()
            .is_some_and(|p| p.trim().is_empty() || p.len() > 4096 || p.contains('\0'))
        {
            return Err("Settings: укажите непустой путь каталога".into());
        }
        if self
            .storage
            .session_name
            .as_ref()
            .is_some_and(|p| p.trim().is_empty() || p.len() > 256)
        {
            return Err("Settings: имя сеанса 1..256 байт".into());
        }
        Ok(())
    }
    pub fn load(path: &Path) -> Result<Option<Self>> {
        let file = match fs::File::open(path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("Settings {}: {e}", path.display())),
        };
        if file.metadata().map_err(|e| e.to_string())?.len() > 1_048_576 {
            return Err("Settings: файл превышает 1 MiB".into());
        }
        let settings: Self = serde_json::from_reader(file)
            .map_err(|e| format!("Settings {}: {e}", path.display()))?;
        settings.validate()?;
        Ok(Some(settings))
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent).map_err(|e| format!("Settings directory: {e}"))?;
        }
        let temp = path.with_extension("json.tmp");
        let mut file = fs::File::create(&temp).map_err(|e| format!("Settings save: {e}"))?;
        serde_json::to_writer_pretty(&mut file, self).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        fs::rename(temp, path).map_err(|e| format!("Settings replace: {e}"))
    }
    pub fn reset_group(&mut self, group: usize) {
        let d = Self::default();
        match group {
            0 => self.general = d.general,
            1 => self.storage = d.storage,
            2 => self.appearance = d.appearance,
            3 => self.waveform = d.waveform,
            4 => self.spectral = d.spectral,
            _ => {}
        }
    }
}
pub const COLORS: [&str; 9] = [
    "gray", "white", "black", "red", "green", "yellow", "blue", "magenta", "cyan",
];
pub fn parse_color(s: &str) -> Result<Color> {
    if s.len() == 7 && s.starts_with('#') && s[1..].bytes().all(|b| b.is_ascii_hexdigit()) {
        let rgb = u32::from_str_radix(&s[1..], 16).map_err(|e| e.to_string())?;
        return Ok(Color::Rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8));
    }
    match s {
        "gray" => Ok(Color::Gray),
        "white" => Ok(Color::White),
        "black" => Ok(Color::Black),
        "red" => Ok(Color::Red),
        "green" => Ok(Color::Green),
        "yellow" => Ok(Color::Yellow),
        "blue" => Ok(Color::Blue),
        "magenta" => Ok(Color::Magenta),
        "cyan" => Ok(Color::Cyan),
        _ => Err(format!(
            "Неизвестный цвет {s}; допустимы {}",
            COLORS.join(", ")
        )),
    }
}
#[derive(Clone)]
pub struct Palette {
    pub background: Color,
    pub text: Color,
    pub muted: Color,
    pub accent: Color,
    pub raw: Color,
    pub filtered: Color,
    pub channels: [Color; 2],
    pub bands: [Color; 3],
    pub good: Color,
    pub warning: Color,
    pub error: Color,
}
impl Appearance {
    pub fn palette(&self) -> Palette {
        let mut p = Palette {
            background: Color::Black,
            text: Color::White,
            muted: Color::Gray,
            accent: Color::Cyan,
            raw: Color::Gray,
            filtered: Color::Cyan,
            channels: [Color::Cyan, Color::Magenta],
            bands: [Color::Green, Color::Magenta, Color::Yellow],
            good: Color::Green,
            warning: Color::Yellow,
            error: Color::Red,
        };
        match self.theme {
            Theme::Dark => {
                // xterm-256 is supported by macOS Terminal and common SSH/PTY
                // sessions.  Avoid unconditional 24-bit escape sequences here:
                // terminals which only advertise 256 colors can render those
                // sequences as broken backgrounds or unreadable foregrounds.
                p.background = Color::Indexed(234); // #1c1c1c, not absolute black
                p.text = Color::Indexed(255); // #eeeeee
                p.muted = Color::Indexed(250); // #bcbcbc
                p.accent = Color::Indexed(220); // bright yellow
                p.raw = Color::Indexed(117); // light blue
                p.filtered = Color::Indexed(48); // bright cyan-green
                p.channels = [Color::Indexed(75), Color::Indexed(215)]; // blue / orange
                p.bands = [
                    Color::Indexed(141), // violet: Theta
                    Color::Indexed(48),  // green: Alpha/Mu
                    Color::Indexed(220), // amber: Beta
                ];
                p.good = Color::Indexed(48);
                p.warning = Color::Indexed(220);
                p.error = Color::Indexed(203); // coral/red
            }
            Theme::Light => {
                p.background = Color::White;
                p.text = Color::Black;
                p.muted = Color::DarkGray;
                p.accent = Color::Blue;
                p.raw = Color::DarkGray;
                p.filtered = Color::Blue;
                p.channels = [Color::Blue, Color::Magenta];
                p.bands = [Color::Green, Color::Magenta, Color::Blue];
            }
            Theme::HighContrast => {
                p.background = Color::Black;
                p.text = Color::White;
                p.accent = Color::Yellow;
                p.raw = Color::White;
                p.filtered = Color::Yellow;
                p.channels = [Color::Yellow, Color::Cyan];
                p.bands = [Color::Green, Color::Cyan, Color::Yellow];
            }
            Theme::Monochrome => {
                p.accent = Color::White;
                p.raw = Color::Gray;
                p.filtered = Color::White;
                p.channels = [Color::White; 2];
                p.bands = [Color::White; 3];
                p.good = Color::White;
                p.warning = Color::White;
                p.error = Color::White;
            }
            Theme::Custom => {
                p.raw = parse_color(&self.raw).unwrap_or(Color::Gray);
                p.filtered = parse_color(&self.filtered).unwrap_or(Color::Cyan);
                p.channels = self
                    .channels
                    .each_ref()
                    .map(|s| parse_color(s).unwrap_or(Color::White));
                p.bands = self
                    .bands
                    .each_ref()
                    .map(|s| parse_color(s).unwrap_or(Color::White));
            }
        }
        p
    }
    pub fn customize(&mut self) {
        if self.theme != Theme::Custom {
            let p = self.palette();
            self.raw = color_name(p.raw);
            self.filtered = color_name(p.filtered);
            self.channels = p.channels.map(color_name);
            self.bands = p.bands.map(color_name);
            self.theme = Theme::Custom;
        }
    }
}
fn color_name(c: Color) -> String {
    match c {
        Color::Rgb(r, g, b) => return format!("#{r:02x}{g:02x}{b:02x}"),
        Color::Indexed(index) => {
            let (r, g, b) = xterm_rgb(index);
            return format!("#{r:02x}{g:02x}{b:02x}");
        }
        _ => {}
    }
    COLORS
        .iter()
        .find(|s| parse_color(s).ok() == Some(c))
        .unwrap_or(&"gray")
        .to_string()
}

pub(crate) fn xterm_rgb(index: u8) -> (u8, u8, u8) {
    const ANSI: [(u8, u8, u8); 16] = [
        (0, 0, 0),
        (128, 0, 0),
        (0, 128, 0),
        (128, 128, 0),
        (0, 0, 128),
        (128, 0, 128),
        (0, 128, 128),
        (192, 192, 192),
        (128, 128, 128),
        (255, 0, 0),
        (0, 255, 0),
        (255, 255, 0),
        (0, 0, 255),
        (255, 0, 255),
        (0, 255, 255),
        (255, 255, 255),
    ];
    match index {
        0..=15 => ANSI[index as usize],
        16..=231 => {
            let n = index - 16;
            let level = |value: u8| if value == 0 { 0 } else { 55 + value * 40 };
            (level(n / 36), level((n / 6) % 6), level(n % 6))
        }
        _ => {
            let gray = 8 + (index - 232) * 10;
            (gray, gray, gray)
        }
    }
}

pub(crate) fn nearest_xterm((r, g, b): (u8, u8, u8)) -> u8 {
    (16u16..=255)
        .min_by_key(|&index| {
            let (cr, cg, cb) = xterm_rgb(index as u8);
            let dr = r as i32 - cr as i32;
            let dg = g as i32 - cg as i32;
            let db = b as i32 - cb as i32;
            dr * dr + dg * dg + db * db
        })
        .unwrap_or(255) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_and_themes() {
        let s = Settings::default();
        s.validate().expect("defaults");
        let mut a = s.appearance;
        let dark = a.palette();
        a.theme = Theme::Light;
        assert_ne!(dark.background, a.palette().background);
        a.theme = Theme::Monochrome;
        assert_eq!(a.palette().good, a.palette().error);
        a.customize();
        a.raw = "red".into();
        assert_eq!(a.palette().raw, Color::Red);
    }
    #[test]
    fn dark_colors_have_contrast_and_survive_customization() {
        let mut a = Settings::default().appearance;
        let p = a.palette();
        let luminance = |color: Color| {
            let (r, g, b) = match color {
                Color::Rgb(r, g, b) => (r, g, b),
                Color::Indexed(index) => xterm_rgb(index),
                _ => panic!("RGB or indexed palette expected"),
            };
            let linear = |v: u8| {
                let v = v as f64 / 255.;
                if v <= 0.04045 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
        };
        for color in [
            p.text,
            p.muted,
            p.raw,
            p.filtered,
            p.accent,
            p.error,
            p.warning,
            p.channels[0],
            p.channels[1],
            p.bands[0],
            p.bands[1],
            p.bands[2],
        ] {
            assert!((luminance(color) + 0.05) / (luminance(p.background) + 0.05) >= 4.5);
        }
        assert_ne!(p.raw, p.filtered);
        assert_ne!(p.channels[0], p.channels[1]);
        assert_ne!(p.bands[0], p.bands[1]);
        assert_ne!(p.bands[1], p.bands[2]);
        a.customize();
        assert_eq!(a.palette().raw, Color::Rgb(135, 215, 255));
        assert_eq!(a.palette().filtered, Color::Rgb(0, 255, 135));
        assert_eq!(a.palette().bands[0], Color::Rgb(175, 135, 255));
    }
    #[test]
    fn xterm_palette_conversion_covers_ansi_cube_and_gray() {
        assert_eq!(xterm_rgb(0), (0, 0, 0));
        assert_eq!(xterm_rgb(117), (135, 215, 255));
        assert_eq!(xterm_rgb(234), (28, 28, 28));
        assert_eq!(nearest_xterm(xterm_rgb(117)), 117);
        assert_eq!(
            parse_color(&color_name(Color::Indexed(215))),
            Ok(Color::Rgb(255, 175, 95))
        );
    }
    #[test]
    fn partial_settings_and_bad_colors() {
        let mut s: Settings = serde_json::from_str("{\"general\":{\"fps\":30}}").expect("partial");
        assert_eq!(s.general.fps, 30);
        assert_eq!(s.waveform.seconds, 5.);
        s.appearance.raw = "invalid".into();
        assert!(s.validate().is_err());
    }
    #[test]
    fn roundtrip() {
        let path = std::env::temp_dir().join(format!("lemon-settings-{}.json", std::process::id()));
        let mut s = Settings::default();
        s.appearance.theme = Theme::Light;
        s.spectral.history_seconds = 120.;
        s.save(&path).expect("save");
        assert_eq!(Settings::load(&path).expect("load"), Some(s));
        std::fs::remove_file(path).expect("cleanup");
    }
}
