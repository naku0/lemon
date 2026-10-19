//! Stateful causal biquads and one-sided Hann-window PSD. No terminal dependencies.
pub use core_types::WindowFunction;
pub use spectrum::{integrate_band_power, one_sided_psd, PsdResult, PSD_ALGORITHM_VERSION};
mod spectrum;
pub mod windows;
use core_types::{
    BandPower, ChannelSpectrum, Config, FilteredChannel, ProcessedSampleMetadata,
    ProcessedSignalBlock, ProcessingConfig, RawSignalBlock, Result, RingBuffer, SignalFlag,
    SignalQuality, SpectrumSnapshot,
};
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
#[derive(Clone, Copy, PartialEq, Eq)]
enum ResetReason {
    ContinuityBreak,
    ArtifactRejected,
}

struct ChannelState {
    filters: Vec<Biquad>,
    raw: RingBuffer<f64>,
    quality_raw: RingBuffer<f64>,
    quality_since_fft: usize,
    quality_mains_ratio: f64,
    filtered: RingBuffer<f64>,
    since_fft: usize,
    last_seen: Option<(u64, u64)>,
    last_valid_raw: Option<f64>,
    warmup_remaining: usize,
    segment_id: u64,
    spectrum: Option<ChannelSpectrum>,
    smoothed: Vec<Option<f64>>,
}
impl ChannelState {
    fn reset_signal(&mut self, warmup_samples: usize, reason: ResetReason) {
        if reason == ResetReason::ContinuityBreak {
            self.last_valid_raw = None;
        }
        for f in &mut self.filters {
            f.reset();
        }
        self.quality_raw.clear();
        self.quality_since_fft = 0;
        self.quality_mains_ratio = 0.0;
        self.raw.clear();
        self.filtered.clear();
        self.since_fft = 0;
        self.spectrum = None;
        self.smoothed.fill(None);
        self.warmup_remaining = warmup_samples;
        self.segment_id = self.segment_id.saturating_add(1);
    }
    fn clear_spectral(&mut self) {
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
    warmup_samples: usize,
    last_block: Option<u64>,
    input_block: Option<u64>,
    next_internal_block: u64,
    channel_ids: Option<Vec<(String, String)>>,
    source_id: Option<String>,
    aligned_samples: usize,
}
impl Processor {
    pub fn new(config: &Config) -> Result<Self> {
        config.validate()?;
        let p = &config.processing;
        let fs = config.source.sample_rate_hz;
        let warmup_samples = (p.filter_warmup_seconds * fs).ceil() as usize;
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
                    quality_raw: RingBuffer::new(p.window_samples),
                    quality_since_fft: 0,
                    quality_mains_ratio: 0.0,
                    filtered: RingBuffer::new(p.window_samples),
                    since_fft: 0,
                    last_seen: None,
                    last_valid_raw: None,
                    warmup_remaining: warmup_samples,
                    segment_id: 0,
                    spectrum: None,
                    smoothed: vec![None; p.bands.len()],
                }
            })
            .collect();
        Ok(Self {
            config: p.clone(),
            fs,
            states,
            warmup_samples,
            last_block: None,
            input_block: None,
            next_internal_block: 0,
            channel_ids: None,
            source_id: None,
            aligned_samples: 0,
        })
    }
    /// Start a new acquisition/replay segment without inheriting its raw reference.
    pub fn reset_after_continuity_break(&mut self) {
        for state in &mut self.states {
            state.reset_signal(self.warmup_samples, ResetReason::ContinuityBreak);
            state.last_seen = None;
        }
        self.last_block = None;
        self.input_block = None;
        self.channel_ids = None;
        self.aligned_samples = 0;
        self.source_id = None;
    }

    /// Process a block; the returned spectrum is the latest display snapshot.
    pub fn process(&mut self, block: RawSignalBlock) -> Result<ProcessedSignalBlock> {
        self.process_with_frames(block, |_| {})
    }

    /// Stream every hop update to `visit` without retaining all PSDs in memory.
    /// The callback receives a single acquisition step; inspect its spectrum and bands.
    /// Filtering, quality decisions and EMA are independent of caller block boundaries.
    pub fn process_with_frames(
        &mut self,
        block: RawSignalBlock,
        mut visit: impl FnMut(&ProcessedSignalBlock),
    ) -> Result<ProcessedSignalBlock> {
        if block
            .channels
            .iter()
            .any(|channel| channel.samples.len() > 4096)
        {
            return Err("processing: block exceeds 4096 samples".into());
        }
        if self
            .source_id
            .as_ref()
            .is_some_and(|source| source != &block.source)
        {
            self.reset_after_continuity_break();
        }
        self.source_id = Some(block.source.clone());
        if self
            .input_block
            .is_some_and(|previous| block.sequence <= previous)
        {
            return Err("processing: non-monotonic block sequence".into());
        }
        let gap = self
            .input_block
            .is_some_and(|previous| block.sequence > previous.saturating_add(1));
        self.input_block = Some(block.sequence);
        let count = block
            .channels
            .iter()
            .map(|channel| channel.samples.len())
            .max()
            .unwrap_or(0);
        let mut combined: Option<ProcessedSignalBlock> = None;
        for index in 0..count.max(1) {
            let mut part = RawSignalBlock {
                sequence: block.sequence,
                started_at: block.started_at,
                sample_rate_hz: block.sample_rate_hz,
                source: block.source.clone(),
                synchronized_clock: block.synchronized_clock,
                // Block-level continuity faults describe the boundary before
                // this block. Applying them to every internal sample would
                // reject the whole block and restart warmup repeatedly.
                flags: block
                    .flags
                    .iter()
                    .copied()
                    .filter(|flag| {
                        index == 0
                            || !matches!(
                                flag,
                                SignalFlag::LostSample
                                    | SignalFlag::InvalidPacket
                                    | SignalFlag::Disconnected
                            )
                    })
                    .collect(),
                channels: block
                    .channels
                    .iter()
                    .map(|channel| core_types::ChannelSamples {
                        id: channel.id.clone(),
                        device: channel.device.clone(),
                        samples: Vec::new(),
                    })
                    .collect(),
            };
            part.sequence = self.next_internal_block;
            self.next_internal_block = self
                .next_internal_block
                .checked_add(1)
                .ok_or("processing: internal sequence overflow")?;
            for (target, source) in part.channels.iter_mut().zip(&block.channels) {
                target.samples = source.samples.get(index).cloned().into_iter().collect();
            }
            if gap && index == 0 {
                part.flags.push(SignalFlag::LostSample);
            }
            let out = self.process_step(part)?;
            if out.spectrum.is_some() {
                visit(&out);
            }
            if let Some(all) = &mut combined {
                for (target, source) in all.raw_channels.iter_mut().zip(&out.raw_channels) {
                    target.samples.extend_from_slice(&source.samples);
                }
                for (target, source) in all.filtered_channels.iter_mut().zip(&out.filtered_channels)
                {
                    target.samples.extend_from_slice(&source.samples);
                }
                for (target, source) in all.sample_metadata.iter_mut().zip(&out.sample_metadata) {
                    target.extend_from_slice(source);
                }
                for (quality, current) in all.channel_quality.iter_mut().zip(&out.channel_quality) {
                    let rank = |q: SignalQuality| match q {
                        SignalQuality::Good => 0,
                        SignalQuality::Unknown => 1,
                        SignalQuality::Acceptable => 2,
                        SignalQuality::Poor => 3,
                        SignalQuality::Disconnected => 4,
                    };
                    if rank(*current) > rank(*quality) {
                        *quality = *current;
                    }
                }
                all.lost_samples = all.lost_samples.saturating_add(out.lost_samples);
                all.invalid_samples += out.invalid_samples;
                all.outliers += out.outliers;
                all.skew_ms = match (all.skew_ms, out.skew_ms) {
                    (Some(a), Some(b)) => Some(a.max(b)),
                    (a, b) => a.or(b),
                };
                // A reset invalidates a cached display spectrum as well.
                if out.spectrum.is_some()
                    || out
                        .sample_metadata
                        .iter()
                        .flatten()
                        .any(|sample| !sample.sample_valid)
                {
                    all.spectrum = out.spectrum;
                    all.band_power = out.band_power;
                }
                for flag in out.flags {
                    if !all.flags.contains(&flag) {
                        all.flags.push(flag);
                    }
                }
            } else {
                combined = Some(out);
            }
        }
        let mut output = combined.ok_or("processing: empty processing result")?;
        output.sequence = block.sequence;
        output.started_at = block.started_at;
        Ok(output)
    }

    fn process_step(&mut self, block: RawSignalBlock) -> Result<ProcessedSignalBlock> {
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
        let block_gap = self
            .last_block
            .is_some_and(|s| block.sequence > s.saturating_add(1));
        if block_gap {
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
        let mut sample_metadata = Vec::new();
        let mut lost_samples = 0u64;
        let mut invalid_samples = 0;
        let mut outliers = 0;
        let mut fft_updated = false;
        for (channel, state) in block.channels.iter().zip(&mut self.states) {
            if channel.samples.is_empty()
                && block.flags.iter().any(|flag| {
                    matches!(
                        flag,
                        SignalFlag::LostSample
                            | SignalFlag::InvalidPacket
                            | SignalFlag::Disconnected
                    )
                })
            {
                state.reset_signal(self.warmup_samples, ResetReason::ContinuityBreak);
            }
            let mut filtered = Vec::with_capacity(channel.samples.len());
            let mut metadata = Vec::with_capacity(channel.samples.len());
            let mut bad = 0usize;
            let mut gap = false;
            let mut warmup_seen = false;
            let mut channel_fft_updated = false;
            for sample in &channel.samples {
                let mut sample_flags = sample.flags.clone();
                let mut invalid_packet = false;
                sample_flags.extend(block.flags.iter().copied().filter(|flag| {
                    matches!(
                        flag,
                        SignalFlag::Disconnected
                            | SignalFlag::LostSample
                            | SignalFlag::InvalidPacket
                            | SignalFlag::Outlier
                            | SignalFlag::Saturation
                            | SignalFlag::LowQuality
                    )
                }));
                if let Some((sequence, timestamp_ns)) = state.last_seen {
                    if sample.sequence <= sequence || sample.timestamp_ns <= timestamp_ns {
                        invalid_packet = true;
                    } else if sample.sequence > sequence.saturating_add(1) {
                        let lost = sample.sequence - sequence - 1;
                        lost_samples = lost_samples.saturating_add(lost);
                        gap = true;
                    }
                }
                if !invalid_packet {
                    if let Some((_, timestamp)) = state.last_seen {
                        if ((sample.timestamp_ns.saturating_sub(timestamp) as f64) - 1e9 / self.fs)
                            .abs()
                            > 0.51e9 / self.fs
                        {
                            gap = true;
                        }
                    }
                    if gap {
                        state.reset_signal(self.warmup_samples, ResetReason::ContinuityBreak);
                        flags.push(SignalFlag::LostSample);
                    }
                    state.last_seen = Some((sample.sequence, sample.timestamp_ns));
                }
                if !sample.value.is_finite() || sample.value.abs() > 1e100 {
                    invalid_packet = true;
                }
                if invalid_packet {
                    sample_flags.push(SignalFlag::InvalidPacket);
                }
                if sample_flags.contains(&SignalFlag::InvalidPacket) {
                    invalid_samples += 1;
                }
                if sample_flags.iter().any(|flag| {
                    matches!(
                        flag,
                        SignalFlag::InvalidPacket
                            | SignalFlag::LostSample
                            | SignalFlag::Disconnected
                    )
                }) {
                    state.last_valid_raw = None;
                }
                if sample.value.is_finite() && sample.value.abs() > self.config.expected_abs_max {
                    sample_flags.push(SignalFlag::Saturation);
                }
                if sample.value.is_finite()
                    && state
                        .last_valid_raw
                        .is_some_and(|last| (sample.value - last).abs() > self.config.outlier_step)
                {
                    sample_flags.push(SignalFlag::Outlier);
                }
                if sample_flags.contains(&SignalFlag::Outlier) {
                    outliers += 1;
                }
                sample_flags.sort_by_key(|flag| *flag as u8);
                sample_flags.dedup();
                let rejected = sample_flags.iter().any(|flag| {
                    matches!(
                        flag,
                        SignalFlag::InvalidPacket
                            | SignalFlag::LostSample
                            | SignalFlag::Outlier
                            | SignalFlag::Saturation
                            | SignalFlag::Disconnected
                    )
                });
                if rejected {
                    bad += 1;
                    let reason = if sample_flags.iter().any(|flag| {
                        matches!(
                            flag,
                            SignalFlag::InvalidPacket
                                | SignalFlag::LostSample
                                | SignalFlag::Disconnected
                        )
                    }) {
                        ResetReason::ContinuityBreak
                    } else {
                        ResetReason::ArtifactRejected
                    };
                    state.reset_signal(self.warmup_samples, reason);
                    flags.extend(sample_flags.iter().copied());
                    filtered.push(None);
                    metadata.push(ProcessedSampleMetadata {
                        segment_id: state.segment_id,
                        sample_valid: false,
                        filter_warmup: false,
                        artifact_rejected: true,
                        flags: sample_flags,
                    });
                    continue;
                }

                state.last_valid_raw = Some(sample.value);
                let mut y = sample.value;
                for filter in &mut state.filters {
                    y = filter.apply(y);
                }
                if !y.is_finite() {
                    invalid_samples += 1;
                    bad += 1;
                    sample_flags.push(SignalFlag::InvalidPacket);
                    flags.push(SignalFlag::InvalidPacket);
                    state.reset_signal(self.warmup_samples, ResetReason::ContinuityBreak);
                    filtered.push(None);
                    metadata.push(ProcessedSampleMetadata {
                        segment_id: state.segment_id,
                        sample_valid: false,
                        filter_warmup: false,
                        artifact_rejected: true,
                        flags: sample_flags,
                    });
                    continue;
                }

                let in_warmup =
                    state.warmup_remaining > 0 || sample_flags.contains(&SignalFlag::FilterWarmup);
                // Quality evidence is independent of the eligible spectral buffer.
                // Rejecting a low-quality window must not erase its cause.
                if !in_warmup {
                    state.quality_raw.push(sample.value);
                    state.quality_since_fft += 1;
                    if state.quality_raw.len() == self.config.window_samples
                        && state.quality_since_fft >= self.config.hop_samples
                    {
                        let values = state.quality_raw.iter().copied().collect::<Vec<_>>();
                        let psd = one_sided_psd(&values, self.fs, self.config.window_function)?;
                        let total = psd.values.iter().skip(1).sum::<f64>();
                        let mains = psd
                            .frequencies_hz
                            .iter()
                            .zip(&psd.values)
                            .filter(|(frequency, _)| {
                                (**frequency - self.config.notch_hz).abs() <= 2.0
                            })
                            .map(|(_, power)| power)
                            .sum::<f64>();
                        state.quality_mains_ratio = if total > 0.0 { mains / total } else { 0.0 };
                        state.quality_since_fft = 0;
                    }
                }
                if in_warmup {
                    if !state.raw.is_empty() {
                        state.clear_spectral();
                    }
                    state.warmup_remaining = state.warmup_remaining.saturating_sub(1);
                    warmup_seen = true;
                    if !sample_flags.contains(&SignalFlag::FilterWarmup) {
                        sample_flags.push(SignalFlag::FilterWarmup);
                    }
                    flags.push(SignalFlag::FilterWarmup);
                } else if self.config.reject_low_quality_windows
                    && sample_flags.contains(&SignalFlag::LowQuality)
                {
                    state.clear_spectral();
                } else {
                    state.raw.push(sample.value);
                    state.filtered.push(y);
                    state.since_fft += 1;
                    if state.raw.len() == self.config.window_samples
                        && (state.spectrum.is_none() || state.since_fft >= self.config.hop_samples)
                    {
                        let raw_values = state.raw.iter().copied().collect::<Vec<_>>();
                        let filtered_values = state.filtered.iter().copied().collect::<Vec<_>>();
                        let raw = one_sided_psd(&raw_values, self.fs, self.config.window_function)?;
                        let filtered_psd_result =
                            one_sided_psd(&filtered_values, self.fs, self.config.window_function)?;
                        let total = raw.values.iter().skip(1).sum::<f64>();
                        let mains = raw
                            .frequencies_hz
                            .iter()
                            .zip(&raw.values)
                            .filter(|(frequency, _)| {
                                (**frequency - self.config.notch_hz).abs() <= 2.0
                            })
                            .map(|(_, power)| power)
                            .sum::<f64>();
                        state.spectrum = Some(ChannelSpectrum {
                            channel: channel.id.clone(),
                            timestamp_ns: sample.timestamp_ns,
                            raw_psd: raw.values,
                            filtered_psd: filtered_psd_result.values,
                            mains_ratio: if total > 0.0 { mains / total } else { 0.0 },
                            segment_id: state.segment_id,
                            sample_count: self.config.window_samples,
                            frequency_resolution_hz: raw.resolution_hz,
                            window_function: self.config.window_function,
                        });
                        state.since_fft = 0;
                        channel_fft_updated = true;
                    }
                }
                filtered.push(Some(y));
                metadata.push(ProcessedSampleMetadata {
                    segment_id: state.segment_id,
                    sample_valid: !(in_warmup
                        || self.config.reject_low_quality_windows
                            && sample_flags.contains(&SignalFlag::LowQuality)),
                    filter_warmup: in_warmup,
                    artifact_rejected: false,
                    flags: sample_flags,
                });
            }
            let quality_ready = state.quality_raw.len() >= self.config.window_samples.min(32);
            let flat = quality_ready && {
                let mean = state.quality_raw.iter().sum::<f64>() / state.quality_raw.len() as f64;
                state
                    .quality_raw
                    .iter()
                    .map(|value| (value - mean).powi(2))
                    .sum::<f64>()
                    / (state.quality_raw.len() as f64)
                    < self.config.flat_stddev.powi(2)
            };
            let mains = state.quality_mains_ratio > self.config.mains_ratio_limit;
            let quality = if block.flags.contains(&SignalFlag::Disconnected) {
                SignalQuality::Disconnected
            } else if channel.samples.is_empty() {
                SignalQuality::Unknown
            } else if flat || bad > channel.samples.len() / 10 {
                SignalQuality::Poor
            } else if bad > 0
                || mains
                || metadata
                    .iter()
                    .any(|item| item.flags.contains(&SignalFlag::LowQuality))
            {
                SignalQuality::Acceptable
            } else if warmup_seen || !quality_ready {
                SignalQuality::Unknown
            } else {
                SignalQuality::Good
            };
            if matches!(quality, SignalQuality::Poor | SignalQuality::Acceptable) {
                flags.push(SignalFlag::LowQuality);
                for item in &mut metadata {
                    if item.sample_valid {
                        item.flags.push(SignalFlag::LowQuality);
                        if self.config.reject_low_quality_windows {
                            item.sample_valid = false;
                        }
                    }
                }
                if self.config.reject_low_quality_windows {
                    state.clear_spectral();
                    channel_fft_updated = false;
                }
            }
            fft_updated |= channel_fft_updated;
            qualities.push(quality);
            filtered_channels.push(FilteredChannel {
                id: channel.id.clone(),
                samples: filtered,
            });
            sample_metadata.push(metadata);
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
        let enough = self.states.iter().any(|s| s.spectrum.is_some());
        if !enough {
            flags.push(SignalFlag::InsufficientData);
        }
        let mut spectrum = None;
        let mut band_power = None;
        if enough && fft_updated {
            let channels: Vec<_> =
                self.states
                    .iter()
                    .zip(&block.channels)
                    .filter_map(|(state, channel)| {
                        state
                            .spectrum
                            .as_ref()
                            .filter(|spectrum| {
                                channel.samples.last().is_some_and(|sample| {
                                    sample.timestamp_ns == spectrum.timestamp_ns
                                })
                            })
                            .cloned()
                    })
                    .collect();
            let mut powers = Vec::new();
            for spec in &channels {
                let ch = block
                    .channels
                    .iter()
                    .position(|channel| channel.id == spec.channel)
                    .ok_or("processing: spectral channel missing")?;
                let state = &mut self.states[ch];
                // Relative power denominator is all non-DC filtered PSD bins, not the sum of overlapping bands.
                let psd = PsdResult {
                    frequencies_hz: (0..spec.filtered_psd.len())
                        .map(|index| index as f64 * spec.frequency_resolution_hz)
                        .collect(),
                    values: spec.filtered_psd.clone(),
                    resolution_hz: spec.frequency_resolution_hz,
                };
                let total =
                    spec.filtered_psd.iter().skip(1).sum::<f64>() * spec.frequency_resolution_hz;
                for (i, band) in self.config.bands.iter().enumerate() {
                    let absolute = integrate_band_power(&psd, band.low_hz, band.high_hz)?;
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
                        segment_id: spec.segment_id,
                        quality_status: qualities
                            .get(ch)
                            .copied()
                            .unwrap_or(SignalQuality::Unknown),
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
                timestamp_ns: channels
                    .iter()
                    .map(|c| c.timestamp_ns)
                    .max()
                    .unwrap_or(block.started_at),
                frequencies: (0..=self.config.window_samples / 2)
                    .map(|k| k as f64 * self.fs / self.config.window_samples as f64)
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
            sample_metadata,
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

#[cfg(test)]
mod tests {
    use super::*;
    use core_types::{ChannelSamples, Sample};

    fn config(sample_rate_hz: f64) -> Config {
        let mut config = Config::default();
        config.source.channels = 1;
        config.source.sample_rate_hz = sample_rate_hz;
        config.source.block_size = 64;
        config.source.mains = 0.0;
        config.processing.high_pass_hz = 1.0;
        config.processing.low_pass_hz = sample_rate_hz / 2.0 - 1.0;
        config.processing.notch = false;
        config.processing.window_samples = 8;
        config.processing.hop_samples = 4;
        config.processing.filter_warmup_seconds = 0.0;
        config.processing.expected_abs_max = 500.0;
        config.processing.outlier_step = 50.0;
        config.processing.reject_low_quality_windows = false;
        config
    }

    fn block(values: &[f64], sample_rate_hz: f64, first_sequence: u64) -> RawSignalBlock {
        RawSignalBlock {
            sequence: if values.is_empty() {
                0
            } else {
                first_sequence / values.len() as u64
            },
            started_at: first_sequence * 1_000_000_000,
            sample_rate_hz,
            source: "test".into(),
            synchronized_clock: true,
            channels: vec![ChannelSamples {
                id: "A0".into(),
                device: "test".into(),
                samples: values
                    .iter()
                    .enumerate()
                    .map(|(index, value)| Sample {
                        sequence: first_sequence + index as u64,
                        timestamp_ns: ((first_sequence + index as u64) as f64 / sample_rate_hz
                            * 1e9) as u64,
                        value: *value,
                        flags: Vec::new(),
                    })
                    .collect(),
            }],
            flags: Vec::new(),
        }
    }

    #[test]
    fn rejected_artifacts_never_enter_filters_and_outlier_is_not_reference() {
        let mut config = config(100.0);
        config.processing.outlier_step = 10.0;
        let mut processor = Processor::new(&config).expect("processor");
        let output = processor
            .process(block(
                &[
                    1.0,
                    100.0,
                    2.0,
                    f64::NAN,
                    3.0,
                    f64::INFINITY,
                    4.0,
                    1000.0,
                    5.0,
                ],
                100.0,
                0,
            ))
            .expect("process");
        let filtered = &output.filtered_channels[0].samples;
        assert!(filtered[0].is_some());
        assert!(filtered[1].is_none());
        assert!(
            filtered[2].is_some(),
            "outlier must not become the next reference"
        );
        assert!(filtered[3].is_none());
        assert!(filtered[4].is_some());
        assert!(filtered[5].is_none());
        assert!(filtered[6].is_some());
        assert!(filtered[7].is_none());
        assert!(filtered[8].is_some());
        assert!(output.sample_metadata[0][1].artifact_rejected);
        assert!(output.sample_metadata[0][7].artifact_rejected);
        assert!(output.sample_metadata[0][1]
            .flags
            .contains(&SignalFlag::Outlier));
        assert!(output.sample_metadata[0][7]
            .flags
            .contains(&SignalFlag::Saturation));
        assert!(output.sample_metadata[0][2].segment_id > output.sample_metadata[0][0].segment_id);
    }

    #[test]
    fn non_monotonic_sequence_and_timestamp_are_rejected() {
        let config = config(100.0);
        let mut processor = Processor::new(&config).expect("processor");
        let mut input = block(&[1.0, 2.0, 3.0, 4.0], 100.0, 0);
        input.channels[0].samples[2].sequence = 1;
        input.channels[0].samples[3].timestamp_ns = 1;
        let output = processor.process(input).expect("process");
        assert!(output.filtered_channels[0].samples[2].is_none());
        assert!(output.filtered_channels[0].samples[3].is_none());
        assert!(output.sample_metadata[0][2]
            .flags
            .contains(&SignalFlag::InvalidPacket));
        assert!(output.sample_metadata[0][3]
            .flags
            .contains(&SignalFlag::InvalidPacket));
    }

    #[test]
    fn psd_appears_only_after_warmup_and_a_complete_window() {
        let mut config = config(64.0);
        config.processing.filter_warmup_seconds = 2.0 / 64.0;
        let values = (0..10)
            .map(|index| (std::f64::consts::TAU * 8.0 * index as f64 / 64.0).sin())
            .collect::<Vec<_>>();
        let mut processor = Processor::new(&config).expect("processor");
        let output = processor.process(block(&values, 64.0, 0)).expect("process");
        assert!(output.sample_metadata[0][..2]
            .iter()
            .all(|sample| sample.filter_warmup));
        assert!(output.spectrum.is_some());
        assert!(output.band_power.is_some());
    }

    #[test]
    fn segment_break_resets_spectral_window_and_ema() {
        let mut config = config(64.0);
        config.processing.hop_samples = 8;
        config.processing.smoothing = 0.1;
        let signal = |amplitude: f64, count: usize| {
            (0..count)
                .map(|index| amplitude * (std::f64::consts::TAU * 8.0 * index as f64 / 64.0).sin())
                .collect::<Vec<_>>()
        };
        let mut processor = Processor::new(&config).expect("processor");
        let first = processor
            .process(block(&signal(1.0, 8), 64.0, 0))
            .expect("first");
        assert!(first.band_power.is_some());
        let first_segment = first.spectrum.as_ref().expect("first spectrum").channels[0].segment_id;

        let mut values = vec![0.0];
        values.extend(signal(2.0, 8));
        let mut second_block = block(&values, 64.0, 8);
        second_block.sequence = 1;
        second_block.channels[0].samples[0]
            .flags
            .push(SignalFlag::LostSample);
        let second = processor.process(second_block).expect("second");
        let powers = second.band_power.expect("post-break spectrum");
        assert!(powers
            .iter()
            .all(|power| (power.smoothed - power.absolute).abs() < 1e-12));
        assert!(powers.iter().all(|power| power.segment_id > first_segment));
    }

    #[test]
    fn warmup_runs_at_start_and_after_reset_without_spectral_output() {
        let mut config = config(64.0);
        config.processing.filter_warmup_seconds = 0.03125;
        config.processing.outlier_step = 20.0;
        let mut processor = Processor::new(&config).expect("processor");
        let first = processor
            .process(block(&[1.0, 2.0, 3.0], 64.0, 0))
            .expect("first");
        assert_eq!(
            first.sample_metadata[0]
                .iter()
                .map(|sample| sample.filter_warmup)
                .collect::<Vec<_>>(),
            [true, true, false]
        );
        assert!(first.spectrum.is_none());
        let second = processor
            .process(block(&[100.0, 4.0, 5.0], 64.0, 3))
            .expect("second");
        assert!(second.filtered_channels[0].samples[0].is_none());
        assert!(second.sample_metadata[0][1].filter_warmup);
        assert!(second.sample_metadata[0][2].filter_warmup);
        assert!(second.spectrum.is_none());
    }

    #[test]
    fn psd_peak_parseval_centering_and_nyquist_are_correct() {
        let fs = 128.0;
        for n in [127usize, 128] {
            let samples = (0..n)
                .map(|index| (std::f64::consts::TAU * 8.0 * index as f64 / fs).sin() + 4.0)
                .collect::<Vec<_>>();
            let psd = one_sided_psd(&samples, fs, WindowFunction::HannSymmetric).expect("PSD");
            assert_eq!(psd.values.len(), n / 2 + 1);
            assert!(psd.values.iter().all(|value| value.is_finite()));
            let peak_power = psd.values.iter().copied().fold(0.0, f64::max);
            assert!(psd.values[0] < peak_power * 0.01);
            let peak = psd
                .values
                .iter()
                .enumerate()
                .skip(1)
                .max_by(|left, right| left.1.total_cmp(right.1))
                .map(|(index, _)| psd.frequencies_hz[index])
                .expect("peak");
            assert!((peak - 8.0).abs() <= psd.resolution_hz);
            let integrated = psd.values.iter().sum::<f64>() * psd.resolution_hz;
            assert!((integrated - 0.5).abs() < 0.08, "n={n}, power={integrated}");
        }
    }

    #[test]
    fn all_hop_frames_and_ema_are_block_invariant_with_artifacts() {
        let mut config = config(128.0);
        config.processing.window_samples = 64;
        config.processing.hop_samples = 16;
        config.processing.filter_warmup_seconds = 8.0 / 128.0;
        config.processing.reject_low_quality_windows = true;
        let values = (0..512)
            .map(|i| (std::f64::consts::TAU * 10.0 * i as f64 / 128.0).sin())
            .collect::<Vec<_>>();
        let mut input = block(&values, 128.0, 0);
        input.channels[0].samples[120].value = 1000.0;
        input.channels[0].samples[179]
            .flags
            .push(SignalFlag::LowQuality);
        for sample in &mut input.channels[0].samples[250..] {
            sample.sequence += 2;
            sample.timestamp_ns += 15_625_000;
        }
        let run = |size: usize| {
            let mut processor = Processor::new(&config).expect("processor");
            let mut frames = Vec::new();
            let mut samples = Vec::new();
            for (index, chunk) in input.channels[0].samples.chunks(size).enumerate() {
                let mut part = input.clone();
                part.sequence = index as u64;
                part.channels[0].samples = chunk.to_vec();
                let output = processor
                    .process_with_frames(part, |output| {
                        let spectrum = output.spectrum.as_ref().expect("frame");
                        let powers = output.band_power.as_ref().expect("bands");
                        frames.push((
                            spectrum.timestamp_ns,
                            spectrum.channels[0].segment_id,
                            spectrum.channels[0].filtered_psd.clone(),
                            powers
                                .iter()
                                .map(|band| (band.absolute, band.smoothed))
                                .collect::<Vec<_>>(),
                        ));
                    })
                    .expect("process");
                for (filtered, metadata) in output.filtered_channels[0]
                    .samples
                    .iter()
                    .zip(&output.sample_metadata[0])
                {
                    samples.push((
                        *filtered,
                        metadata.segment_id,
                        metadata.sample_valid,
                        metadata.flags.clone(),
                    ));
                }
            }
            (frames, samples)
        };
        let expected = run(1);
        assert!(!expected.0.is_empty());
        for size in [7, 64, 512] {
            assert_eq!(run(size), expected, "block size {size}");
        }
        let mut clean = Processor::new(&config).expect("processor");
        let mut count = 0;
        clean
            .process_with_frames(block(&values, 128.0, 0), |_| count += 1)
            .expect("clean");
        assert_eq!(count, (512 - 8 - 64) / 16 + 1);
    }

    #[test]
    fn consecutive_outliers_do_not_replace_last_valid_reference() {
        let mut config = config(128.0);
        config.processing.outlier_step = 10.0;
        let output = Processor::new(&config)
            .expect("processor")
            .process(block(&[1.0, 100.0, 101.0, 2.0], 128.0, 0))
            .expect("process");
        assert!(output.filtered_channels[0].samples[1..3]
            .iter()
            .all(Option::is_none));
        assert!(output.filtered_channels[0].samples[3].is_some());
    }

    #[test]
    fn psd_matches_direct_dft_for_even_and_odd_windows() {
        for n in [8, 9] {
            let samples = (0..n).map(|i| (i * i % 7) as f64).collect::<Vec<_>>();
            let mean = samples.iter().map(|value| value / n as f64).sum::<f64>();
            let weights = (0..n)
                .map(|i| 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / (n - 1) as f64).cos())
                .collect::<Vec<_>>();
            let scale = 100.0 * weights.iter().map(|w| w * w).sum::<f64>();
            let psd = one_sided_psd(&samples, 100.0, WindowFunction::HannSymmetric).expect("psd");
            for (k, actual) in psd.values.iter().enumerate() {
                let mut real = 0.0;
                let mut imaginary = 0.0;
                for i in 0..n {
                    let phase = std::f64::consts::TAU * (k * i) as f64 / n as f64;
                    let value = (samples[i] - mean) * weights[i];
                    real += value * phase.cos();
                    imaginary -= value * phase.sin();
                }
                let factor = if k == 0 || (n % 2 == 0 && k == n / 2) {
                    1.0
                } else {
                    2.0
                };
                assert!(
                    (actual - factor * (real * real + imaginary * imaginary) / scale).abs() < 1e-12
                );
            }
        }
    }

    #[test]
    fn rejecting_flat_windows_does_not_erase_quality_evidence() {
        let mut config = config(128.0);
        config.processing.window_samples = 64;
        config.processing.hop_samples = 16;
        config.processing.reject_low_quality_windows = true;
        let mut processor = Processor::new(&config).expect("processor");
        for index in 0..256 {
            let output = processor
                .process(block(&[10.0], 128.0, index))
                .expect("flat input");
            let expected = if index < 31 {
                SignalQuality::Unknown
            } else {
                SignalQuality::Poor
            };
            assert_eq!(output.channel_quality[0], expected, "sample {index}");
            assert!(output.spectrum.is_none());
        }
        let mut recovered = false;
        for index in 256..512 {
            let value = 10.0 + (std::f64::consts::TAU * 10.0 * index as f64 / 128.0).sin();
            let output = processor
                .process(block(&[value], 128.0, index))
                .expect("varying input");
            if index >= 320 {
                assert_eq!(output.channel_quality[0], SignalQuality::Good);
            }
            recovered |= output.spectrum.is_some();
        }
        assert!(
            recovered,
            "quality and spectral updates must recover with new evidence"
        );
    }

    #[test]
    fn mains_quality_evidence_survives_spectral_rejection() {
        let mut config = config(256.0);
        config.processing.window_samples = 128;
        config.processing.hop_samples = 16;
        config.processing.reject_low_quality_windows = true;
        let mut processor = Processor::new(&config).expect("processor");
        for index in 0..512 {
            let value = 10.0 * (std::f64::consts::TAU * 50.0 * index as f64 / 256.0).sin();
            let output = processor
                .process(block(&[value], 256.0, index))
                .expect("input");
            if index >= 127 {
                assert_eq!(
                    output.channel_quality[0],
                    SignalQuality::Acceptable,
                    "sample {index}"
                );
                assert!(output.spectrum.is_none());
            }
        }
    }

    #[test]
    fn continuity_breaks_accept_a_new_raw_level_and_resume_psd() {
        for reason in ["gap", "disconnect", "invalid", "source", "forced"] {
            let mut config = config(128.0);
            config.processing.outlier_step = 150.0;
            config.processing.filter_warmup_seconds = 2.0 / 128.0;
            let mut processor = Processor::new(&config).expect("processor");
            processor
                .process(block(&[0.0, 0.0, 0.0], 128.0, 0))
                .expect("first");
            let mut first_sequence = 3;
            let mut block_sequence = 1;
            match reason {
                "gap" => first_sequence = 5,
                "disconnect" => {
                    let mut event = block(&[], 128.0, 3);
                    event.sequence = 1;
                    event.flags.push(SignalFlag::Disconnected);
                    processor.process(event).expect("disconnect");
                    block_sequence = 2;
                }
                "invalid" => {
                    let mut event = block(&[f64::NAN], 128.0, 3);
                    event.sequence = 1;
                    processor.process(event).expect("invalid");
                    first_sequence = 4;
                    block_sequence = 2;
                }
                "forced" => processor.reset_after_continuity_break(),
                _ => {}
            }
            let mut next = block(&[200.0; 12], 128.0, first_sequence);
            next.sequence = block_sequence;
            if reason == "source" {
                next.source = "new-source".into();
                next.sequence = 0;
            }
            let output = processor.process(next).expect("recovered");
            assert!(
                output.filtered_channels[0]
                    .samples
                    .iter()
                    .all(Option::is_some),
                "{reason}"
            );
            assert!(!output.flags.contains(&SignalFlag::Outlier), "{reason}");
            assert!(output.sample_metadata[0][0].segment_id > 0, "{reason}");
            assert!(
                output.sample_metadata[0][..2]
                    .iter()
                    .all(|sample| sample.filter_warmup),
                "{reason}"
            );
            assert!(output.spectrum.is_some(), "{reason}: resumed PSD");
        }
    }

    #[test]
    fn block_continuity_flag_applies_only_to_first_internal_sample() {
        let mut config = config(128.0);
        config.processing.window_samples = 8;
        config.processing.hop_samples = 4;
        config.processing.filter_warmup_seconds = 2.0 / 128.0;
        config.processing.outlier_step = 150.0;
        let mut processor = Processor::new(&config).expect("processor");
        processor
            .process(block(&[0.0; 8], 128.0, 0))
            .expect("baseline");

        let mut recovered = block(&[200.0; 16], 128.0, 16);
        recovered.flags.push(SignalFlag::LostSample);
        recovered.flags.push(SignalFlag::LowQuality);
        let output = processor.process(recovered).expect("recovery");

        assert!(output.filtered_channels[0].samples[0].is_none());
        assert!(output.filtered_channels[0].samples[1..]
            .iter()
            .all(Option::is_some));
        assert!(output.sample_metadata[0][0]
            .flags
            .contains(&SignalFlag::LostSample));
        assert!(output.sample_metadata[0][1..]
            .iter()
            .all(|metadata| !metadata.flags.contains(&SignalFlag::LostSample)));
        assert!(output.sample_metadata[0]
            .iter()
            .all(|metadata| metadata.flags.contains(&SignalFlag::LowQuality)));
        assert!(output.spectrum.is_some());
    }

    #[test]
    fn artifact_resets_preserve_reference_for_single_and_repeated_outliers() {
        let mut config = config(128.0);
        config.processing.outlier_step = 150.0;
        for values in [
            vec![10.0, 11.0, 250.0, 12.0],
            vec![10.0, 250.0, 260.0, 270.0, 11.0],
        ] {
            let output = Processor::new(&config)
                .expect("processor")
                .process(block(&values, 128.0, 0))
                .expect("process");
            for (value, filtered) in values.iter().zip(&output.filtered_channels[0].samples) {
                assert_eq!(filtered.is_some(), *value < 200.0);
            }
        }
    }

    #[test]
    fn band_integration_uses_half_open_bounds() {
        let psd = PsdResult {
            frequencies_hz: vec![0.0, 1.0, 2.0, 3.0],
            values: vec![10.0, 1.0, 2.0, 4.0],
            resolution_hz: 1.0,
        };
        assert_eq!(integrate_band_power(&psd, 1.0, 3.0).expect("band"), 3.0);
    }
}
