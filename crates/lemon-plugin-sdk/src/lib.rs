//! Stable, typed access to the public LEMON AnalysisBundle file contract.
use analysis::AnalysisBundle;
pub use analysis::{BandSummary, ChannelSummary, QualityCount, Summary};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
    error::Error,
    fmt,
    fs::{self, File},
    marker::PhantomData,
    path::{Component, Path, PathBuf},
};

pub const PLUGIN_API_VERSION: u32 = 1;
pub const RESULT_SCHEMA_VERSION: u32 = 1;

fn legacy_dsp_version() -> u32 {
    1
}

#[derive(Debug)]
pub enum SdkError {
    Bundle(String),
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Json {
        path: PathBuf,
        source: serde_json::Error,
    },
    Csv {
        path: PathBuf,
        source: csv::Error,
    },
    InvalidResult(String),
}
impl fmt::Display for SdkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bundle(e) | Self::InvalidResult(e) => f.write_str(e),
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Json { path, source } => write!(f, "JSON {}: {source}", path.display()),
            Self::Csv { path, source } => write!(f, "CSV {}: {source}", path.display()),
        }
    }
}
impl Error for SdkError {}

#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub analysis_format_version: u32,
    pub lemon_version: String,
    pub created_at: String,
    pub source: String,
    pub source_format: String,
    pub source_path: String,
    #[serde(default)]
    pub source_hash: String,
    #[serde(default)]
    pub source_hashes: serde_json::Value,
    #[serde(default)]
    pub source_metadata: serde_json::Value,
    pub channels: Vec<String>,
    pub units: Option<String>,
    pub sample_rate_hz: f64,
    pub sample_rate_source: String,
    pub duration_seconds: f64,
    pub processing_config: serde_json::Value,
    #[serde(default)]
    pub spectral_config: serde_json::Value,
    #[serde(default)]
    pub configured_bands: serde_json::Value,
    #[serde(default)]
    pub import_warnings: Vec<String>,
    #[serde(default)]
    pub analysis_warnings: Vec<String>,
    #[serde(default)]
    pub time_normalization: String,
    #[serde(default = "legacy_dsp_version")]
    pub dsp_algorithm_version: u32,
    #[serde(default)]
    pub windowing_algorithm_version: u32,
    #[serde(default = "legacy_dsp_version")]
    pub psd_algorithm_version: u32,
    #[serde(default)]
    pub processing_mode: String,
    #[serde(default)]
    pub window_function: String,
    #[serde(default)]
    pub filter_warmup_seconds: f64,
    #[serde(default)]
    pub reject_low_quality_windows: bool,
}

#[derive(Debug, Clone)]
pub struct Bundle {
    directory: PathBuf,
    manifest: Manifest,
    summary: Summary,
}
impl Bundle {
    pub fn open(directory: impl AsRef<Path>) -> Result<Self, SdkError> {
        let canonical = fs::canonicalize(directory.as_ref()).map_err(|source| SdkError::Io {
            path: directory.as_ref().to_path_buf(),
            source,
        })?;
        let base = AnalysisBundle::open(&canonical).map_err(SdkError::Bundle)?;
        let path = canonical.join("manifest.json");
        let manifest = read_json(&path)?;
        Ok(Self {
            directory: canonical,
            manifest,
            summary: base.summary,
        })
    }
    pub fn directory(&self) -> &Path {
        &self.directory
    }
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }
    pub fn summary(&self) -> &Summary {
        &self.summary
    }
    pub fn channels(&self) -> &[String] {
        &self.manifest.channels
    }
    pub fn sample_rate_hz(&self) -> f64 {
        self.manifest.sample_rate_hz
    }
    pub fn units(&self) -> Option<&str> {
        self.manifest.units.as_deref()
    }
    pub fn processed(&self) -> Result<CsvRows<ProcessedRow>, SdkError> {
        self.rows("processed.csv")
    }
    pub fn spectrum(&self) -> Result<CsvRows<SpectrumRow>, SdkError> {
        self.rows("spectrum.csv")
    }
    pub fn band_power(&self) -> Result<CsvRows<BandPowerRow>, SdkError> {
        self.rows("band-power.csv")
    }
    pub fn quality_events(&self) -> Result<CsvRows<QualityEventRow>, SdkError> {
        self.rows("quality-events.csv")
    }
    pub fn excluded_windows(&self) -> Result<Option<CsvRows<ExcludedWindowRow>>, SdkError> {
        let path = self.directory.join("excluded-windows.csv");
        if !path.is_file() && self.manifest.analysis_format_version < 2 {
            return Ok(None);
        }
        CsvRows::open(path).map(Some)
    }
    fn rows<T: DeserializeOwned>(&self, name: &str) -> Result<CsvRows<T>, SdkError> {
        CsvRows::open(self.directory.join(name))
    }
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, SdkError> {
    let file = File::open(path).map_err(|source| SdkError::Io {
        path: path.into(),
        source,
    })?;
    serde_json::from_reader(file).map_err(|source| SdkError::Json {
        path: path.into(),
        source,
    })
}

pub struct CsvRows<T> {
    path: PathBuf,
    rows: csv::DeserializeRecordsIntoIter<File, T>,
    marker: PhantomData<T>,
}
impl<T: DeserializeOwned> CsvRows<T> {
    fn open(path: PathBuf) -> Result<Self, SdkError> {
        let reader = csv::Reader::from_path(&path).map_err(|source| SdkError::Csv {
            path: path.clone(),
            source,
        })?;
        Ok(Self {
            path,
            rows: reader.into_deserialize(),
            marker: PhantomData,
        })
    }
}
impl<T: DeserializeOwned> Iterator for CsvRows<T> {
    type Item = Result<T, SdkError>;
    fn next(&mut self) -> Option<Self::Item> {
        self.rows.next().map(|row| {
            row.map_err(|source| SdkError::Csv {
                path: self.path.clone(),
                source,
            })
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProcessedRow {
    pub timestamp_ns: u64,
    pub sequence: u64,
    pub channel: String,
    pub raw: f64,
    pub filtered: Option<f64>,
    #[serde(default)]
    pub segment_id: u64,
    #[serde(default = "default_true")]
    pub sample_valid: bool,
    #[serde(default)]
    pub filter_warmup: bool,
    #[serde(default)]
    pub artifact_rejected: bool,
    pub quality: String,
    pub flags: String,
}
#[derive(Debug, Clone, Deserialize)]
pub struct SpectrumRow {
    pub window_index: u64,
    pub window_start_ns: u64,
    pub window_end_ns: u64,
    pub channel: String,
    #[serde(default)]
    pub segment_id: u64,
    pub frequency_hz: f64,
    pub raw_psd: f64,
    pub filtered_psd: Option<f64>,
    #[serde(default)]
    pub window_function: String,
    #[serde(default)]
    pub sample_count: usize,
    #[serde(default)]
    pub frequency_resolution_hz: f64,
}
#[derive(Debug, Clone, Deserialize)]
pub struct BandPowerRow {
    pub window_index: u64,
    pub window_start_ns: u64,
    pub window_end_ns: u64,
    pub channel: String,
    #[serde(default)]
    pub segment_id: u64,
    pub band: String,
    pub low_hz: f64,
    pub high_hz: f64,
    pub absolute: f64,
    pub relative: f64,
    pub smoothed: Option<f64>,
    pub baseline_change_pct: Option<f64>,
    #[serde(default)]
    pub quality_status: String,
}
#[derive(Debug, Clone, Deserialize)]
pub struct QualityEventRow {
    pub timestamp_ns: u64,
    pub channel: Option<String>,
    pub kind: String,
    pub severity: String,
    pub details: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExcludedWindowRow {
    pub channel: String,
    pub segment_id: u64,
    pub window_index: u64,
    pub window_start_ns: u64,
    pub window_end_ns: u64,
    pub reason_codes: String,
    pub details: String,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PluginStatus {
    Completed,
    CompletedWithWarnings,
    Failed,
}
impl fmt::Display for PluginStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Completed => "completed",
            Self::CompletedWithWarnings => "completed_with_warnings",
            Self::Failed => "failed",
        })
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Data,
    Table,
    Figure,
    Report,
}
impl fmt::Display for ArtifactKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Data => "data",
            Self::Table => "table",
            Self::Figure => "figure",
            Self::Report => "report",
        })
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginIdentity {
    pub id: String,
    pub version: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub path: String,
    pub kind: ArtifactKind,
    pub description: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginResult {
    pub schema_version: u32,
    pub plugin: PluginIdentity,
    pub status: PluginStatus,
    #[serde(default)]
    pub artifacts: Vec<Artifact>,
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
impl PluginResult {
    pub fn completed(
        id: impl Into<String>,
        version: impl Into<String>,
        artifacts: Vec<Artifact>,
    ) -> Self {
        Self {
            schema_version: RESULT_SCHEMA_VERSION,
            plugin: PluginIdentity {
                id: id.into(),
                version: version.into(),
            },
            status: PluginStatus::Completed,
            artifacts,
            warnings: Vec::new(),
            error: None,
        }
    }
}

pub fn write_plugin_result(
    output: impl AsRef<Path>,
    result: &PluginResult,
) -> Result<PathBuf, SdkError> {
    if result.schema_version != RESULT_SCHEMA_VERSION {
        return Err(SdkError::InvalidResult(
            "unsupported plugin result schema_version".into(),
        ));
    }
    for artifact in &result.artifacts {
        validate_relative(&artifact.path)?;
    }
    let output = output.as_ref();
    fs::create_dir_all(output).map_err(|source| SdkError::Io {
        path: output.into(),
        source,
    })?;
    let path = output.join("plugin-result.json");
    let temp = output.join(".plugin-result.json.tmp");
    let file = File::create(&temp).map_err(|source| SdkError::Io {
        path: temp.clone(),
        source,
    })?;
    serde_json::to_writer_pretty(&file, result).map_err(|source| SdkError::Json {
        path: temp.clone(),
        source,
    })?;
    file.sync_all().map_err(|source| SdkError::Io {
        path: temp.clone(),
        source,
    })?;
    fs::rename(&temp, &path).map_err(|source| SdkError::Io {
        path: path.clone(),
        source,
    })?;
    Ok(path)
}
fn validate_relative(value: &str) -> Result<(), SdkError> {
    let path = Path::new(value);
    if value.is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(SdkError::InvalidResult(format!(
            "artifact path must be a relative path without traversal: {value:?}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn temp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("lemon-sdk-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).expect("temp");
        p
    }
    fn fixture() -> PathBuf {
        let p = temp("bundle");
        fs::write(p.join("manifest.json"),r#"{"analysis_format_version":1,"lemon_version":"0.2.0","created_at":"0","source":"fixture","source_format":"csv","source_path":"fixture.csv","channels":["A0"],"units":"a.u.","sample_rate_hz":250.0,"sample_rate_source":"confirmed","duration_seconds":1.0,"processing_config":{},"spectral_config":{},"configured_bands":[]}"#).expect("manifest");
        fs::write(p.join("summary.json"),r#"{"duration_seconds":1.0,"channel_count":1,"sample_count_by_channel":{"A0":1},"valid_filtered_count_by_channel":{"A0":1},"missing_count_by_channel":{"A0":0},"lost_sample_count":0,"invalid_packet_count":0,"outlier_count":0,"saturation_count":0,"disconnection_count":0,"continuous_segment_count":1,"sample_rate_hz":250.0,"sample_rate_source":"confirmed","channels":{},"warnings":[],"dominant_band_by_channel":{}}"#).expect("summary");
        fs::write(p.join("summary.md"), "fixture").expect("md");
        fs::write(
            p.join("processed.csv"),
            "timestamp_ns,sequence,channel,raw,filtered,quality,flags\n0,0,A0,1.5,1.0,Good,\n",
        )
        .expect("processed");
        fs::write(p.join("spectrum.csv"),"window_index,window_start_ns,window_end_ns,channel,frequency_hz,raw_psd,filtered_psd\n").expect("spectrum");
        fs::write(p.join("band-power.csv"),"window_index,window_start_ns,window_end_ns,channel,band,low_hz,high_hz,absolute,relative,smoothed,baseline_change_pct\n").expect("bands");
        fs::write(
            p.join("quality-events.csv"),
            "timestamp_ns,channel,kind,severity,details\n",
        )
        .expect("quality");
        p
    }
    #[test]
    fn opens_and_streams_bundle() {
        let p = fixture();
        let b = Bundle::open(&p).expect("bundle");
        assert_eq!(b.channels(), &["A0"]);
        assert_eq!(b.sample_rate_hz(), 250.0);
        assert_eq!(b.manifest().dsp_algorithm_version, 1);
        assert_eq!(b.manifest().psd_algorithm_version, 1);
        assert!(b.excluded_windows().expect("excluded").is_none());
        let rows = b
            .processed()
            .expect("rows")
            .collect::<Result<Vec<_>, _>>()
            .expect("csv");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].raw, 1.5);
        assert!(rows[0].sample_valid);
        let _ = fs::remove_dir_all(p);
    }
    #[test]
    fn result_writer_rejects_traversal() {
        let p = temp("result");
        let mut r = PluginResult::completed(
            "x",
            "1",
            vec![Artifact {
                path: "../x".into(),
                kind: ArtifactKind::Data,
                description: String::new(),
            }],
        );
        assert!(write_plugin_result(&p, &r).is_err());
        r.artifacts[0].path = "x.json".into();
        assert!(write_plugin_result(&p, &r).is_ok());
        let _ = fs::remove_dir_all(p);
    }
}
