use super::{EditTarget, Overlay, View};
use crate::{
    settings::{Palette, PowerMode, WaveMode},
    settings_ui::{FIELDS, GROUPS},
};
use eeg_core::*;
use ratatui::{
    prelude::*,
    symbols,
    widgets::{Axis, Block, Chart, Clear, Dataset, GraphType, Paragraph, Tabs, Wrap},
};

fn panel<'a>(title: impl Into<Line<'a>>, v: &View) -> Block<'a> {
    let p = v.settings.appearance.palette();
    let b = if v.settings.appearance.borders {
        Block::bordered()
    } else {
        Block::default()
    };
    b.title(title)
        .style(Style::default().fg(p.text).bg(p.background))
        .border_style(Style::default().fg(p.muted))
}
fn para(f: &mut Frame, area: Rect, title: &str, text: String, v: &View) {
    f.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .block(panel(title, v)),
        area,
    );
}
pub(super) fn clock(ns: u64) -> String {
    let s = ns / 1_000_000_000;
    format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}
fn quality_color(q: SignalQuality, p: &Palette) -> Color {
    match q {
        SignalQuality::Good => p.good,
        SignalQuality::Acceptable => p.warning,
        SignalQuality::Poor | SignalQuality::Disconnected => p.error,
        SignalQuality::Unknown => p.muted,
    }
}
fn quality(v: &View, s: &RuntimeStatus, i: usize) -> SignalQuality {
    if s.finished || s.connection == "Disconnected" {
        SignalQuality::Disconnected
    } else {
        v.latest
            .as_ref()
            .and_then(|b| b.channel_quality.get(i))
            .copied()
            .unwrap_or(SignalQuality::Unknown)
    }
}
fn units(config: &Config) -> &str {
    if config.source.mode == "synthetic" {
        "a.u."
    } else {
        config.source.units.as_deref().unwrap_or("a.u.")
    }
}
pub(super) fn summary(s: &RecordingSummary) -> String {
    format!(
        "Duration: {} (source time)\nSamples: {}\nLost: {}\nErrors: {}\nPath: {}",
        clock(s.duration_ns),
        s.samples,
        s.lost,
        s.errors,
        s.path
    )
}
fn visible(v: &View, name: &str) -> bool {
    v.settings.spectral.visible_bands.is_empty()
        || v.settings.spectral.visible_bands.iter().any(|b| b == name)
}

pub(super) fn draw(
    f: &mut Frame,
    config: &Config,
    ports: &[String],
    v: &View,
    status: &RuntimeStatus,
    dropped: u64,
) {
    let p = v.settings.appearance.palette();
    let area = f.area();
    f.render_widget(
        Block::default().style(Style::default().fg(p.text).bg(p.background)),
        area,
    );
    if area.width < 52 || area.height < 14 {
        f.render_widget(
            Paragraph::new("LEMON\nTerminal too small. Resize to at least 52 x 14 (recommended 120 x 35).\nQ: exit | recording continues")
                .style(Style::default().fg(p.text).bg(p.background))
                .wrap(Wrap{trim:false}),
            area,
        );
        if v.overlay.is_some() {
            overlay(f, v, status, config, &p);
        }
        ascii(f, v);
        return;
    }
    let compact = v.settings.general.compact || area.width < 110 || area.height < 30;
    let parts = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(if compact { 2 } else { 3 }),
    ])
    .split(area);
    let mode = match config.source.mode.as_str() {
        "synthetic" => "DEMO",
        "serial-test" => "LIVE / TEST",
        "replay" => "REPLAY",
        _ => "SOURCE",
    };
    let elapsed = status
        .recording_started_ns
        .map_or(0, |t| status.now_ns.saturating_sub(t));
    let rec = if status.recording_active {
        format!(
            "{} REC {}",
            if v.settings.general.unicode {
                "●"
            } else {
                "*"
            },
            clock(elapsed)
        )
    } else {
        "IDLE".into()
    };
    let header = Line::from(vec![
        Span::styled(" LEMON ", Style::default().fg(p.accent).bold()),
        Span::raw(format!(
            "| {mode} · {} channels · {} Hz | ",
            config.source.channels, config.source.sample_rate_hz
        )),
        Span::styled(
            &status.connection,
            Style::default().fg(if status.connection == "Connected" {
                p.good
            } else {
                p.muted
            }),
        ),
        Span::raw(" | "),
        Span::styled(
            rec,
            Style::default()
                .fg(if status.recording_active {
                    p.error
                } else {
                    p.muted
                })
                .bold(),
        ),
        Span::styled(
            if v.paused {
                " | PAUSED — DISPLAY ONLY"
            } else {
                ""
            },
            Style::default().fg(p.warning).bold(),
        ),
    ]);
    f.render_widget(Paragraph::new(header).wrap(Wrap { trim: false }), parts[0]);
    let titles = if compact {
        vec!["1 Src", "2 Wave", "3 FFT", "4 Bands", "5 Rec", "6 Settings"]
    } else {
        vec![
            "1 Sources",
            "2 Waveform",
            "3 Spectrum",
            "4 Bands",
            "5 Recording",
            "6 Settings",
        ]
    };
    f.render_widget(
        Tabs::new(titles)
            .select(v.page)
            .highlight_style(Style::default().fg(p.accent).bold()),
        parts[1],
    );
    match v.page {
        0 => sources(f, parts[2], v, status, config, ports, dropped),
        1 => waveform(f, parts[2], v, status, config, &p),
        2 => spectrum(f, parts[2], v, config, &p),
        3 => bands(f, parts[2], v, status, config, &p, compact),
        4 => recording(f, parts[2], v, status),
        _ => settings(f, parts[2], v, config, &p),
    }
    let commands = if v.page == 5 {
        "Tab Screen  Up/Down Select  Enter Edit  Left/Right Change  Space Toggle  S Save  R Reset group  ? Help  Q Quit"
    } else {
        "Tab Screen  R Record  M Marker  Space Pause  ? Help  Q Quit"
    };
    let context = match v.page {
        0 => "D Disconnect",
        1 => "V Raw/Filtered  C Channel  A Auto/fixed scale  +/- Window",
        2 => "C Channel  Settings: linear/log PSD",
        3 => "P Absolute/Relative/Baseline  C Channel",
        4 => "Up/Down Select field  Enter Edit (before recording)",
        _ => "Ctrl+R Reset all (confirmation) | Advanced is read only",
    };
    let message = status.fatal_error.as_ref().unwrap_or(&v.message);
    f.render_widget(
        Paragraph::new(format!(
            "{commands}\n{context}{}",
            if message.is_empty() {
                String::new()
            } else {
                format!(" | {message}")
            }
        ))
        .wrap(Wrap { trim: false })
        .style(Style::default().fg(if status.fatal_error.is_some() {
            p.error
        } else {
            p.muted
        })),
        parts[3],
    );
    if v.overlay.is_some() {
        overlay(f, v, status, config, &p);
    }
    ascii(f, v);
}
fn sources(
    f: &mut Frame,
    area: Rect,
    v: &View,
    s: &RuntimeStatus,
    c: &Config,
    ports: &[String],
    dropped: u64,
) {
    let skew = v
        .latest
        .as_ref()
        .and_then(|b| b.skew_ms)
        .map_or("unknown".into(), |x| format!("{x:.2} ms"));
    para(f,area,"Sources",format!("LEMON — Lightweight EEG Monitoring Of Neuroactivity\n\nMode: {}\nPorts: {}\nSelected: {:?}\nChannels: {} | nominal: {} Hz | arrival: {:.1} samples/s/channel\nSamples: {} | blocks: {} | lost: {} | errors: {} | outliers: {}\nChannel skew: {}\nDisplay dropped: {} | consumer blocks: {} | consumer dropped: {}\n\n{}\n\nQuality: {:?}",c.source.mode,if ports.is_empty(){"No serial ports detected".into()}else{ports.join(", ")},c.source.ports,c.source.channels,c.source.sample_rate_hz,s.rate_hz,s.received_samples,s.processed_blocks,s.lost_samples,s.errors,s.outliers,skew,dropped,s.consumer_blocks,s.consumer_dropped,if c.source.mode=="serial-test"{"TEST protocol only. Bitronics protocol is NOT confirmed. Host timestamps do not synchronize devices."}else{"Raw recording and processing continue independently of display refresh and pause."},(0..c.source.channels).map(|i|quality(v,s,i)).collect::<Vec<_>>()),v);
}
fn waveform(f: &mut Frame, area: Rect, v: &View, status: &RuntimeStatus, c: &Config, p: &Palette) {
    let indices: Vec<_> = (0..v.history.len()).filter(|i| v.selected(*i)).collect();
    if indices.is_empty() {
        para(
            f,
            area,
            "Waveform",
            "Selected channel is unavailable. Press C to select all channels.".into(),
            v,
        );
        return;
    }
    let areas = Layout::vertical(vec![
        Constraint::Ratio(1, indices.len() as u32);
        indices.len()
    ])
    .split(area);
    for (row, &i) in indices.iter().enumerate() {
        if v.history[i].is_empty() {
            para(
                f,
                areas[row],
                &format!("ch{} | Unknown", i + 1),
                "Waiting for samples...".into(),
                v,
            );
            continue;
        }
        let end = v.history[i].iter().last().map_or(0., |v| v.0);
        let start = (end - v.settings.waveform.seconds).max(0.);
        let right = (start + v.settings.waveform.seconds).max(end);
        let raw: Vec<_> = v.history[i]
            .iter()
            .filter(|v| v.0 >= start && v.1.is_finite())
            .map(|v| (v.0, v.1))
            .collect();
        let clean: Vec<_> = v.history[i]
            .iter()
            .filter(|v| v.0 >= start)
            .filter_map(|v| v.2.filter(|y| y.is_finite()).map(|y| (v.0, y)))
            .collect();
        let max = if v.settings.waveform.auto_scale {
            raw.iter()
                .chain(&clean)
                .map(|v| v.1.abs())
                .fold(1f64, f64::max)
                * 1.1
        } else {
            v.settings.waveform.fixed_max
        };
        let marker = if v.settings.general.unicode {
            symbols::Marker::Braille
        } else {
            symbols::Marker::Dot
        };
        let mut sets = Vec::new();
        if v.settings.waveform.mode != WaveMode::Filtered {
            sets.push(
                Dataset::default()
                    .name("Raw")
                    .marker(marker)
                    .graph_type(GraphType::Scatter)
                    .style(Style::default().fg(p.raw))
                    .data(&raw),
            );
        }
        if v.settings.waveform.mode != WaveMode::Raw {
            sets.push(
                Dataset::default()
                    .name("Filtered")
                    .marker(marker)
                    .graph_type(GraphType::Scatter)
                    .style(Style::default().fg(p.filtered))
                    .data(&clean),
            );
        }
        let q = quality(v, status, i);
        let title = Line::from(vec![
            Span::styled(
                format!("ch{} ", i + 1),
                Style::default().fg(p.channels[i % 2]).bold(),
            ),
            Span::styled(format!("{q:?}"), Style::default().fg(quality_color(q, p))),
            Span::raw(format!(
                " | {} s | {} | {}",
                v.settings.waveform.seconds,
                units(c),
                if v.settings.waveform.auto_scale {
                    "Auto"
                } else {
                    "Fixed"
                }
            )),
        ]);
        let mut chart = Chart::new(sets)
            .style(Style::default().fg(p.text).bg(p.background))
            .hidden_legend_constraints((Constraint::Percentage(50), Constraint::Percentage(80)))
            .block(panel(title, v))
            .x_axis(
                Axis::default()
                    .title("Time, s")
                    .bounds([start, right])
                    .labels([format!("{start:.1}"), format!("{right:.1}")]),
            )
            .y_axis(Axis::default().title(units(c)).bounds([-max, max]).labels([
                format!("{:.1}", -max),
                "0".into(),
                format!("{max:.1}"),
            ]));
        if !v.settings.appearance.legends {
            chart = chart.legend_position(None);
        }
        f.render_widget(chart, areas[row]);
    }
}
fn spectrum(f: &mut Frame, area: Rect, v: &View, c: &Config, p: &Palette) {
    let Some(s) = &v.spectrum else {
        para(f,area,"Spectrum",format!("Accumulating a contiguous FFT window: {} samples ({:.2} s).\nAfter gaps, old spectra are cleared.\nRaw and Filtered PSD will appear here.",c.processing.window_samples,c.processing.window_samples as f64/c.source.sample_rate_hz),v);
        return;
    };
    let indices: Vec<_> = (0..s.channels.len()).filter(|i| v.selected(*i)).collect();
    if indices.is_empty() {
        para(
            f,
            area,
            "Spectrum",
            "Selected channel unavailable. Press C.".into(),
            v,
        );
        return;
    }
    let areas = Layout::vertical(vec![
        Constraint::Ratio(1, indices.len() as u32);
        indices.len()
    ])
    .split(area);
    let convert = |y: f64| {
        if v.settings.spectral.log_scale {
            10. * y.max(1e-12).log10()
        } else {
            y
        }
    };
    for (row, &i) in indices.iter().enumerate() {
        let ch = &s.channels[i];
        let raw: Vec<_> = s
            .frequencies
            .iter()
            .copied()
            .zip(ch.raw_psd.iter().map(|v| convert(*v)))
            .collect();
        let clean: Vec<_> = s
            .frequencies
            .iter()
            .copied()
            .zip(ch.filtered_psd.iter().map(|v| convert(*v)))
            .collect();
        let peak = ch
            .filtered_psd
            .iter()
            .enumerate()
            .skip(1)
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map_or(0., |(i, _)| s.frequencies[i]);
        let min = if v.settings.spectral.log_scale {
            -120.
        } else {
            0.
        };
        let max = raw
            .iter()
            .chain(&clean)
            .map(|v| v.1)
            .fold(min + 1., f64::max);
        let max = if v.settings.spectral.log_scale {
            max + 3.
        } else {
            max * 1.1
        };
        let band_points: Vec<Vec<_>> = c
            .processing
            .bands
            .iter()
            .map(|b| {
                if visible(v, &b.name) {
                    clean
                        .iter()
                        .copied()
                        .filter(|(hz, _)| *hz >= b.low_hz && *hz < b.high_hz)
                        .collect()
                } else {
                    Vec::new()
                }
            })
            .collect();
        let mut sets = vec![
            Dataset::default()
                .name("Raw PSD")
                .data(&raw)
                .graph_type(GraphType::Line)
                .style(Style::default().fg(p.raw)),
            Dataset::default()
                .name("Filtered PSD")
                .data(&clean)
                .graph_type(GraphType::Line)
                .style(Style::default().fg(p.filtered)),
        ];
        for (j, points) in band_points.iter().enumerate() {
            if !points.is_empty() {
                sets.push(
                    Dataset::default()
                        .name(c.processing.bands[j].name.as_str())
                        .data(points)
                        .graph_type(GraphType::Line)
                        .style(Style::default().fg(p.bands[j % 3])),
                );
            }
        }
        let nyquist = c.source.sample_rate_hz / 2.;
        let unit = if v.settings.spectral.log_scale {
            format!("dB re 1 {}^2/Hz", units(c))
        } else {
            format!("{}^2/Hz", units(c))
        };
        let mut chart = Chart::new(sets)
            .style(Style::default().fg(p.text).bg(p.background))
            .hidden_legend_constraints((Constraint::Percentage(70), Constraint::Percentage(90)))
            .block(panel(
                format!(
                    "{} | peak {:.1} Hz | raw 48-52 Hz: {:.1}%",
                    ch.channel,
                    peak,
                    ch.mains_ratio * 100.
                ),
                v,
            ))
            .x_axis(
                Axis::default()
                    .title("Frequency, Hz")
                    .bounds([0., nyquist])
                    .labels([
                        "0".into(),
                        format!("{:.1}", nyquist / 2.),
                        format!("{nyquist:.1}"),
                    ]),
            )
            .y_axis(
                Axis::default()
                    .title(unit)
                    .bounds([min, max])
                    .labels([format!("{min:.1}"), format!("{max:.1}")]),
            );
        if !v.settings.appearance.legends {
            chart = chart.legend_position(None);
        }
        f.render_widget(chart, areas[row]);
    }
}
fn bands(
    f: &mut Frame,
    area: Rect,
    v: &View,
    status: &RuntimeStatus,
    c: &Config,
    p: &Palette,
    compact: bool,
) {
    let parts = Layout::vertical([
        Constraint::Min(4),
        Constraint::Length(if compact { 3 } else { 7 }),
    ])
    .split(area);
    let indices: Vec<_> = (0..c.source.channels).filter(|i| v.selected(*i)).collect();
    if indices.is_empty() {
        para(
            f,
            area,
            "Bands",
            "Selected channel unavailable. Press C.".into(),
            v,
        );
        return;
    }
    let areas = Layout::vertical(vec![
        Constraint::Ratio(1, indices.len() as u32);
        indices.len()
    ])
    .split(parts[0]);
    let mode = v.settings.spectral.power;
    let unit = match mode {
        PowerMode::Absolute => format!("{}^2", units(c)),
        PowerMode::Relative => "% total".into(),
        PowerMode::Baseline => "% baseline change".into(),
    };
    let end = v.band_history.latest_time().unwrap_or(0.);
    let start = (end - v.settings.spectral.history_seconds).max(0.);
    let right = end.max(start + 1.);
    for (row, &ch) in indices.iter().enumerate() {
        let id = v
            .latest
            .as_ref()
            .and_then(|b| b.raw_channels.get(ch))
            .map(|c| c.id.clone())
            .unwrap_or_else(|| format!("ch{}", ch + 1));
        let mut lines = Vec::new();
        for (i, b) in c
            .processing
            .bands
            .iter()
            .enumerate()
            .filter(|(_, b)| visible(v, &b.name))
        {
            for line in v
                .band_history
                .series(&id, &b.name, mode, v.settings.spectral.smoothed)
            {
                lines.push((i, b.name.clone(), line));
            }
        }
        if lines.is_empty() {
            para(
                f,
                areas[row],
                &format!("{} | {:?}", id, quality(v, status, ch)),
                if mode == PowerMode::Baseline {
                    "No baseline values. Configure baseline in experiment JSON; restart to apply."
                        .into()
                } else {
                    "Waiting for band power history. A contiguous FFT window is required.".into()
                },
                v,
            );
            continue;
        }
        let low = lines
            .iter()
            .flat_map(|(_, _, line)| line.iter().map(|p| p.1))
            .fold(0f64, f64::min);
        let high = lines
            .iter()
            .flat_map(|(_, _, line)| line.iter().map(|p| p.1))
            .fold(0.01f64, f64::max);
        let bounds = [if low < 0. { low * 1.1 } else { 0. }, high * 1.1];
        let mut named = Vec::new();
        let datasets: Vec<_> = lines
            .iter()
            .map(|(i, name, line)| {
                let first = !named.contains(i);
                named.push(*i);
                let d = Dataset::default()
                    .data(line)
                    .marker(symbols::Marker::Dot)
                    .graph_type(if line.len() > 1 {
                        GraphType::Line
                    } else {
                        GraphType::Scatter
                    })
                    .style(Style::default().fg(p.bands[*i % 3]));
                if first {
                    d.name(name.as_str())
                } else {
                    d
                }
            })
            .collect();
        let mut chart = Chart::new(datasets)
            .style(Style::default().fg(p.text).bg(p.background))
            .hidden_legend_constraints((Constraint::Percentage(65), Constraint::Percentage(90)))
            .block(panel(
                format!(
                    "{} | {:?} | {:?} history{}",
                    id,
                    quality(v, status, ch),
                    mode,
                    if v.settings.spectral.smoothed && mode == PowerMode::Absolute {
                        " (EMA)"
                    } else {
                        ""
                    }
                ),
                v,
            ))
            .x_axis(
                Axis::default()
                    .title("Time, s")
                    .bounds([start, right])
                    .labels([format!("{start:.1}"), format!("{right:.1}")]),
            )
            .y_axis(
                Axis::default()
                    .title(unit.clone())
                    .bounds(bounds)
                    .labels([format!("{:.1}", bounds[0]), format!("{:.1}", bounds[1])]),
            );
        if !v.settings.appearance.legends {
            chart = chart.legend_position(None);
        }
        f.render_widget(chart, areas[row]);
    }
    let mut table =
        String::from("Channel / Band       Absolute   Relative   EMA      Baseline    Asymmetry\n");
    for b in &v.bands {
        if visible(v, &b.band) {
            table.push_str(&format!(
                "{} {:12} {:8.2} {:7.1}% {:8.2} {:>10} {:>9}\n",
                b.channel,
                b.band,
                b.absolute,
                b.relative * 100.,
                b.smoothed,
                b.baseline_change_pct
                    .map_or("--".into(), |v| format!("{v:+.1}%")),
                b.asymmetry.map_or("--".into(), |v| format!("{v:+.3}"))
            ));
        }
    }
    if v.bands.is_empty() {
        table.push_str("No current window. History is kept with gaps, never filled with zero.\n");
    }
    f.render_widget(Paragraph::new(table), parts[1]);
}
fn recording(f: &mut Frame, area: Rect, v: &View, s: &RuntimeStatus) {
    let names = [
        "Session name",
        "User ID",
        "Electrode 1",
        "Electrode 2",
        "Note",
        "Destination",
    ];
    let mut text = String::new();
    for (i, name) in names.iter().enumerate() {
        text.push_str(&format!(
            "{} {name}: {}\n",
            if i == v.session_selection { ">" } else { " " },
            v.edit_value(i)
        ));
    }
    let elapsed = s
        .recording_started_ns
        .map_or(0, |t| s.now_ns.saturating_sub(t));
    text.push_str(&format!("\n{} | {} (source time)\nRecorded channel-samples: {} | stream lost: {} | stream errors: {}\nFile: {}\n{}\n\nRecent markers and events:\n",if s.recording_active{"REC - session fields locked"}else{"IDLE"},clock(elapsed),s.recording_samples,s.lost_samples,s.errors,s.recording.as_deref().unwrap_or("No recording yet"),v.message));
    for event in s.events.iter().filter(|e| {
        matches!(
            e.kind.as_str(),
            "Marker" | "RecordingStarted" | "RecordingStopped" | "RecordingError" | "SourceError"
        )
    }) {
        text.push_str(&format!(
            "{:.3}s {}: {}\n",
            event.timestamp_ns as f64 / 1e9,
            event.kind,
            event.text
        ));
    }
    if !s.recording_active {
        if let Some(summary) = &s.recording_summary {
            text.push_str(&format!("\nLast recording\n{}", self::summary(summary)));
        }
    }
    para(f, area, "Recording", text, v);
}
fn settings(f: &mut Frame, area: Rect, v: &View, c: &Config, p: &Palette) {
    let field = FIELDS[v.selection];
    let title = format!(
        "Settings{} | {}",
        if v.dirty { " * unsaved" } else { "" },
        GROUPS[field.group()]
    );
    let frame = panel(title, v);
    let inner = frame.inner(area);
    f.render_widget(frame, area);
    let parts = Layout::vertical([Constraint::Min(1), Constraint::Length(3)]).split(inner);
    let rows = parts[0].height as usize;
    let start = v
        .selection
        .saturating_sub(rows.saturating_sub(1) / 2)
        .min(FIELDS.len().saturating_sub(rows));
    let lines: Vec<Line> = FIELDS
        .iter()
        .enumerate()
        .skip(start)
        .take(rows)
        .map(|(i, field)| {
            let value = field.value(&v.settings, c);
            Line::from(format!(
                "{} {:34} {}",
                if i == v.selection { ">" } else { " " },
                field.name(),
                if value.is_empty() {
                    "(inherit / all)".into()
                } else {
                    value
                }
            ))
            .style(if i == v.selection {
                Style::default().fg(p.accent).bold()
            } else {
                Style::default().fg(if field.group() == 5 { p.muted } else { p.text })
            })
        })
        .collect();
    f.render_widget(Paragraph::new(lines), parts[0]);
    f.render_widget(Paragraph::new(format!("{}\n{}\n{}",field.hint(),v.settings_path.display(),if v.directory_locked{"Directory overridden by CLI/env; Settings default applies on a later launch without that override."}else{"Theme and history are display-only. DSP requires a restart."})).wrap(Wrap{trim:false}),parts[1]);
}
fn overlay(f: &mut Frame, v: &View, status: &RuntimeStatus, c: &Config, p: &Palette) {
    let Some(overlay) = &v.overlay else {
        return;
    };
    let area = f.area();
    let width = area.width.saturating_sub(4).min(104);
    let height = area.height.saturating_sub(2).min(match overlay {
        Overlay::Help => 25,
        Overlay::Summary(_) => 12,
        _ => 9,
    });
    let rect = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    let (title,text)=match overlay {
        Overlay::Help=>("LEMON Help",format!("Lightweight EEG Monitoring Of Neuroactivity\n\nTab / 1-6: screen | R: record | M: marker | Space: pause display | Q: quit\nSettings: Up/Down select; Enter edit; Left/Right change; S save; R reset group; Ctrl+R reset all.\nCurrent screen: {}\nWaveform: V raw/filtered/both; C channel; A auto/fixed; +/- seconds.\nBands: P absolute/relative/baseline; C channel. Recording: Up/Down, Enter edit.\nSources: D disconnect.\n\nRaw: original samples. Filtered: causal high-pass, low-pass and optional notch.\nTheta, Alpha/Mu and Beta are frequency-component power estimates.\nAlpha and Mu overlap (usually 8-13 Hz); electrode placement and experimental context distinguish them. 8-13 Hz alone is NOT MuERD.\n\nGood / Acceptable / Poor / Disconnected / Unknown: heuristic quality; not a validated clinical metric.\nPause affects display only. Raw recording continues. Gaps and absent baseline are never zeros.\nThis application is NOT a medical device. Bitronics protocol is NOT confirmed.\n\nEsc closes Help; it does not exit LEMON.",["Sources","Waveform","Spectrum","Bands","Recording","Settings"][v.page])),
        Overlay::Quit=>("Confirm exit",if status.recording_active || v.record_requested{"Запись активна. Остановить запись и выйти?\n[Y] Да  [N] Вернуться\n\nQueued samples will be flushed before exit.".into()}else{"Exit LEMON?\n[Y] Yes  [N] Return".into()}),
        Overlay::Reset=>("Reset all settings","Reset every preference to defaults?\n[Y] Reset  [N] Return\nThis does not change experiment/DSP configuration. Save with S to persist.".into()),
        Overlay::Summary(s)=>("Recording complete",format!("{}\n\nEnter / Esc: close",summary(s))),
        Overlay::Edit(target,text)=>("Edit",format!("{}\n\n{}\n\nEnter: apply | Esc: cancel\n{}",match target{EditTarget::Setting(field)=>field.name(),EditTarget::Session(_)=>"Session field (next recording)",EditTarget::Marker=>"Marker"},text,v.message)),
    };
    let _ = c;
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(p.text).bg(p.background))
            .block(
                Block::bordered()
                    .title(title)
                    .border_style(Style::default().fg(p.accent)),
            ),
        rect,
    );
}
fn ascii(f: &mut Frame, v: &View) {
    if v.settings.general.unicode {
        return;
    }
    for cell in &mut f.buffer_mut().content {
        let text = cell.symbol();
        let replacement = match text {
            "│" | "┃" => Some("|"),
            "─" | "━" => Some("-"),
            "┌" | "┐" | "└" | "┘" | "┼" | "┤" | "├" | "┬" | "┴" => Some("+"),
            "●" | "•" | "·" => Some("*"),
            "—" | "–" => Some("-"),
            _ => None,
        };
        if let Some(r) = replacement {
            cell.set_symbol(r);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        settings::{Settings, Theme},
        UiOptions,
    };
    use ratatui::{backend::TestBackend, Terminal};
    use std::{
        path::PathBuf,
        sync::{atomic::AtomicBool, Arc},
    };
    #[test]
    fn screens_and_overlay_render_at_small_sizes() {
        let c = Config::default();
        let mut v = View::new(
            &c,
            UiOptions {
                settings: Settings::default(),
                settings_path: PathBuf::from("test"),
                interrupt: Arc::new(AtomicBool::new(false)),
                data_directory_locked: false,
                experiment_directory: c.recording_dir.clone(),
                experiment_session_name: c.session.name.clone(),
            },
        );
        for (w, h) in [(160, 42), (80, 24), (40, 10)] {
            let mut terminal = Terminal::new(TestBackend::new(w, h)).expect("terminal");
            for page in 0..6 {
                v.page = page;
                terminal
                    .draw(|f| draw(f, &c, &[], &v, &RuntimeStatus::default(), 0))
                    .expect("draw");
            }
            v.overlay = Some(Overlay::Help);
            terminal
                .draw(|f| draw(f, &c, &[], &v, &RuntimeStatus::default(), 0))
                .expect("help");
            v.overlay = None;
        }
        v.settings.appearance.theme = Theme::Light;
        let mut terminal = Terminal::new(TestBackend::new(120, 35)).expect("terminal");
        terminal
            .draw(|f| draw(f, &c, &[], &v, &RuntimeStatus::default(), 0))
            .expect("theme");
        assert_eq!(terminal.backend().buffer()[(0, 0)].bg, Color::White);
    }
}
