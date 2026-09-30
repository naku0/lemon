//! Stateful causal biquads and one-sided Hann-window PSD. No terminal dependencies.
use eeg_core::*;
use rustfft::{num_complex::Complex, Fft, FftPlanner};
use std::sync::Arc;

#[derive(Clone)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    z: [f64; 2],
}
impl Biquad {
    fn new(fs: f64, hz: f64, q: f64, kind: u8) -> Self {
        let omega = std::f64::consts::TAU * hz / fs;
        let c = omega.cos();
        let alpha = omega.sin() / (2. * q);
        let a0 = 1. + alpha;
        let b = match kind {
            0 => [(1. + c) / 2., -(1. + c), (1. + c) / 2.],
            1 => [(1. - c) / 2., 1. - c, (1. - c) / 2.],
            _ => [1., -2. * c, 1.],
        };
        Self {
            b: b.map(|x| x / a0),
            a: [-2. * c / a0, (1. - alpha) / a0],
            z: [0.; 2],
        }
    }
    fn apply(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
    fn reset(&mut self) {
        self.z = [0.; 2];
    }
}
struct ChannelState {
    filters: Vec<Biquad>,
    raw: RingBuffer<f64>,
    filtered: RingBuffer<f64>,
    since_fft: usize,
    previous: Option<(u64, u64, f64)>,
    spectrum: Option<ChannelSpectrum>,
    smoothed: Vec<Option<f64>>,
}
impl ChannelState {
    fn reset(&mut self) {
        for f in &mut self.filters {
            f.reset();
        }
        self.raw.clear();
        self.filtered.clear();
        self.since_fft = 0;
        self.spectrum = None;
        self.smoothed.fill(None);
    }
}
pub struct Processor {
    config: ProcessingConfig,
    fs: f64,
    states: Vec<ChannelState>,
    fft: Arc<dyn Fft<f64>>,
    hann: Vec<f64>,
    last_block: Option<u64>,
    channel_ids: Option<Vec<(String, String)>>,
    aligned_samples: usize,
}
impl Processor {
    pub fn new(config: &Config) -> Result<Self> {
        config.validate()?;
        let p = &config.processing;
        let fs = config.source.sample_rate_hz;
        let mut planner = FftPlanner::new();
        let fft = planner.plan_fft_forward(p.window_samples);
        let hann = (0..p.window_samples)
            .map(|i| {
                0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / (p.window_samples - 1) as f64).cos()
            })
            .collect();
        let states = (0..config.source.channels)
            .map(|_| {
                let mut filters = vec![
                    Biquad::new(fs, p.high_pass_hz, std::f64::consts::FRAC_1_SQRT_2, 0),
                    Biquad::new(fs, p.low_pass_hz, std::f64::consts::FRAC_1_SQRT_2, 1),
                ];
                if p.notch {
                    filters.push(Biquad::new(fs, p.notch_hz, p.notch_q, 2));
                }
                ChannelState {
                    filters,
                    raw: RingBuffer::new(p.window_samples),
                    filtered: RingBuffer::new(p.window_samples),
                    since_fft: 0,
                    previous: None,
                    spectrum: None,
                    smoothed: vec![None; p.bands.len()],
                }
            })
            .collect();
        Ok(Self {
            config: p.clone(),
            fs,
            states,
            fft,
            hann,
            last_block: None,
            channel_ids: None,
            aligned_samples: 0,
        })
    }
    pub fn process(&mut self, block: RawSignalBlock) -> Result<ProcessedSignalBlock> {
        if block.channels.iter().any(|ch| ch.samples.len() > 4096) {
            return Err("processing: блок превышает 4096 отсчётов".into());
        }
        let ids: Vec<_> = block
            .channels
            .iter()
            .map(|ch| (ch.id.clone(), ch.device.clone()))
            .collect();
        if ids
            .iter()
            .enumerate()
            .any(|(i, id)| id.0.is_empty() || ids[..i].iter().any(|other| other.0 == id.0))
        {
            return Err("processing: пустые или повторяющиеся ID каналов".into());
        }
        if self
            .channel_ids
            .as_ref()
            .is_some_and(|previous| *previous != ids)
        {
            return Err("processing: идентификаторы или порядок каналов изменились".into());
        }
        if block.channels.len() != self.states.len() {
            return Err("processing: число каналов изменилось".into());
        }
        if !block.sample_rate_hz.is_finite() || (block.sample_rate_hz - self.fs).abs() > 1e-6 {
            return Err("processing: неожиданная частота дискретизации".into());
        }
        if self.last_block.is_some_and(|s| block.sequence <= s) {
            return Err("processing: немонотонный номер блока".into());
        }
        let mut flags = block.flags.clone();
        if self
            .last_block
            .is_some_and(|s| block.sequence > s.saturating_add(1))
        {
            flags.push(SignalFlag::LostSample);
        }
        self.last_block = Some(block.sequence);
        self.channel_ids = Some(ids);
        let skew = if block.channels.len() == 2 {
            block.channels[0]
                .samples
                .iter()
                .zip(&block.channels[1].samples)
                .map(|(a, b)| a.timestamp_ns.abs_diff(b.timestamp_ns) as f64 / 1e6)
                .max_by(f64::total_cmp)
        } else {
            None
        };
        let synchronized = block.channels.len() == 1
            || (block.synchronized_clock
                && skew.is_some_and(|s| s <= self.config.sync_tolerance_ms)
                && block.channels[0].samples.len() == block.channels[1].samples.len());
        if !synchronized {
            flags.push(SignalFlag::ChannelMismatch);
        }
        let mut qualities = Vec::new();
        let mut filtered_channels = Vec::new();
        let mut lost_samples = 0u64;
        let mut invalid_samples = 0;
        let mut outliers = 0;
        let mut fft_updated = false;
        for (channel, state) in block.channels.iter().zip(&mut self.states) {
            let mut filtered = Vec::with_capacity(channel.samples.len());
            let mut bad = 0usize;
            let mut gap = false;
            let mut sum = 0.;
            let mut sum2 = 0.;
            let mut valid = 0;
            for sample in &channel.samples {
                if let Some((seq, time, _)) = state.previous {
                    if sample.sequence <= seq || sample.timestamp_ns <= time {
                        invalid_samples += 1;
                        bad += 1;
                        flags.push(SignalFlag::InvalidPacket);
                        filtered.push(None);
                        state.reset();
                        continue;
                    }
                    if sample.sequence > seq.saturating_add(1) {
                        lost_samples = lost_samples.saturating_add(sample.sequence - seq - 1);
                        gap = true;
                        flags.push(SignalFlag::LostSample);
                        state.reset();
                    }
                }
                flags.extend(sample.flags.iter().copied());
                if !sample.value.is_finite() || sample.value.abs() > 1e100 {
                    invalid_samples += 1;
                    bad += 1;
                    flags.push(SignalFlag::InvalidPacket);
                    state.reset();
                    state.previous = Some((sample.sequence, sample.timestamp_ns, 0.));
                    filtered.push(None);
                    continue;
                }
                if sample.value.abs() > self.config.expected_abs_max {
                    bad += 1;
                    flags.push(SignalFlag::Saturation);
                }
                if state
                    .previous
                    .is_some_and(|(_, _, x)| (sample.value - x).abs() > self.config.outlier_step)
                {
                    bad += 1;
                    outliers += 1;
                    flags.push(SignalFlag::Outlier);
                }
                state.previous = Some((sample.sequence, sample.timestamp_ns, sample.value));
                sum += sample.value;
                sum2 += sample.value * sample.value;
                valid += 1;
                let mut y = sample.value;
                for filter in &mut state.filters {
                    y = filter.apply(y);
                }
                if !y.is_finite() {
                    invalid_samples += 1;
                    bad += 1;
                    flags.push(SignalFlag::InvalidPacket);
                    state.reset();
                    filtered.push(None);
                    continue;
                }
                filtered.push(Some(y));
                state.raw.push(sample.value);
                state.filtered.push(y);
                state.since_fft += 1;
                if state.raw.len() == self.config.window_samples
                    && (state.spectrum.is_none() || state.since_fft >= self.config.hop_samples)
                {
                    let raw_psd = psd(&self.fft, &self.hann, state.raw.iter().copied(), self.fs);
                    let filtered_psd = psd(
                        &self.fft,
                        &self.hann,
                        state.filtered.iter().copied(),
                        self.fs,
                    );
                    let df = self.fs / self.config.window_samples as f64;
                    let total = raw_psd.iter().skip(1).sum::<f64>();
                    let mains = raw_psd
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| ((*i as f64 * df) - 50.).abs() <= 2.)
                        .map(|(_, p)| p)
                        .sum::<f64>();
                    state.spectrum = Some(ChannelSpectrum {
                        channel: channel.id.clone(),
                        timestamp_ns: sample.timestamp_ns,
                        raw_psd,
                        filtered_psd,
                        mains_ratio: if total > 0. { mains / total } else { 0. },
                    });
                    state.since_fft = 0;
                    fft_updated = true;
                }
            }
            // Use a window for flatline detection: single-sample serial blocks cannot estimate variance.
            let flat = if state.raw.len() >= 32 {
                let mean = state.raw.iter().sum::<f64>() / state.raw.len() as f64;
                state.raw.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (state.raw.len() as f64)
                    < self.config.flat_stddev.powi(2)
            } else {
                valid >= 8
                    && (sum2 / valid as f64 - (sum / valid as f64).powi(2))
                        .max(0.)
                        .sqrt()
                        < self.config.flat_stddev
            };
            let mains = state
                .spectrum
                .as_ref()
                .is_some_and(|s| s.mains_ratio > self.config.mains_ratio_limit);
            let quality = if block.flags.contains(&SignalFlag::Disconnected) {
                SignalQuality::Disconnected
            } else if channel.samples.is_empty() {
                SignalQuality::Unknown
            } else if flat || bad > channel.samples.len() / 10 || !synchronized {
                SignalQuality::Poor
            } else if gap || bad > 0 || mains {
                SignalQuality::Acceptable
            } else {
                SignalQuality::Good
            };
            if matches!(quality, SignalQuality::Poor | SignalQuality::Acceptable) {
                flags.push(SignalFlag::LowQuality);
            }
            qualities.push(quality);
            filtered_channels.push(FilteredChannel {
                id: channel.id.clone(),
                samples: filtered,
            });
        }
        if synchronized
            && lost_samples == 0
            && invalid_samples == 0
            && !flags.iter().any(|f| {
                matches!(
                    f,
                    SignalFlag::LostSample
                        | SignalFlag::InvalidPacket
                        | SignalFlag::ChannelMismatch
                )
            })
        {
            self.aligned_samples = (self.aligned_samples + block.channels[0].samples.len())
                .min(self.config.window_samples);
        } else {
            self.aligned_samples = 0;
        }
        let enough = self.states.iter().all(|s| s.spectrum.is_some());
        if !enough {
            flags.push(SignalFlag::InsufficientData);
        }
        let mut spectrum = None;
        let mut band_power = None;
        if enough && fft_updated {
            let df = self.fs / self.config.window_samples as f64;
            let channels: Vec<_> = self
                .states
                .iter()
                .filter_map(|s| s.spectrum.clone())
                .collect();
            let mut powers = Vec::new();
            for (ch, (state, spec)) in self.states.iter_mut().zip(&channels).enumerate() {
                // Relative power denominator is all non-DC filtered PSD bins, not the sum of overlapping bands.
                let total = spec.filtered_psd.iter().skip(1).sum::<f64>() * df;
                for (i, band) in self.config.bands.iter().enumerate() {
                    let absolute = spec
                        .filtered_psd
                        .iter()
                        .enumerate()
                        .filter(|(k, _)| {
                            *k as f64 * df >= band.low_hz && (*k as f64 * df) < band.high_hz
                        })
                        .map(|(_, p)| p)
                        .sum::<f64>()
                        * df;
                    let smoothed = state.smoothed[i].map_or(absolute, |last| {
                        self.config.smoothing * absolute + (1. - self.config.smoothing) * last
                    });
                    state.smoothed[i] = Some(smoothed);
                    let baseline = self
                        .config
                        .baseline
                        .get(ch)
                        .and_then(|r| r.get(i))
                        .map(|base| (absolute / base - 1.) * 100.);
                    powers.push(BandPower {
                        channel: spec.channel.clone(),
                        band: band.name.clone(),
                        absolute,
                        relative: if total > 0. { absolute / total } else { 0. },
                        smoothed,
                        baseline_change_pct: baseline,
                        asymmetry: None,
                    });
                }
            }
            if channels.len() == 2
                && synchronized
                && self.aligned_samples >= self.config.window_samples
                && channels[0].timestamp_ns.abs_diff(channels[1].timestamp_ns) as f64 / 1e6
                    <= self.config.sync_tolerance_ms
                && !flags
                    .iter()
                    .any(|f| matches!(f, SignalFlag::LostSample | SignalFlag::InvalidPacket))
            {
                let n = self.config.bands.len();
                for i in 0..n {
                    let a = powers[i].absolute;
                    let b = powers[i + n].absolute;
                    if a + b > 0. {
                        let asym = (a - b) / (a + b);
                        powers[i].asymmetry = Some(asym);
                        powers[i + n].asymmetry = Some(asym);
                    }
                }
            }
            spectrum = Some(SpectrumSnapshot {
                timestamp_ns: block
                    .channels
                    .iter()
                    .filter_map(|c| c.samples.last().map(|s| s.timestamp_ns))
                    .max()
                    .unwrap_or(block.started_at),
                frequencies: (0..=self.config.window_samples / 2)
                    .map(|k| k as f64 * df)
                    .collect(),
                channels,
            });
            band_power = Some(powers);
        }
        let mut unique = Vec::new();
        for flag in flags {
            if !unique.contains(&flag) {
                unique.push(flag);
            }
        }
        Ok(ProcessedSignalBlock {
            sequence: block.sequence,
            started_at: block.started_at,
            sample_rate_hz: self.fs,
            raw_channels: block.channels,
            filtered_channels,
            channel_quality: qualities,
            spectrum,
            band_power,
            flags: unique,
            lost_samples,
            invalid_samples,
            outliers,
            skew_ms: skew,
        })
    }
}
fn psd(
    fft: &Arc<dyn Fft<f64>>,
    hann: &[f64],
    values: impl Iterator<Item = f64>,
    fs: f64,
) -> Vec<f64> {
    let data: Vec<f64> = values.collect();
    let mean = data.iter().sum::<f64>() / data.len() as f64;
    let mut bins: Vec<Complex<f64>> = data
        .iter()
        .zip(hann)
        .map(|(x, w)| Complex::new((x - mean) * w, 0.))
        .collect();
    fft.process(&mut bins);
    let scale = fs * hann.iter().map(|w| w * w).sum::<f64>();
    let n = hann.len();
    bins[..=n / 2]
        .iter()
        .enumerate()
        .map(|(i, v)| {
            v.norm_sqr() / scale
                * if i == 0 || (n.is_multiple_of(2) && i == n / 2) {
                    1.
                } else {
                    2.
                }
        })
        .collect()
}
