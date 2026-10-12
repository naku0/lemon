//! Bounded, offline heuristic inspection of a captured tagged byte stream.
use core_types::Result;
use std::{
    collections::BTreeSet,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

#[derive(Debug)]
pub struct CounterCandidate {
    pub tag: String,
    pub discontinuities: u64,
    pub minimum_missing: u64,
    pub duplicates: u64,
}
#[derive(Debug)]
pub struct TaggedCandidate {
    pub tags: Vec<String>,
    pub frame_size: usize,
    pub complete_frames: u64,
    pub prefix_bytes: u64,
    pub suffix_bytes: u64,
    pub internal_discarded_bytes: u64,
    pub counters: Vec<CounterCandidate>,
}
fn matches(bytes: &[u8], tags: &[[u8; 2]]) -> bool {
    bytes
        .iter()
        .enumerate()
        .all(|(i, b)| i % 3 == 2 || *b == tags[i / 3][i % 3])
}
fn infer(bytes: &[u8]) -> Option<Vec<[u8; 2]>> {
    let mut candidates = BTreeSet::new();
    for start in 0..bytes.len() {
        let mut tags = Vec::new();
        for pos in (start..bytes.len().saturating_sub(2)).step_by(3).take(11) {
            if bytes[pos] != b'A' || !bytes[pos + 1].is_ascii_digit() {
                break;
            }
            let tag = [bytes[pos], bytes[pos + 1]];
            if tags.first() == Some(&tag) {
                let size = tags.len() * 3;
                if start + size * 3 <= bytes.len()
                    && (0..3)
                        .all(|i| matches(&bytes[start + i * size..start + (i + 1) * size], &tags))
                {
                    if let Some((i, _)) = tags.iter().enumerate().min_by_key(|(_, t)| *t) {
                        tags.rotate_left(i);
                    }
                    candidates.insert(tags);
                }
                break;
            }
            if tags.contains(&tag) {
                break;
            }
            tags.push(tag);
        }
    }
    candidates
        .into_iter()
        .map(|tags| {
            let size = tags.len() * 3;
            let mut covered = 0;
            let mut i = 0;
            while i + size <= bytes.len() {
                if matches(&bytes[i..i + size], &tags) {
                    covered += size;
                    i += size;
                } else {
                    i += 1;
                }
            }
            (covered, tags)
        })
        .filter(|(covered, _)| *covered >= bytes.len() / 2)
        .max_by_key(|(n, t)| (*n, t.len()))
        .map(|(_, t)| t)
}

/// Infer a candidate from at most 64 KiB, then verify the entire file in a streaming pass.
/// No signal fields are selected automatically. A counter-like signal is only a candidate.
pub fn inspect_tagged_capture(path: &Path) -> Result<Option<TaggedCandidate>> {
    let mut file =
        File::open(path).map_err(|e| format!("probe/inspect {}: {e}", path.display()))?;
    let mut sample = Vec::new();
    (&mut file)
        .take(65536)
        .read_to_end(&mut sample)
        .map_err(|e| e.to_string())?;
    let Some(tags) = infer(&sample) else {
        return Ok(None);
    };
    file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let mut pending = Vec::new();
    let mut total = 0u64;
    let mut first_start = None;
    let mut last_end = 0;
    let mut frames = 0u64;
    let mut previous: Option<Vec<u8>> = None;
    let mut discontinuities = vec![0u64; tags.len()];
    let mut missing = vec![0u64; tags.len()];
    let mut duplicates = vec![0u64; tags.len()];
    let size = tags.len() * 3;
    let mut buffer = [0u8; 4096];
    loop {
        let n = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        for &byte in &buffer[..n] {
            total += 1;
            pending.push(byte);
            while !pending.is_empty() && !matches(&pending, &tags) {
                pending.remove(0);
            }
            if pending.len() == size {
                first_start.get_or_insert(total - size as u64);
                last_end = total;
                frames += 1;
                let values: Vec<u8> = pending.chunks_exact(3).map(|p| p[2]).collect();
                if let Some(previous) = previous {
                    for i in 0..tags.len() {
                        let delta = values[i].wrapping_sub(previous[i]);
                        if delta != 1 {
                            discontinuities[i] += 1;
                        }
                        missing[i] += delta.saturating_sub(1) as u64;
                        duplicates[i] += u64::from(delta == 0);
                    }
                }
                previous = Some(values);
                pending.clear();
            }
        }
    }
    let tag_names: Vec<String> = tags
        .iter()
        .map(|t| String::from_utf8_lossy(t).into_owned())
        .collect();
    let counters = (0..tags.len())
        .filter(|&i| frames >= 10 && discontinuities[i] * 100 <= frames.saturating_sub(1) * 5)
        .map(|i| CounterCandidate {
            tag: tag_names[i].clone(),
            discontinuities: discontinuities[i],
            minimum_missing: missing[i],
            duplicates: duplicates[i],
        })
        .collect();
    let prefix = first_start.unwrap_or(total);
    let suffix = total - last_end;
    Ok(Some(TaggedCandidate {
        tags: tag_names,
        frame_size: size,
        complete_frames: frames,
        prefix_bytes: prefix,
        suffix_bytes: suffix,
        internal_discarded_bytes: total.saturating_sub(prefix + suffix + frames * size as u64),
        counters,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detects_complete_cycle_not_six_byte_prefix() {
        let path =
            std::env::temp_dir().join(format!("lemon-tagged-inspect-{}.bin", std::process::id()));
        let mut bytes = b"garbage".to_vec();
        for seq in 250u16..550 {
            for tag in b"012345679" {
                bytes.extend([b'A', *tag, if *tag == b'9' { seq as u8 } else { b'A' }]);
            }
        }
        bytes.extend(b"A0");
        std::fs::write(&path, bytes).expect("fixture");
        let result = inspect_tagged_capture(&path)
            .expect("inspect")
            .expect("candidate");
        assert_eq!(
            result.tags,
            ["A0", "A1", "A2", "A3", "A4", "A5", "A6", "A7", "A9"]
        );
        assert_eq!(result.frame_size, 27);
        assert_eq!(result.complete_frames, 300);
        assert_eq!(
            (
                result.prefix_bytes,
                result.suffix_bytes,
                result.internal_discarded_bytes
            ),
            (7, 2, 0)
        );
        assert_eq!(result.counters.len(), 1);
        assert_eq!(result.counters[0].tag, "A9");
        assert_eq!(result.counters[0].discontinuities, 0);
        std::fs::remove_file(path).expect("cleanup");
    }
}
