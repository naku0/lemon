use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs::File, path::PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WindowPower {
    pub(crate) dsp_algorithm_version: u32,
    pub(crate) psd_algorithm_version: u32,
    pub(crate) window_function: String,
    pub(crate) bundle: String,
    pub(crate) condition: String,
    pub(crate) channel: String,
    pub(crate) window_index: u64,
    pub(crate) segment_id: Option<u64>,
    pub(crate) window_start_ns: u64,
    pub(crate) window_end_ns: u64,
    pub(crate) midpoint_seconds: f64,
    pub(crate) band: String,
    pub(crate) low_hz: f64,
    pub(crate) high_hz: f64,
    pub(crate) power: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ExcludedWindow {
    pub(crate) dsp_algorithm_version: u32,
    pub(crate) bundle: String,
    pub(crate) condition: String,
    pub(crate) channel: String,
    pub(crate) window_index: u64,
    pub(crate) segment_id: Option<u64>,
    pub(crate) window_start_ns: u64,
    pub(crate) window_end_ns: u64,
    pub(crate) reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ConditionResult {
    pub(crate) dsp_algorithm_version: u32,
    pub(crate) psd_algorithm_version: u32,
    pub(crate) window_function: String,
    pub(crate) condition: String,
    pub(crate) channel: String,
    pub(crate) band: String,
    pub(crate) valid_window_count: usize,
    pub(crate) excluded_window_count: usize,
    pub(crate) median_power: Option<f64>,
    pub(crate) iqr_power: Option<f64>,
    pub(crate) baseline_median_power: Option<f64>,
    pub(crate) relative_change_percent: Option<f64>,
    pub(crate) change_db: Option<f64>,
    pub(crate) change_direction: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct Summary {
    pub(crate) title: &'static str,
    pub(crate) plugin_id: &'static str,
    pub(crate) plugin_version: &'static str,
    pub(crate) input_bundle_versions: BTreeMap<String, String>,
    pub(crate) dsp_algorithm_versions: BTreeMap<String, u32>,
    pub(crate) psd_algorithm_versions: BTreeMap<String, u32>,
    pub(crate) computed_psd_algorithm_version: u32,
    pub(crate) windowing_algorithm_version: u32,
    pub(crate) input_windowing_algorithm_versions: BTreeMap<String, u32>,
    pub(crate) input_processing_configs: BTreeMap<String, serde_json::Value>,
    pub(crate) synchronization_status: BTreeMap<String, String>,
    pub(crate) sample_rate_hz: f64,
    pub(crate) sample_rates_hz: BTreeMap<String, f64>,
    pub(crate) sample_rate_sources: BTreeMap<String, String>,
    pub(crate) quality_assessment: BTreeMap<String, String>,
    pub(crate) units: Option<String>,
    pub(crate) channels: Vec<String>,
    pub(crate) window_seconds: f64,
    pub(crate) hop_seconds: f64,
    pub(crate) window_samples: usize,
    pub(crate) hop_samples: usize,
    pub(crate) window_samples_by_input: BTreeMap<String, usize>,
    pub(crate) hop_samples_by_input: BTreeMap<String, usize>,
    pub(crate) window_function: &'static str,
    pub(crate) psd_method: &'static str,
    pub(crate) bands_hz: BTreeMap<String, [f64; 2]>,
    pub(crate) valid_windows: BTreeMap<String, usize>,
    pub(crate) excluded_windows: BTreeMap<String, usize>,
    pub(crate) exclusion_reasons: BTreeMap<String, usize>,
    pub(crate) bundle_excluded_windows: BTreeMap<String, usize>,
    pub(crate) bundle_exclusion_reasons: BTreeMap<String, BTreeMap<String, usize>>,
    pub(crate) warnings: Vec<String>,
    pub(crate) limitations: Vec<String>,
}

pub(crate) fn write_powers(path: PathBuf, rows: &[WindowPower]) -> Result<(), String> {
    write_rows(
        path,
        &[
            "dsp_algorithm_version",
            "psd_algorithm_version",
            "window_function",
            "bundle",
            "condition",
            "channel",
            "window_index",
            "segment_id",
            "window_start_ns",
            "window_end_ns",
            "midpoint_seconds",
            "band",
            "low_hz",
            "high_hz",
            "power",
        ],
        rows,
    )
}

pub(crate) fn write_excluded(path: PathBuf, rows: &[ExcludedWindow]) -> Result<(), String> {
    write_rows(
        path,
        &[
            "dsp_algorithm_version",
            "bundle",
            "condition",
            "channel",
            "window_index",
            "segment_id",
            "window_start_ns",
            "window_end_ns",
            "reason",
        ],
        rows,
    )
}

pub(crate) fn write_results(path: PathBuf, rows: &[ConditionResult]) -> Result<(), String> {
    write_rows(
        path,
        &[
            "dsp_algorithm_version",
            "psd_algorithm_version",
            "window_function",
            "condition",
            "channel",
            "band",
            "valid_window_count",
            "excluded_window_count",
            "median_power",
            "iqr_power",
            "baseline_median_power",
            "relative_change_percent",
            "change_db",
            "change_direction",
        ],
        rows,
    )
}

fn write_rows<T: Serialize>(path: PathBuf, headers: &[&str], rows: &[T]) -> Result<(), String> {
    let mut writer = csv::WriterBuilder::new()
        .has_headers(false)
        .from_writer(File::create(path).map_err(|error| error.to_string())?);
    writer
        .write_record(headers)
        .map_err(|error| error.to_string())?;
    for row in rows {
        writer.serialize(row).map_err(|error| error.to_string())?;
    }
    writer.flush().map_err(|error| error.to_string())
}
