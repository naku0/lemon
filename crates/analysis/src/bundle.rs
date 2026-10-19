use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, fs::File, io::Read, path::PathBuf};

use crate::AnalysisError;

#[derive(Debug, Clone)]
pub struct AnalysisBundle {
    pub directory: PathBuf,
    pub summary: Summary,
}

impl AnalysisBundle {
    /// Open and validate the public AnalysisBundle contract without running DSP.
    pub fn open(directory: impl Into<PathBuf>) -> Result<Self, AnalysisError> {
        let directory = directory.into();
        for name in ["manifest.json", "summary.json"] {
            let path = directory.join(name);
            if fs::metadata(&path)
                .map_err(|e| format!("analysis/open {}: {e}", path.display()))?
                .len()
                > 4 * 1024 * 1024
            {
                return Err(format!(
                    "analysis/open {}: JSON exceeds 4 MiB",
                    path.display()
                ));
            }
        }
        let manifest: serde_json::Value = serde_json::from_reader(
            File::open(directory.join("manifest.json")).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        let format_version = manifest["analysis_format_version"]
            .as_u64()
            .ok_or("analysis/open: missing or invalid analysis_format_version")?;
        if !matches!(format_version, 1 | 2) {
            return Err("analysis/open: unsupported analysis_format_version".into());
        }
        if format_version == 2
            && (!matches!(manifest["dsp_algorithm_version"].as_u64(), Some(2 | 3))
                || manifest["psd_algorithm_version"].as_u64() != Some(2)
                || manifest["processing_mode"].as_str() != Some("causal_live_compatible")
                || manifest["window_function"].as_str() != Some("hann_symmetric"))
        {
            return Err("analysis/open: missing or unsupported v2 DSP provenance".into());
        }
        if manifest["dsp_algorithm_version"].as_u64() == Some(3)
            && manifest["windowing_algorithm_version"].as_u64() != Some(1)
        {
            return Err("analysis/open: missing or unsupported windowing_algorithm_version".into());
        }
        let summary: Summary = serde_json::from_reader(
            File::open(directory.join("summary.json")).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        for name in [
            "manifest.json",
            "processed.csv",
            "spectrum.csv",
            "band-power.csv",
            "quality-events.csv",
        ] {
            if !directory.join(name).is_file() {
                return Err(format!("analysis: missing {name}"));
            }
        }
        if format_version >= 2 && !directory.join("excluded-windows.csv").is_file() {
            return Err("analysis: missing excluded-windows.csv".into());
        }
        if format_version == 2 {
            for (name, required) in [
                ("processed.csv", "timestamp_ns,sequence,channel,raw,filtered,segment_id,sample_valid,filter_warmup,artifact_rejected,quality,flags"),
                ("spectrum.csv", "window_index,window_start_ns,window_end_ns,channel,segment_id,frequency_hz,raw_psd,filtered_psd,window_function,sample_count,frequency_resolution_hz"),
                ("band-power.csv", "window_index,window_start_ns,window_end_ns,channel,segment_id,band,low_hz,high_hz,absolute,relative,smoothed,baseline_change_pct,quality_status"),
                ("excluded-windows.csv", "channel,segment_id,window_index,window_start_ns,window_end_ns,reason_codes,details"),
            ] {
                let file = File::open(directory.join(name))
                    .map_err(|error| format!("analysis/open {name}: {error}"))?;
                let mut csv = csv::Reader::from_reader(file.take(65_536));
                let headers = csv
                    .headers()
                    .map_err(|error| format!("analysis/open {name}: {error}"))?;
                for field in required.split(',') {
                    if headers.iter().filter(|header| *header == field).count() != 1 {
                        return Err(format!(
                            "analysis/open {name}: v2 requires exactly one {field} column"
                        ));
                    }
                }
            }
        }
        Ok(Self { directory, summary })
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct QualityCount {
    pub count: u64,
    pub fraction: f64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct BandSummary {
    pub low_hz: f64,
    pub high_hz: f64,
    pub mean_absolute_power: f64,
    pub median_absolute_power: Option<f64>,
    pub mean_relative_power: f64,
    pub median_relative_power: Option<f64>,
    pub min_relative_power: f64,
    pub max_relative_power: f64,
    pub mean_baseline_change_pct: Option<f64>,
    pub valid_window_count: u64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChannelSummary {
    pub raw_min: f64,
    pub raw_max: f64,
    pub raw_mean: f64,
    pub raw_stddev: f64,
    pub filtered_min: Option<f64>,
    pub filtered_max: Option<f64>,
    pub filtered_mean: Option<f64>,
    pub filtered_stddev: Option<f64>,
    pub quality_distribution: BTreeMap<String, QualityCount>,
    pub dominant_peak_frequency_hz: Option<f64>,
    pub dominant_peak_power: Option<f64>,
    pub mains_power_before: f64,
    pub mains_power_after: f64,
    pub mains_reduction_percent: Option<f64>,
    pub band_statistics: BTreeMap<String, BandSummary>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Summary {
    pub duration_seconds: f64,
    pub channel_count: usize,
    pub sample_count_by_channel: BTreeMap<String, u64>,
    pub valid_filtered_count_by_channel: BTreeMap<String, u64>,
    pub missing_count_by_channel: BTreeMap<String, u64>,
    pub lost_sample_count: u64,
    pub invalid_packet_count: u64,
    pub outlier_count: u64,
    pub saturation_count: u64,
    pub disconnection_count: u64,
    pub continuous_segment_count: u64,
    pub sample_rate_hz: f64,
    pub sample_rate_source: String,
    pub channels: BTreeMap<String, ChannelSummary>,
    pub warnings: Vec<String>,
    pub dominant_band_by_channel: BTreeMap<String, String>,
    #[serde(default)]
    pub valid_spectral_window_count_by_channel: BTreeMap<String, u64>,
    #[serde(default)]
    pub excluded_spectral_window_count_by_channel: BTreeMap<String, u64>,
    #[serde(default)]
    pub excluded_window_reasons: BTreeMap<String, u64>,
}
