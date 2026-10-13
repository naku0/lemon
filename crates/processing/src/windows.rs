//! Bounded, deterministic candidate windows shared by bundle analysis and plugins.
use core_types::Result;
pub use core_types::SignalFlag;
use std::collections::{BTreeSet, VecDeque};

pub const WINDOWING_ALGORITHM_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq)]
pub struct AnalysisSample {
    pub sequence: u64,
    pub timestamp_ns: u64,
    pub segment_id: u64,
    pub value: Option<f64>,
    pub eligible: bool,
    pub flags: Vec<SignalFlag>,
}
#[derive(Debug, Clone)]
pub struct WindowingConfig {
    pub window_samples: usize,
    pub hop_samples: usize,
    pub sample_rate_hz: f64,
    pub timestamp_tolerance_ratio: f64,
    pub reject_low_quality: bool,
}
impl WindowingConfig {
    pub fn new(window_samples: usize, hop_samples: usize, sample_rate_hz: f64) -> Self {
        Self {
            window_samples,
            hop_samples,
            sample_rate_hz,
            timestamp_tolerance_ratio: 0.51,
            reject_low_quality: true,
        }
    }
    fn validate(&self) -> Result<()> {
        if !(2..=65536).contains(&self.window_samples)
            || self.hop_samples == 0
            || self.hop_samples > self.window_samples
        {
            return Err("windowing: window must be 2..65536 samples; hop must be 1..window".into());
        }
        if !self.sample_rate_hz.is_finite()
            || self.sample_rate_hz <= 0.0
            || !self.timestamp_tolerance_ratio.is_finite()
            || !(0.0..1.0).contains(&self.timestamp_tolerance_ratio)
        {
            return Err(
                "windowing: positive finite sample rate and tolerance in [0,1) required".into(),
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WindowRejectionReason {
    InsufficientData,
    MissingFilteredSample,
    IneligibleSample,
    MixedSegment,
    SequenceDiscontinuity,
    TimestampDiscontinuity,
    InvalidPacket,
    LostSample,
    Outlier,
    Saturation,
    Disconnected,
    FilterWarmup,
    LowQuality,
}
impl WindowRejectionReason {
    pub fn code(self) -> &'static str {
        match self {
            Self::InsufficientData => "InsufficientData",
            Self::MissingFilteredSample => "MissingFilteredSample",
            Self::IneligibleSample => "IneligibleSample",
            Self::MixedSegment => "MixedSegment",
            Self::SequenceDiscontinuity => "SequenceDiscontinuity",
            Self::TimestampDiscontinuity => "TimestampDiscontinuity",
            Self::InvalidPacket => "InvalidPacket",
            Self::LostSample => "LostSample",
            Self::Outlier => "Outlier",
            Self::Saturation => "Saturation",
            Self::Disconnected => "Disconnected",
            Self::FilterWarmup => "FilterWarmup",
            Self::LowQuality => "LowQuality",
        }
    }
}
#[derive(Debug, Clone, PartialEq)]
pub enum WindowCandidate {
    Accepted {
        index: u64,
        segment_id: u64,
        start_timestamp_ns: u64,
        end_timestamp_ns: u64,
        samples: Vec<AnalysisSample>,
    },
    Rejected {
        index: u64,
        segment_id: Option<u64>,
        start_timestamp_ns: u64,
        end_timestamp_ns: u64,
        available_samples: usize,
        reasons: Vec<WindowRejectionReason>,
    },
}

/// At most one window is buffered; push emits at most a trailing and a full candidate.
/// A new grid starts at each segment/eligibility transition. Finish emits one
/// incomplete tail only when new samples arrived since the last full candidate.
pub struct WindowGenerator {
    config: WindowingConfig,
    buffer: VecDeque<AnalysisSample>,
    run: Option<(u64, bool)>,
    new_samples: usize,
    next_index: u64,
}
impl WindowGenerator {
    pub fn new(config: WindowingConfig) -> Result<Self> {
        config.validate()?;
        Ok(Self {
            config,
            buffer: VecDeque::new(),
            run: None,
            new_samples: 0,
            next_index: 0,
        })
    }
    pub fn push(&mut self, sample: AnalysisSample) -> Vec<WindowCandidate> {
        let mut output = Vec::new();
        let run = (sample.segment_id, self.sample_reasons(&sample).is_empty());
        if self.run.is_some_and(|previous| previous != run) {
            if let Some(tail) = self.finish() {
                output.push(tail);
            }
            self.buffer.clear();
        }
        self.run = Some(run);
        self.buffer.push_back(sample);
        self.new_samples += 1;
        if self.buffer.len() == self.config.window_samples {
            output.push(self.candidate(false));
            self.buffer.drain(..self.config.hop_samples);
            self.new_samples = 0;
        }
        output
    }
    pub fn finish(&mut self) -> Option<WindowCandidate> {
        let candidate = if self.new_samples > 0 && !self.buffer.is_empty() {
            Some(self.candidate(true))
        } else {
            None
        };
        self.buffer.clear();
        self.new_samples = 0;
        self.run = None;
        candidate
    }
    fn sample_reasons(&self, sample: &AnalysisSample) -> BTreeSet<WindowRejectionReason> {
        use WindowRejectionReason as R;
        let mut reasons = BTreeSet::new();
        if !sample.eligible {
            reasons.insert(R::IneligibleSample);
        }
        match sample.value {
            None => {
                reasons.insert(R::MissingFilteredSample);
            }
            Some(value) if !value.is_finite() => {
                reasons.insert(R::InvalidPacket);
            }
            _ => {}
        }
        for flag in &sample.flags {
            let reason = match flag {
                SignalFlag::InvalidPacket => R::InvalidPacket,
                SignalFlag::LostSample => R::LostSample,
                SignalFlag::Outlier => R::Outlier,
                SignalFlag::Saturation => R::Saturation,
                SignalFlag::Disconnected => R::Disconnected,
                SignalFlag::FilterWarmup => R::FilterWarmup,
                SignalFlag::LowQuality if self.config.reject_low_quality => R::LowQuality,
                _ => continue,
            };
            reasons.insert(reason);
        }
        reasons
    }
    fn candidate(&mut self, incomplete: bool) -> WindowCandidate {
        use WindowRejectionReason as R;
        // Only called with a nonempty bounded buffer.
        let first = &self.buffer[0];
        let last = &self.buffer[self.buffer.len() - 1];
        let mut reasons = BTreeSet::new();
        if incomplete {
            reasons.insert(R::InsufficientData);
        }
        for sample in &self.buffer {
            reasons.extend(self.sample_reasons(sample));
        }
        let segment_id = if self
            .buffer
            .iter()
            .all(|sample| sample.segment_id == first.segment_id)
        {
            Some(first.segment_id)
        } else {
            reasons.insert(R::MixedSegment);
            None
        };
        let expected = 1e9 / self.config.sample_rate_hz;
        for (a, b) in self.buffer.iter().zip(self.buffer.iter().skip(1)) {
            if a.sequence.checked_add(1) != Some(b.sequence) {
                reasons.insert(R::SequenceDiscontinuity);
            }
            if b.timestamp_ns <= a.timestamp_ns
                || ((b.timestamp_ns.saturating_sub(a.timestamp_ns) as f64) - expected).abs()
                    > expected * self.config.timestamp_tolerance_ratio
            {
                reasons.insert(R::TimestampDiscontinuity);
            }
        }
        let index = self.next_index;
        self.next_index += 1;
        if reasons.is_empty() {
            WindowCandidate::Accepted {
                index,
                segment_id: first.segment_id,
                start_timestamp_ns: first.timestamp_ns,
                end_timestamp_ns: last.timestamp_ns,
                samples: self.buffer.iter().cloned().collect(),
            }
        } else {
            WindowCandidate::Rejected {
                index,
                segment_id,
                start_timestamp_ns: first.timestamp_ns,
                end_timestamp_ns: last.timestamp_ns,
                available_samples: self.buffer.len(),
                reasons: reasons.into_iter().collect(),
            }
        }
    }
}

/// Legacy files lack segment IDs. Derive boundaries using the same cadence rule.
pub struct LegacySegments {
    previous: Option<(u64, u64)>,
    segment: u64,
    config: WindowingConfig,
}
impl LegacySegments {
    pub fn new(config: WindowingConfig) -> Result<Self> {
        config.validate()?;
        Ok(Self {
            previous: None,
            segment: 0,
            config,
        })
    }
    pub fn segment(&mut self, sequence: u64, timestamp_ns: u64) -> u64 {
        if let Some((last_sequence, last_time)) = self.previous {
            let expected = 1e9 / self.config.sample_rate_hz;
            if last_sequence.checked_add(1) != Some(sequence)
                || timestamp_ns <= last_time
                || ((timestamp_ns.saturating_sub(last_time) as f64) - expected).abs()
                    > expected * self.config.timestamp_tolerance_ratio
            {
                self.segment += 1;
            }
        }
        self.previous = Some((sequence, timestamp_ns));
        self.segment
    }
}

/// Parse the stable pipe-separated codes exported in processed.csv.
pub fn flags_from_codes(codes: &str) -> Vec<SignalFlag> {
    codes
        .split('|')
        .filter_map(|code| match code {
            "InvalidPacket" => Some(SignalFlag::InvalidPacket),
            "LostSample" => Some(SignalFlag::LostSample),
            "Outlier" => Some(SignalFlag::Outlier),
            "Saturation" => Some(SignalFlag::Saturation),
            "Disconnected" => Some(SignalFlag::Disconnected),
            "FilterWarmup" => Some(SignalFlag::FilterWarmup),
            "LowQuality" => Some(SignalFlag::LowQuality),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample(i: u64) -> AnalysisSample {
        AnalysisSample {
            sequence: i,
            timestamp_ns: i * 10_000_000,
            segment_id: 0,
            value: Some(i as f64),
            eligible: true,
            flags: vec![],
        }
    }
    fn run(samples: &[AnalysisSample], chunk: usize) -> Vec<WindowCandidate> {
        let mut generator =
            WindowGenerator::new(WindowingConfig::new(4, 2, 100.0)).expect("generator");
        let mut output = Vec::new();
        for block in samples.chunks(chunk) {
            for sample in block {
                output.extend(generator.push(sample.clone()));
            }
        }
        output.extend(generator.finish());
        assert!(generator.finish().is_none());
        output
    }
    #[test]
    fn segment_and_off_grid_artifact_restart_grid_with_one_tail() {
        let mut samples = (0..13).map(sample).collect::<Vec<_>>();
        for item in &mut samples[5..] {
            item.segment_id = 1;
        }
        samples[5].eligible = false;
        samples[5].flags.push(SignalFlag::Outlier);
        samples[6].eligible = false;
        samples[6].flags.push(SignalFlag::FilterWarmup);
        let output = run(&samples, 1);
        assert_eq!(output, run(&samples, 3));
        assert_eq!(output, run(&samples, 13));
        let starts = output
            .iter()
            .filter_map(|candidate| match candidate {
                WindowCandidate::Accepted { samples, .. } => Some(samples[0].sequence),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(starts, vec![0, 7, 9]);
        assert_eq!(output.len(), 5);
        for candidate in output {
            if let WindowCandidate::Accepted {
                segment_id,
                samples,
                ..
            } = candidate
            {
                assert!(samples
                    .iter()
                    .all(|sample| sample.segment_id == segment_id && sample.eligible));
            }
        }
    }
    #[test]
    fn missing_values_and_bad_cadence_have_all_unique_reasons() {
        let mut samples = (0..4).map(sample).collect::<Vec<_>>();
        for sample in &mut samples {
            sample.value = None;
            sample.eligible = false;
            sample.flags = vec![SignalFlag::LostSample; 2];
        }
        samples[2].timestamp_ns += 10_000_000;
        samples[2].sequence += 1;
        let output = run(&samples, 4);
        assert_eq!(output.len(), 1);
        let WindowCandidate::Rejected { reasons, .. } = &output[0] else {
            panic!("rejected")
        };
        assert_eq!(
            reasons,
            &vec![
                WindowRejectionReason::MissingFilteredSample,
                WindowRejectionReason::IneligibleSample,
                WindowRejectionReason::SequenceDiscontinuity,
                WindowRejectionReason::TimestampDiscontinuity,
                WindowRejectionReason::LostSample
            ]
        );
    }
    #[test]
    fn tail_is_once_and_invalid_config_is_rejected() {
        let samples = (0..5).map(sample).collect::<Vec<_>>();
        let output = run(&samples, 2);
        assert_eq!(output.len(), 2);
        assert!(
            matches!(&output[1],WindowCandidate::Rejected { available_samples: 3, reasons, .. } if reasons == &[WindowRejectionReason::InsufficientData])
        );
        assert!(WindowGenerator::new(WindowingConfig::new(4, 5, 100.0)).is_err());
        assert!(WindowGenerator::new(WindowingConfig::new(4, 2, f64::NAN)).is_err());
    }
}
