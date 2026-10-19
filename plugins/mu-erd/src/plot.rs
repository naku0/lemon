//! Static raster plotting, using the existing condition-plugin PNG/font primitives.
use super::{compute::Aggregate, config::Config};
use std::{collections::BTreeSet, fs, path::Path};
struct Image {
    width: usize,
    height: usize,
    pixels: Vec<u8>,
}
impl Image {
    fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            pixels: vec![248; width * height * 3],
        }
    }
    fn pixel(&mut self, x: i32, y: i32, color: [u8; 3]) {
        if x >= 0 && y >= 0 && (x as usize) < self.width && (y as usize) < self.height {
            let i = (y as usize * self.width + x as usize) * 3;
            self.pixels[i..i + 3].copy_from_slice(&color);
        }
    }
    fn rect(&mut self, x: i32, y: i32, w: i32, h: i32, color: [u8; 3]) {
        for yy in y..y + h {
            for xx in x..x + w {
                self.pixel(xx, yy, color);
            }
        }
    }
    fn line(&mut self, mut x0: i32, mut y0: i32, x1: i32, y1: i32, color: [u8; 3]) {
        let dx = (x1 - x0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let dy = -(y1 - y0).abs();
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut error = dx + dy;
        loop {
            self.rect(x0 - 1, y0 - 1, 3, 3, color);
            if x0 == x1 && y0 == y1 {
                break;
            }
            let e = 2 * error;
            if e >= dy {
                error += dy;
                x0 += sx;
            }
            if e <= dx {
                error += dx;
                y0 += sy;
            }
        }
    }
    fn text(&mut self, x: i32, y: i32, text: &str, color: [u8; 3], scale: i32) {
        let mut cursor = x;
        for c in text.to_ascii_uppercase().chars() {
            let bits = glyph(c);
            for (row, mask) in bits.iter().enumerate() {
                for col in 0..5 {
                    if mask & (1 << (4 - col)) != 0 {
                        self.rect(
                            cursor + col * scale,
                            y + row as i32 * scale,
                            scale,
                            scale,
                            color,
                        );
                    }
                }
            }
            cursor += 6 * scale;
        }
    }
    fn save_png(&self, path: &Path) -> Result<(), String> {
        let mut raw = Vec::with_capacity((self.width * 3 + 1) * self.height);
        for row in self.pixels.chunks(self.width * 3) {
            raw.push(0);
            raw.extend_from_slice(row);
        }
        let mut z = vec![0x78, 0x01];
        let mut offset = 0;
        while offset < raw.len() {
            let len = (raw.len() - offset).min(65535);
            let final_block = offset + len == raw.len();
            z.push(u8::from(final_block));
            z.extend_from_slice(&(len as u16).to_le_bytes());
            z.extend_from_slice(&(!(len as u16)).to_le_bytes());
            z.extend_from_slice(&raw[offset..offset + len]);
            offset += len;
        }
        z.extend_from_slice(&adler32(&raw).to_be_bytes());
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&(self.width as u32).to_be_bytes());
        ihdr.extend_from_slice(&(self.height as u32).to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
        chunk(&mut png, b"IHDR", &ihdr);
        chunk(&mut png, b"IDAT", &z);
        chunk(&mut png, b"IEND", &[]);
        fs::write(path, png).map_err(|e| e.to_string())
    }
}
fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut check = kind.to_vec();
    check.extend_from_slice(data);
    out.extend_from_slice(&crc32(&check).to_be_bytes());
}
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffff;
    for byte in data {
        crc ^= *byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                0xedb8_8320 ^ (crc >> 1)
            } else {
                crc >> 1
            };
        }
    }
    !crc
}
fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for byte in data {
        a = (a + *byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}
fn glyph(c: char) -> [u8; 7] {
    match c {
        'A' => [14, 17, 17, 31, 17, 17, 17],
        'B' => [30, 17, 17, 30, 17, 17, 30],
        'C' => [14, 17, 16, 16, 16, 17, 14],
        'D' => [30, 17, 17, 17, 17, 17, 30],
        'E' => [31, 16, 16, 30, 16, 16, 31],
        'F' => [31, 16, 16, 30, 16, 16, 16],
        'G' => [14, 17, 16, 23, 17, 17, 14],
        'H' => [17, 17, 17, 31, 17, 17, 17],
        'I' => [14, 4, 4, 4, 4, 4, 14],
        'J' => [7, 2, 2, 2, 18, 18, 12],
        'K' => [17, 18, 20, 24, 20, 18, 17],
        'L' => [16, 16, 16, 16, 16, 16, 31],
        'M' => [17, 27, 21, 21, 17, 17, 17],
        'N' => [17, 25, 21, 19, 17, 17, 17],
        'O' => [14, 17, 17, 17, 17, 17, 14],
        'P' => [30, 17, 17, 30, 16, 16, 16],
        'Q' => [14, 17, 17, 17, 21, 18, 13],
        'R' => [30, 17, 17, 30, 20, 18, 17],
        'S' => [15, 16, 16, 14, 1, 1, 30],
        'T' => [31, 4, 4, 4, 4, 4, 4],
        'U' => [17, 17, 17, 17, 17, 17, 14],
        'V' => [17, 17, 17, 17, 17, 10, 4],
        'W' => [17, 17, 17, 21, 21, 21, 10],
        'X' => [17, 17, 10, 4, 10, 17, 17],
        'Y' => [17, 17, 10, 4, 4, 4, 4],
        'Z' => [31, 1, 2, 4, 8, 16, 31],
        '0' => [14, 17, 19, 21, 25, 17, 14],
        '1' => [4, 12, 4, 4, 4, 4, 14],
        '2' => [14, 17, 1, 2, 4, 8, 31],
        '3' => [30, 1, 1, 14, 1, 1, 30],
        '4' => [2, 6, 10, 18, 31, 2, 2],
        '5' => [31, 16, 16, 30, 1, 1, 30],
        '6' => [14, 16, 16, 30, 17, 17, 14],
        '7' => [31, 1, 2, 4, 8, 8, 8],
        '8' => [14, 17, 17, 14, 17, 17, 14],
        '9' => [14, 17, 17, 15, 1, 1, 14],
        '-' => [0, 0, 0, 31, 0, 0, 0],
        '.' => [0, 0, 0, 0, 0, 12, 12],
        '/' => [1, 1, 2, 4, 8, 16, 16],
        ';' => [0, 12, 12, 0, 12, 4, 8],
        ':' => [0, 12, 12, 0, 12, 12, 0],
        '(' => [2, 4, 8, 8, 8, 4, 2],
        ')' => [8, 4, 2, 2, 2, 4, 8],
        '^' => [4, 10, 17, 0, 0, 0, 0],
        ' ' => [0; 7],
        _ => [31, 17, 2, 4, 0, 4, 0],
    }
}

const INK: [u8; 3] = [30, 40, 55];
const COLORS: [[u8; 3]; 6] = [
    [0, 114, 178],
    [0, 128, 90],
    [111, 78, 175],
    [200, 85, 0],
    [160, 50, 90],
    [0, 130, 150],
];
fn filename(s: &str) -> String {
    if !s.is_empty()
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    {
        s.into()
    } else {
        format!(
            "utf8-{}",
            s.as_bytes()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        )
    }
}
fn label(s: &str) -> String {
    s.chars()
        .flat_map(|c| {
            if c.is_ascii() {
                c.to_string().chars().collect::<Vec<_>>()
            } else {
                format!("U{:04X}", c as u32).chars().collect()
            }
        })
        .collect()
}
fn limits(rows: &[&Aggregate]) -> (f64, f64) {
    let low = rows.iter().map(|r| r.q1_percent).fold(0., f64::min);
    let high = rows.iter().map(|r| r.q3_percent).fold(0., f64::max);
    let span = (high - low).max(1.);
    (low - span * 0.1, high + span * 0.1)
}
fn y(value: f64, lo: f64, hi: f64) -> i32 {
    550 - ((value - lo) / (hi - lo) * 410.) as i32
}
fn frame(image: &mut Image, lo: f64, hi: f64) {
    image.text(
        30,
        98,
        "CHANGE RELATIVE TO TRIAL BASELINE (PERCENT)",
        INK,
        2,
    );
    for i in 0..=5 {
        let value = lo + (hi - lo) * i as f64 / 5.;
        let py = y(value, lo, hi);
        image.line(110, py, 1120, py, [215, 220, 225]);
        image.text(10, py - 3, &format!("{value:.1}"), INK, 1);
    }
    image.line(110, 140, 110, 550, INK);
    image.line(110, 550, 1120, 550, INK);
    let zero = y(0., lo, hi);
    image.line(110, zero, 1120, zero, [145, 150, 155]);
}
pub(super) fn create(
    dir: &Path,
    c: &Config,
    curves: &[Aggregate],
    comparisons: &[Aggregate],
    versions: &str,
) -> Result<Vec<String>, String> {
    let mut made = Vec::new();
    if curves.is_empty() && comparisons.is_empty() {
        return Ok(made);
    }
    fs::create_dir(dir.join("figures")).map_err(|e| e.to_string())?;
    for (band, range, base_color) in [
        ("Mu", c.bands.mu_hz, [0, 110, 160]),
        ("Beta", c.bands.beta_hz, [205, 100, 0]),
    ] {
        for channel in &c.channels {
            let rows: Vec<_> = curves
                .iter()
                .filter(|r| r.band == band && r.channel == *channel)
                .collect();
            if rows.is_empty() {
                continue;
            }
            let conditions: Vec<_> = rows
                .iter()
                .map(|r| r.condition.as_str())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            let mut image = Image::new(1200, 750 + conditions.len() * 35);
            image.text(
                30,
                20,
                &format!(
                    "{band} {:.1}-{:.1} HZ / {}",
                    range[0],
                    range[1],
                    label(channel)
                ),
                base_color,
                2,
            );
            image.text(
                30,
                50,
                "MEDIAN AND IQR ACROSS TRIALS; N LABELLED AT EACH POINT",
                INK,
                2,
            );
            image.text(30, 75, versions, INK, 1);
            let (lo, hi) = limits(&rows);
            let x = |t: f64| {
                110 + ((t - c.epoch_seconds[0]) / (c.epoch_seconds[1] - c.epoch_seconds[0]) * 1010.)
                    as i32
            };
            for (r, color) in [
                (c.baseline_seconds, [224, 236, 246]),
                (c.analysis_seconds, [246, 236, 215]),
            ] {
                image.rect(x(r[0]), 140, x(r[1]) - x(r[0]), 410, color);
            }
            frame(&mut image, lo, hi);
            image.line(x(0.), 140, x(0.), 550, [190, 50, 50]);
            image.text(x(0.) - 20, 120, "0 TASKSTART", [170, 40, 40], 1);
            for i in 0..=6 {
                let t =
                    c.epoch_seconds[0] + (c.epoch_seconds[1] - c.epoch_seconds[0]) * i as f64 / 6.;
                image.text(x(t) - 10, 568, &format!("{t:.1}"), INK, 1);
            }
            image.text(
                240,
                595,
                "WINDOW START RELATIVE TO TASKSTART (SECONDS)",
                INK,
                2,
            );
            image.text(
                30,
                625,
                "BLUE BACKGROUND: BASELINE / SAND BACKGROUND: ANALYSIS",
                INK,
                1,
            );
            for (i, condition) in conditions.iter().enumerate() {
                let color = COLORS[(i + if band == "Beta" { 3 } else { 0 }) % COLORS.len()];
                let mut points: Vec<_> = rows
                    .iter()
                    .copied()
                    .filter(|r| r.condition == *condition)
                    .collect();
                points.sort_by(|a, b| {
                    a.relative_time_seconds
                        .unwrap_or(0.)
                        .total_cmp(&b.relative_time_seconds.unwrap_or(0.))
                });
                let mut previous: Option<(f64, i32, i32)> = None;
                for r in points {
                    let t = r.relative_time_seconds.unwrap_or(0.);
                    let px = x(t);
                    let py = y(r.median_percent, lo, hi);
                    if let Some((pt, px0, py0)) = previous {
                        if (t - pt - c.hop_seconds).abs() < 1e-9 {
                            image.line(px0, py0, px, py, color);
                        }
                    }
                    image.line(
                        px,
                        y(r.q1_percent, lo, hi),
                        px,
                        y(r.q3_percent, lo, hi),
                        color,
                    );
                    image.line(
                        px - 4,
                        y(r.q1_percent, lo, hi),
                        px + 4,
                        y(r.q1_percent, lo, hi),
                        color,
                    );
                    image.line(
                        px - 4,
                        y(r.q3_percent, lo, hi),
                        px + 4,
                        y(r.q3_percent, lo, hi),
                        color,
                    );
                    image.rect(px - 3, py - 3, 7, 7, color);
                    image.text(px + 6, py - 12, &format!("N{}", r.trial_count), color, 1);
                    previous = Some((t, px, py));
                }
                image.rect(30, 655 + i as i32 * 35, 15, 12, color);
                image.text(55, 655 + i as i32 * 35, &label(condition), INK, 1);
            }
            let name = format!(
                "figures/{}-timecourse-{}.png",
                band.to_lowercase(),
                filename(channel)
            );
            image.save_png(&dir.join(&name))?;
            made.push(name);
        }
        let rows: Vec<_> = comparisons.iter().filter(|r| r.band == band).collect();
        if rows.is_empty() {
            continue;
        }
        let width = (rows.len() * 130 + 180).max(1200);
        if width > 20000 {
            return Err("too many summary groups to plot".into());
        }
        let mut image = Image::new(width, 800 + rows.len() * 20);
        image.text(
            30,
            20,
            &format!(
                "{band} {:.1}-{:.1} HZ CONDITION SUMMARY",
                range[0], range[1]
            ),
            base_color,
            2,
        );
        image.text(30, 50, "MEDIAN AND IQR OF PER-TRIAL CHANGES", INK, 2);
        image.text(30, 75, versions, INK, 1);
        let (lo, hi) = limits(&rows);
        frame(&mut image, lo, hi);
        for (i, row) in rows.iter().enumerate() {
            let px = 150 + i as i32 * ((width - 200) / rows.len()) as i32;
            image.line(
                px,
                y(row.q1_percent, lo, hi),
                px,
                y(row.q3_percent, lo, hi),
                base_color,
            );
            image.line(
                px - 8,
                y(row.q1_percent, lo, hi),
                px + 8,
                y(row.q1_percent, lo, hi),
                base_color,
            );
            image.line(
                px - 8,
                y(row.q3_percent, lo, hi),
                px + 8,
                y(row.q3_percent, lo, hi),
                base_color,
            );
            image.rect(
                px - 5,
                y(row.median_percent, lo, hi) - 5,
                11,
                11,
                base_color,
            );
            image.text(
                px - 20,
                570,
                &format!("GROUP {} N{}", i + 1, row.trial_count),
                INK,
                1,
            );
            image.text(
                30,
                650 + i as i32 * 20,
                &format!(
                    "GROUP {}: {} / {} / N{}",
                    i + 1,
                    label(&row.condition),
                    label(&row.channel),
                    row.trial_count
                ),
                INK,
                1,
            );
        }
        image.text(
            30,
            605,
            "CONDITION / CHANNEL GROUPS; NO SIGNIFICANCE TESTS",
            INK,
            2,
        );
        let name = format!("figures/{}-condition-summary.png", band.to_lowercase());
        image.save_png(&dir.join(&name))?;
        made.push(name);
    }
    Ok(made)
}
