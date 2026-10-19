use lemon_plugin_sdk::{write_plugin_result, Artifact, ArtifactKind, Bundle, PluginResult};
use std::{env, fs::File, path::PathBuf};

fn main() {
    if let Err(error) = run() {
        eprintln!("example-summary: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    if env::var("LEMON_PLUGIN_API").as_deref() != Ok("1") {
        return Err("LEMON_PLUGIN_API=1 is required".into());
    }
    let mut input = None;
    let mut config = None;
    let mut output = None;
    let mut args = env::args_os().skip(1);
    while let Some(arg) = args.next() {
        let target = match arg.to_str() {
            Some("--input") => &mut input,
            Some("--config") => &mut config,
            Some("--output") => &mut output,
            _ => return Err(format!("unexpected argument {arg:?}")),
        };
        *target = Some(PathBuf::from(args.next().ok_or("missing argument value")?));
    }
    let input = input.ok_or("--input is required")?;
    let config = config.ok_or("--config is required")?;
    let output = output.ok_or("--output is required")?;
    let _: serde_json::Value = serde_json::from_reader(
        File::open(&config).map_err(|e| format!("config {}: {e}", config.display()))?,
    ).map_err(|e| format!("config JSON: {e}"))?;
    let bundle = Bundle::open(input).map_err(|e| e.to_string())?;
    let compact = serde_json::json!({
        "analysis_format_version": bundle.manifest().analysis_format_version,
        "source_format": bundle.manifest().source_format,
        "channels": bundle.channels(),
        "units": bundle.units(),
        "sample_rate_hz": bundle.sample_rate_hz(),
        "duration_seconds": bundle.summary().duration_seconds,
        "sample_count_by_channel": bundle.summary().sample_count_by_channel,
        "warnings": bundle.summary().warnings,
    });
    std::fs::create_dir_all(&output).map_err(|e| e.to_string())?;
    let summary = output.join("summary.json");
    serde_json::to_writer_pretty(File::create(&summary).map_err(|e| e.to_string())?, &compact)
        .map_err(|e| e.to_string())?;
    let result = PluginResult::completed("example-summary", "0.1.0", vec![Artifact {
        path: "summary.json".into(), kind: ArtifactKind::Data,
        description: "Compact technical summary from the validated AnalysisBundle".into(),
    }]);
    write_plugin_result(output, &result).map_err(|e| e.to_string())?;
    Ok(())
}
