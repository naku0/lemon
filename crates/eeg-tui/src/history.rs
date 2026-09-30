//! Bounded display-only history of values already computed by eeg-processing.
use crate::settings::PowerMode;
use eeg_core::{BandPower, ProcessedSignalBlock, SignalFlag};
use std::collections::VecDeque;

const MAX_POINTS: usize = 10_000;
#[derive(Clone, Debug)]
pub struct Point {
    pub time: f64,
    pub segment: u64,
    pub power: BandPower,
}
#[derive(Default)]
pub struct BandHistory {
    points: VecDeque<Point>,
    segment: u64,
    last_sequence: Option<u64>,
}
impl BandHistory {
    pub fn break_line(&mut self) {
        self.segment = self.segment.wrapping_add(1);
    }
    pub fn accept(&mut self, block: &ProcessedSignalBlock, seconds: f64) {
        if self
            .last_sequence
            .is_some_and(|seq| block.sequence != seq.saturating_add(1))
            || block.flags.iter().any(|f| {
                matches!(
                    f,
                    SignalFlag::LostSample
                        | SignalFlag::InvalidPacket
                        | SignalFlag::Disconnected
                        | SignalFlag::InsufficientData
                )
            })
        {
            self.break_line();
        }
        self.last_sequence = Some(block.sequence);
        if let (Some(spectrum), Some(bands)) = (&block.spectrum, &block.band_power) {
            let time = spectrum.timestamp_ns as f64 / 1e9;
            for power in bands {
                self.points.push_back(Point {
                    time,
                    segment: self.segment,
                    power: power.clone(),
                });
            }
            self.trim(time, seconds);
        }
    }
    pub fn trim(&mut self, now: f64, seconds: f64) {
        while self.points.len() > MAX_POINTS
            || self.points.front().is_some_and(|p| p.time < now - seconds)
        {
            self.points.pop_front();
        }
    }
    pub fn len(&self) -> usize {
        self.points.len()
    }
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }
    pub fn latest_time(&self) -> Option<f64> {
        self.points.back().map(|p| p.time)
    }
    pub fn series(
        &self,
        channel: &str,
        band: &str,
        mode: PowerMode,
        smoothed: bool,
    ) -> Vec<Vec<(f64, f64)>> {
        let mut result = Vec::new();
        let mut line = Vec::new();
        let mut segment = None;
        for point in self
            .points
            .iter()
            .filter(|p| p.power.channel == channel && p.power.band == band)
        {
            let value = match mode {
                PowerMode::Absolute => Some(if smoothed {
                    point.power.smoothed
                } else {
                    point.power.absolute
                }),
                PowerMode::Relative => Some(point.power.relative * 100.),
                PowerMode::Baseline => point.power.baseline_change_pct,
            }
            .filter(|v| v.is_finite());
            if (segment != Some(point.segment) || value.is_none()) && !line.is_empty() {
                result.push(std::mem::take(&mut line));
            }
            segment = Some(point.segment);
            if let Some(value) = value {
                line.push((point.time, value));
            }
        }
        if !line.is_empty() {
            result.push(line);
        }
        result
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn point(time: f64, baseline: Option<f64>, segment: u64) -> Point {
        Point {
            time,
            segment,
            power: BandPower {
                channel: "ch1".into(),
                band: "Theta".into(),
                absolute: 2.,
                relative: 0.5,
                smoothed: 1.8,
                baseline_change_pct: baseline,
                asymmetry: None,
            },
        }
    }
    #[test]
    fn none_and_breaks_are_not_zero_or_connections() {
        let mut h = BandHistory::default();
        h.points.extend([
            point(0., Some(10.), 0),
            point(1., None, 0),
            point(2., Some(20.), 0),
            point(3., Some(30.), 1),
        ]);
        assert_eq!(
            h.series("ch1", "Theta", PowerMode::Baseline, false),
            vec![vec![(0., 10.)], vec![(2., 20.)], vec![(3., 30.)]]
        );
    }
    #[test]
    fn capacity_and_time_are_bounded() {
        let mut h = BandHistory::default();
        for i in 0..20_000 {
            h.points.push_back(point(i as f64, Some(1.), 0));
            h.trim(i as f64, 3600.);
        }
        assert_eq!(h.len(), 3601);
        for _ in 0..20_000 {
            h.points.push_back(point(20000., Some(1.), 0));
            h.trim(20000., 3600.);
        }
        assert_eq!(h.len(), MAX_POINTS);
    }
}
