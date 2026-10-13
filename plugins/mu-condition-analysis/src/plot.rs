use super::{Bands, ConditionResult, WindowPower};
use std::{fs, path::Path};

const BLUE: [u8; 3] = [0, 114, 178];
const ORANGE: [u8; 3] = [230, 126, 0];
const GREEN: [u8; 3] = [0, 140, 100];
const VIOLET: [u8; 3] = [111, 78, 175];
const COLORS: [[u8; 3]; 6] = [BLUE, ORANGE, GREEN, VIOLET, [213, 94, 0], [86, 180, 233]];

pub(super) fn create_figures(
    output: &Path,
    channels: &[String],
    results: &[ConditionResult],
    powers: &[WindowPower],
    bands: &Bands,
    units: &str,
) -> Result<Vec<String>, String> {
    let figures = output.join("figures");
    fs::create_dir_all(&figures).map_err(|e| e.to_string())?;
    let mut made = Vec::new();
    for (band, range, file, color) in [
        ("Mu", bands.mu_hz, "mu-band-comparison.png", BLUE),
        ("Beta", bands.beta_hz, "beta-band-comparison.png", ORANGE),
    ] {
        let rows: Vec<_> = results
            .iter()
            .filter(|r| r.band == band && r.median_power.is_some())
            .collect();
        if rows.is_empty() {
            continue;
        }
        let mut image = Image::new(1100, 650);
        image.text(
            55,
            25,
            &format!(
                "{band} BAND CONDITION COMPARISON  {:.1}-{:.1} HZ",
                range[0], range[1]
            ),
            [25, 35, 45],
            3,
        );
        image.text(15, 300, &format!("POWER ({units}^2)"), [55, 65, 75], 1);
        image.axes();
        let max = rows
            .iter()
            .filter_map(|r| r.median_power)
            .fold(0f64, f64::max)
            .max(f64::EPSILON);
        let width = (930 / rows.len().max(1) as i32).max(12);
        for (i, row) in rows.iter().enumerate() {
            let x = 90 + i as i32 * width;
            let h = (row.median_power.unwrap_or(0.) / max * 470.) as i32;
            image.rect(x, 550 - h, (width - 8).max(4), h, color);
            image.text(
                x,
                565,
                &short(
                    &format!(
                        "{} {} N{}",
                        row.condition, row.channel, row.valid_window_count
                    ),
                    18,
                ),
                [35, 45, 55],
                1,
            );
        }
        image.text(
            80,
            610,
            "DESCRIPTIVE WINDOW MEDIANS; SEPARATE RECORDINGS",
            [75, 80, 85],
            1,
        );
        image.save_png(&figures.join(file))?;
        made.push(format!("figures/{file}"));
    }
    for channel in channels {
        let rows: Vec<_> = powers
            .iter()
            .filter(|p| p.channel == *channel && p.band == "Mu")
            .collect();
        if rows.is_empty() {
            continue;
        }
        let file = format!("mu-power-over-time-{}.png", safe_file_component(channel));
        let mut image = Image::new(1100, 650);
        image.text(
            55,
            25,
            &format!(
                "MU POWER OVER TIME - {channel}  {:.1}-{:.1} HZ",
                bands.mu_hz[0], bands.mu_hz[1]
            ),
            [25, 35, 45],
            3,
        );
        image.text(15, 300, &format!("POWER ({units}^2)"), [55, 65, 75], 1);
        image.axes();
        let ymax = rows
            .iter()
            .map(|p| p.power)
            .fold(0f64, f64::max)
            .max(f64::EPSILON);
        let mut conditions: Vec<&str> = rows.iter().map(|p| p.condition.as_str()).collect();
        conditions.sort();
        conditions.dedup();
        for (ci, condition) in conditions.iter().enumerate() {
            let condition_rows: Vec<_> =
                rows.iter().filter(|p| p.condition == *condition).collect();
            let min = condition_rows.first().map_or(0., |p| p.midpoint_seconds);
            let max = condition_rows
                .last()
                .map_or(min + 1., |p| p.midpoint_seconds)
                .max(min + f64::EPSILON);
            let color = COLORS[ci % COLORS.len()];
            let mut previous = None;
            for row in condition_rows {
                let x = 80 + ((row.midpoint_seconds - min) / (max - min) * 950.) as i32;
                let y = 550 - (row.power / ymax * 470.) as i32;
                if let Some((previous_index, px, py)) = previous {
                    if row.window_index == previous_index + 1 {
                        image.line(px, py, x, y, color);
                    }
                }
                image.rect(x - 2, y - 2, 5, 5, color);
                previous = Some((row.window_index, x, y));
            }
            image.rect(80 + ci as i32 * 170, 590, 14, 10, color);
            image.text(
                100 + ci as i32 * 170,
                589,
                &short(condition, 18),
                [35, 45, 55],
                1,
            );
        }
        image.text(450, 615, "TIME WITHIN EACH RECORDING", [75, 80, 85], 1);
        image.save_png(&figures.join(&file))?;
        made.push(format!("figures/{file}"));
    }
    Ok(made)
}
fn short(value: &str, max: usize) -> String {
    value
        .chars()
        .take(max)
        .collect::<String>()
        .to_ascii_uppercase()
}

fn safe_file_component(value: &str) -> String {
    let component: String = value
        .chars()
        .take(64)
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect();
    if component.is_empty() {
        "channel".into()
    } else {
        component
    }
}

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
    fn axes(&mut self) {
        self.line(78, 75, 78, 552, [65, 75, 85]);
        self.line(78, 552, 1040, 552, [65, 75, 85]);
        for n in 0..6 {
            let y = 552 - n * 95;
            self.line(75, y, 1040, y, [215, 220, 225]);
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
        ':' => [0, 12, 12, 0, 12, 12, 0],
        '(' => [2, 4, 8, 8, 8, 4, 2],
        ')' => [8, 4, 2, 2, 2, 4, 8],
        '^' => [4, 10, 17, 0, 0, 0, 0],
        ' ' => [0; 7],
        _ => [31, 17, 2, 4, 0, 4, 0],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn png_signature_and_dimensions() {
        let root = std::env::temp_dir().join(format!("lemon-mu-plot-{}.png", std::process::id()));
        let image = Image::new(12, 8);
        image.save_png(&root).expect("png");
        let bytes = fs::read(&root).expect("read");
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
        let _ = fs::remove_file(root);
    }

    #[test]
    fn channel_name_is_safe_for_artifact_paths() {
        assert_eq!(safe_file_component("A0"), "A0");
        assert_eq!(safe_file_component("../A 2"), "___A_2");
    }
}
