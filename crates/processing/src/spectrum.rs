use core_types::{Result, WindowFunction};
use rustfft::{num_complex::Complex, FftPlanner};

pub const PSD_ALGORITHM_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq)]
pub struct PsdResult {
    pub frequencies_hz: Vec<f64>,
    pub values: Vec<f64>,
    pub resolution_hz: f64,
}

/// Mean-centered, symmetric-Hann, one-sided power spectral density.
pub fn one_sided_psd(
    samples: &[f64],
    sample_rate_hz: f64,
    window: WindowFunction,
) -> Result<PsdResult> {
    if samples.len() < 2 {
        return Err("processing/PSD: at least two samples are required".into());
    }
    if !sample_rate_hz.is_finite() || sample_rate_hz <= 0.0 {
        return Err("processing/PSD: sample rate must be finite and positive".into());
    }
    if samples.iter().any(|value| !value.is_finite()) {
        return Err("processing/PSD: samples must be finite".into());
    }
    let sample_count = samples.len();
    let mean = samples
        .iter()
        .map(|value| value / sample_count as f64)
        .sum::<f64>();
    let weights: Vec<f64> = match window {
        WindowFunction::HannSymmetric => (0..sample_count)
            .map(|index| {
                0.5 - 0.5 * (std::f64::consts::TAU * index as f64 / (sample_count - 1) as f64).cos()
            })
            .collect(),
    };
    let scale = sample_rate_hz * weights.iter().map(|weight| weight * weight).sum::<f64>();
    if !scale.is_finite() || scale <= 0.0 {
        return Err("processing/PSD: invalid window normalization".into());
    }
    let mut bins: Vec<Complex<f64>> = samples
        .iter()
        .zip(&weights)
        .map(|(value, weight)| Complex::new((value - mean) * weight, 0.0))
        .collect();
    FftPlanner::new()
        .plan_fft_forward(sample_count)
        .process(&mut bins);
    let resolution_hz = sample_rate_hz / sample_count as f64;
    let values = bins[..=sample_count / 2]
        .iter()
        .enumerate()
        .map(|(index, bin)| {
            let one_sided =
                if index == 0 || (sample_count.is_multiple_of(2) && index == sample_count / 2) {
                    1.0
                } else {
                    2.0
                };
            bin.norm_sqr() / scale * one_sided
        })
        .collect::<Vec<_>>();
    if values.iter().any(|value| !value.is_finite()) {
        return Err("processing/PSD: numerical overflow".into());
    }
    Ok(PsdResult {
        frequencies_hz: (0..values.len())
            .map(|index| index as f64 * resolution_hz)
            .collect(),
        values,
        resolution_hz,
    })
}

/// Integrate a half-open frequency range from an already calculated PSD.
pub fn integrate_band_power(psd: &PsdResult, low_hz: f64, high_hz: f64) -> Result<f64> {
    if !low_hz.is_finite() || !high_hz.is_finite() || low_hz < 0.0 || low_hz >= high_hz {
        return Err("processing/band-power: expected 0 <= low_hz < high_hz".into());
    }
    Ok(psd
        .frequencies_hz
        .iter()
        .zip(&psd.values)
        .filter(|(frequency, _)| **frequency >= low_hz && **frequency < high_hz)
        .map(|(_, value)| value * psd.resolution_hz)
        .sum())
}
