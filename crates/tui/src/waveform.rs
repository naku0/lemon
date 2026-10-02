//! Display-only continuity and min/max aggregation; never changes pipeline samples.
use crate::settings::WaveStyle;
use core_types::{ChannelSamples, FilteredChannel, RingBuffer, SignalFlag};

#[derive(Clone, Debug)]
pub struct Point {
    pub time: f64,
    pub raw: f64,
    pub filtered: Option<f64>,
    pub break_before: bool,
}
pub struct History {
    points: RingBuffer<Point>,
    last: Option<(String, String, u64, u64)>,
    interrupted: bool,
}
impl History {
    pub fn new(capacity: usize) -> Self {
        Self {
            points: RingBuffer::new(capacity),
            last: None,
            interrupted: true,
        }
    }
    pub fn iter(&self) -> impl Iterator<Item = &Point> {
        self.points.iter()
    }
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }
    pub fn break_line(&mut self) {
        self.interrupted = true;
    }
    pub fn accept(
        &mut self,
        channel: &ChannelSamples,
        filtered: Option<&FilteredChannel>,
        block_break: bool,
        rate: f64,
        tolerance: f64,
    ) {
        self.interrupted |= block_break;
        for (i, s) in channel.samples.iter().enumerate() {
            let clean = filtered
                .and_then(|c| c.samples.get(i))
                .copied()
                .flatten()
                .filter(|v| v.is_finite());
            let changed = self.last.as_ref().is_some_and(|(id, device, seq, time)| {
                id != &channel.id
                    || device != &channel.device
                    || s.sequence != seq.saturating_add(1)
                    || s.timestamp_ns <= *time
                    || (s.timestamp_ns - time) as f64 > tolerance * 1e9 / rate
            });
            let flagged = s.flags.iter().any(|f| {
                matches!(
                    f,
                    SignalFlag::LostSample | SignalFlag::InvalidPacket | SignalFlag::Disconnected
                )
            });
            self.points.push(Point {
                time: s.timestamp_ns as f64 / 1e9,
                raw: s.value,
                filtered: clean,
                break_before: self.interrupted || changed || flagged || clean.is_none(),
            });
            self.interrupted = clean.is_none() || flagged;
            self.last = Some((
                channel.id.clone(),
                channel.device.clone(),
                s.sequence,
                s.timestamp_ns,
            ));
        }
    }
}
pub struct Series {
    pub points: Vec<(f64, f64)>,
    pub line: bool,
}
pub fn series(
    history: &History,
    filtered: bool,
    start: f64,
    end: f64,
    width: usize,
    style: WaveStyle,
) -> Vec<Series> {
    let mut segments: Vec<Vec<(f64, f64)>> = Vec::new();
    let mut current = Vec::new();
    for p in history.iter().filter(|p| p.time >= start && p.time <= end) {
        let value = if filtered { p.filtered } else { Some(p.raw) }.filter(|v| v.is_finite());
        if (p.break_before || value.is_none()) && !current.is_empty() {
            segments.push(std::mem::take(&mut current));
        }
        if let Some(v) = value {
            current.push((p.time, v));
        }
    }
    if !current.is_empty() {
        segments.push(current);
    }
    if style != WaveStyle::Envelope {
        return segments
            .into_iter()
            .map(|points| Series {
                line: style == WaveStyle::Line && points.len() > 1,
                points,
            })
            .collect();
    }
    // A bucket crossing discontinuity uses two unconnected extrema, never a vertical bridge.
    let width = width.max(1);
    let mut buckets: Vec<Option<(f64, f64, usize, bool)>> = vec![None; width];
    for (segment, points) in segments.iter().enumerate() {
        for (time, value) in points {
            let i = (((time - start) / (end - start).max(f64::EPSILON) * width as f64) as usize)
                .min(width - 1);
            match &mut buckets[i] {
                Some((min, max, last, broken)) => {
                    *min = min.min(*value);
                    *max = max.max(*value);
                    *broken |= *last != segment;
                    *last = segment;
                }
                b => *b = Some((*value, *value, segment, false)),
            }
        }
    }
    buckets
        .into_iter()
        .enumerate()
        .filter_map(|(i, b)| {
            b.map(|(min, max, _, broken)| {
                let t = start + (i as f64 + 0.5) * (end - start) / width as f64;
                Series {
                    points: vec![(t, min), (t, max)],
                    line: !broken,
                }
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_types::Sample;
    fn input(seq: u64, time: u64, values: &[f64]) -> ChannelSamples {
        ChannelSamples {
            id: "ch1".into(),
            device: "test".into(),
            samples: values
                .iter()
                .enumerate()
                .map(|(i, v)| Sample {
                    sequence: seq + i as u64,
                    timestamp_ns: time + i as u64 * 4_000_000,
                    value: *v,
                    flags: vec![],
                })
                .collect(),
        }
    }
    fn add(h: &mut History, c: &ChannelSamples) {
        let f = FilteredChannel {
            id: c.id.clone(),
            samples: c.samples.iter().map(|s| Some(s.value)).collect(),
        };
        h.accept(c, Some(&f), false, 250., 2.5);
    }
    #[test]
    fn lines_split_at_missing_sequence_time_pause_and_filtered_value() {
        let mut h = History::new(100);
        add(&mut h, &input(0, 0, &[1., 2.]));
        add(&mut h, &input(3, 12_000_000, &[3., 4.]));
        assert_eq!(series(&h, false, 0., 1., 10, WaveStyle::Line).len(), 2);
        h.break_line();
        add(&mut h, &input(5, 20_000_000, &[5., 6.]));
        let c = input(7, 28_000_000, &[7., 8., 9.]);
        h.accept(
            &c,
            Some(&FilteredChannel {
                id: "ch1".into(),
                samples: vec![Some(7.), None, Some(9.)],
            }),
            false,
            250.,
            2.5,
        );
        let s = series(&h, true, 0., 1., 10, WaveStyle::Line);
        assert!(s
            .iter()
            .all(|line| !line.points.iter().any(|(_, y)| *y == 8.)));
        assert_eq!(s.len(), 4);
        add(&mut h, &input(10, 500_000_000, &[10.]));
        assert_eq!(
            series(&h, false, 0., 1., 10, WaveStyle::Line)
                .last()
                .expect("last")
                .points
                .len(),
            1
        );
    }
    #[test]
    fn envelope_retains_spike_and_bounds_output_by_width() {
        let mut h = History::new(1000);
        let mut values = vec![1.; 500];
        values[123] = 999.;
        values[124] = -80.;
        add(&mut h, &input(0, 0, &values));
        let s = series(&h, false, 0., 2., 10, WaveStyle::Envelope);
        assert!(s.len() <= 10);
        assert!(s.iter().flat_map(|s| &s.points).any(|(_, y)| *y == 999.));
        assert!(s.iter().flat_map(|s| &s.points).any(|(_, y)| *y == -80.));
        assert!(series(&h, false, 0., 2., 10, WaveStyle::Points)
            .iter()
            .all(|s| !s.line));
    }
    #[test]
    fn envelope_does_not_bridge_break_in_same_bucket_and_history_is_bounded() {
        let mut h = History::new(4);
        add(&mut h, &input(0, 0, &[1., 2.]));
        h.break_line();
        add(&mut h, &input(2, 8_000_000, &[3., 4.]));
        assert!(!series(&h, false, 0., 1., 1, WaveStyle::Envelope)[0].line);
        add(&mut h, &input(4, 16_000_000, &[5., 6.]));
        assert_eq!(h.iter().count(), 4);
    }
}
