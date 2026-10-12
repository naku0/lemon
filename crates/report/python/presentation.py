"""Notebook presentation helpers. Uses stored analysis results; never reruns DSP."""

# %% LEMON CELL: IMPORTS
import json
import math
import re
from pathlib import Path

import matplotlib.pyplot as plt
import numpy as np
import pandas as pd
from IPython.display import display

MAX_FIELD_BYTES = 1024 * 1024
MAX_PLOT_POINTS = 12_000


def load_json(path):
    """Load a bounded UTF-8 JSON document."""
    limit = 128 if path.name == "report-dataset.json" else 4
    if path.stat().st_size > limit * 1024 * 1024:
        raise ValueError(f"{path.name}: JSON exceeds {limit} MiB")
    return json.loads(path.read_text(encoding="utf-8"))


def _analysis_root():
    if "LEMON_DATA_ROOT" in globals():
        root = Path(LEMON_DATA_ROOT)
        return root, load_json(root / "report-dataset.json")
    location = load_json(Path("report-data.json"))
    return Path(location["analysis_path"]), None


def _display_path(value):
    """Return a portable filename for display without changing source metadata."""
    if not value:
        return "unknown"
    return Path(str(value)).name or str(value)


def _display_text(value):
    """Remove machine-local path prefixes from text shown in notebook outputs."""
    text = str(value)
    candidates = [manifest.get("source"), manifest.get("source_path")]
    candidates.extend((manifest.get("source_hashes") or {}).keys())
    provenance = (manifest.get("source_metadata") or {}).get("provenance") or {}
    candidates.extend(provenance.get("files_used") or [])
    candidates.extend(provenance.get("duplicate_files_ignored") or [])
    for candidate in sorted((str(v) for v in candidates if v), key=len, reverse=True):
        text = text.replace(candidate, _display_path(candidate))
    return text


def warning_messages():
    """Return de-duplicated, portable warnings from stored analysis metadata."""
    items = []
    items.extend(manifest.get("analysis_warnings") or [])
    items.extend(manifest.get("import_warnings") or [])
    items.extend(summary.get("warnings") or [])
    if "estimated" in str(manifest.get("sample_rate_source", "")).lower():
        items.append("Sample rate is estimated, not a hardware specification.")
    if not manifest.get("units"):
        items.append("Units are unknown; amplitude is shown in a.u.")
    if not manifest.get("baseline"):
        items.append("No baseline is configured; baseline-change values are unavailable.")
    items.extend([
        "Signal quality is a LEMON engineering heuristic and is not a clinically validated metric.",
        "This report does not provide neurophysiological, cognitive, or medical interpretation.",
    ])
    return list(dict.fromkeys(_display_text(item) for item in items))


def _initialize_metadata():
    global ROOT, FIGURES, REPORT_DATASET, manifest, summary, warnings
    global AVAILABLE_CHANNELS, SAMPLE_RATE_HZ, SIGNAL_UNITS, BAND_DEFINITIONS
    global RECORDING_START_SECONDS, RECORDING_END_SECONDS, FULL_SAMPLE_DATA
    ROOT, REPORT_DATASET = _analysis_root()
    FIGURES = Path("figures")
    FIGURES.mkdir(exist_ok=True)
    if REPORT_DATASET is None:
        manifest = load_json(ROOT / "manifest.json")
        summary = load_json(ROOT / "summary.json")
        FULL_SAMPLE_DATA = True
    else:
        manifest = REPORT_DATASET["manifest"]
        summary = REPORT_DATASET["summary"]
        FULL_SAMPLE_DATA = (ROOT / "processed.csv").is_file()
    if manifest.get("analysis_format_version") != 1:
        raise ValueError("Unsupported AnalysisBundle version")
    AVAILABLE_CHANNELS = list(summary.get("channels", {}))
    if not 1 <= len(AVAILABLE_CHANNELS) <= 2:
        raise ValueError("Expected one or two channels")
    SAMPLE_RATE_HZ = float(manifest["sample_rate_hz"])
    SIGNAL_UNITS = manifest.get("units") or "a.u."
    BAND_DEFINITIONS = list(manifest.get("configured_bands") or [])
    RECORDING_START_SECONDS = float((REPORT_DATASET or {}).get("start", 0.0))
    RECORDING_END_SECONDS = float(
        (REPORT_DATASET or {}).get("end", summary.get("duration_seconds", 0.0))
    )
    warnings = []


if __name__ == "__main__" or "get_ipython" in globals():
    _initialize_metadata()

# %% LEMON CELL: USER_SETTINGS
# Safe presentation settings: editing these values never changes AnalysisBundle files.
CHANNEL = AVAILABLE_CHANNELS[0]
START_SECONDS = RECORDING_START_SECONDS
END_SECONDS = min(RECORDING_END_SECONDS, RECORDING_START_SECONDS + 30.0)

PSD_MIN_HZ = 0.5
PSD_MAX_HZ = min(45.0, SAMPLE_RATE_HZ / 2.0)

BAND_METRIC = "relative"  # absolute | relative | baseline
BAND_COLORS = {
    "Delta": "#0072B2",
    "Theta": "#6F4EAF",
    "Alpha": "#009E73",
    "Mu": "#56B4E9",
    "Alpha/Mu": "#009E73",
    "Beta": "#E69F00",
    "Gamma": "#D55E00",
}

# %% LEMON CELL: LOAD_DATA
def _read_csv(name, **kwargs):
    """Read one AnalysisBundle table and reject oversized text fields."""
    path = ROOT / name
    frame = pd.read_csv(path, low_memory=False, **kwargs)
    object_columns = frame.select_dtypes(include="object")
    for column in object_columns:
        lengths = object_columns[column].dropna().astype(str).str.len()
        if not lengths.empty and lengths.max() > MAX_FIELD_BYTES:
            raise ValueError(f"{name}: field exceeds 1 MiB")
    return frame


def _envelope_frame(values, column):
    rows = []
    bins = int(REPORT_DATASET["bins"])
    begin = float(REPORT_DATASET["start"])
    finish = float(REPORT_DATASET["end"])
    width = (finish - begin) / max(1, bins)
    for channel, bucket, low, high in values:
        timestamp = begin + (int(bucket) + 0.5) * width
        rows.extend(
            [
                {"timestamp_ns": int(timestamp * 1e9), "time_seconds": timestamp,
                 "sequence": pd.NA, "channel": channel, column: low,
                 "envelope_edge": "min", "is_aggregated": True},
                {"timestamp_ns": int(timestamp * 1e9), "time_seconds": timestamp,
                 "sequence": pd.NA, "channel": channel, column: high,
                 "envelope_edge": "max", "is_aggregated": True},
            ]
        )
    return pd.DataFrame(rows)


def _dataset_band_power():
    keys = ["channel", "band", "time_seconds", "envelope_edge"]
    combined = None
    for metric in ["absolute", "relative", "baseline_change_pct"]:
        rows = []
        for label, bucket, low, high in REPORT_DATASET["bands"][metric]:
            channel, band = label.split(" / ", 1)
            timestamp = RECORDING_START_SECONDS + (int(bucket) + 0.5) * (
                RECORDING_END_SECONDS - RECORDING_START_SECONDS
            ) / max(1, int(REPORT_DATASET["bins"]))
            rows.extend(
                [
                    {"channel": channel, "band": band, "time_seconds": timestamp,
                     "envelope_edge": "min", metric: low},
                    {"channel": channel, "band": band, "time_seconds": timestamp,
                     "envelope_edge": "max", metric: high},
                ]
            )
        part = pd.DataFrame(rows, columns=keys + [metric])
        combined = part if combined is None else combined.merge(part, on=keys, how="outer")
    definitions = pd.DataFrame(BAND_DEFINITIONS)
    if not definitions.empty:
        combined = combined.merge(definitions, left_on="band", right_on="name", how="left").drop(columns="name")
    return combined


def _dataset_events():
    rows = []
    for channel, kind, _bucket, count, first, last in REPORT_DATASET.get("events", []):
        rows.append({"timestamp_ns": int(float(first) * 1e9), "time_seconds": float(first),
                     "interval_end_seconds": float(last), "channel": channel,
                     "kind": kind, "count": int(count), "severity": "", "details": ""})
    return pd.DataFrame(rows)


def load_analysis_data():
    """Load stored analysis tables into stable public DataFrame variables."""
    global signal_df, raw_df, filtered_df, raw_by_channel, filtered_by_channel
    global psd_df, band_power_df, quality_events_df, warnings, DATA_GRANULARITY
    if FULL_SAMPLE_DATA:
        signal_df = _read_csv("processed.csv").rename(
            columns={"raw": "raw_value", "filtered": "filtered_value"}
        )
        signal_df["time_seconds"] = pd.to_numeric(signal_df["timestamp_ns"]) / 1e9
        signal_df["is_aggregated"] = False
        DATA_GRANULARITY = "sample-level"
    else:
        raw_envelope = _envelope_frame(REPORT_DATASET["raw"], "raw_value")
        filtered_envelope = _envelope_frame(REPORT_DATASET["filtered"], "filtered_value")
        keys = ["timestamp_ns", "time_seconds", "sequence", "channel", "envelope_edge", "is_aggregated"]
        signal_df = raw_envelope.merge(filtered_envelope, on=keys, how="outer")
        signal_df["quality"] = "Unknown"
        signal_df["flags"] = "ReportDatasetEnvelope"
        DATA_GRANULARITY = "min/max Report Dataset; sample timestamps are not embedded"
    raw_columns = ["timestamp_ns", "time_seconds", "sequence", "channel", "raw_value", "quality", "flags", "is_aggregated"]
    filtered_columns = ["timestamp_ns", "time_seconds", "sequence", "channel", "filtered_value", "quality", "flags", "is_aggregated"]
    raw_df = signal_df[raw_columns].dropna(subset=["raw_value"]).reset_index(drop=True)
    filtered_df = signal_df[filtered_columns].dropna(subset=["filtered_value"]).reset_index(drop=True)
    raw_by_channel = {channel: raw_df[raw_df["channel"] == channel].copy() for channel in AVAILABLE_CHANNELS}
    filtered_by_channel = {channel: filtered_df[filtered_df["channel"] == channel].copy() for channel in AVAILABLE_CHANNELS}
    if REPORT_DATASET is None:
        psd_df = _read_csv("spectrum.csv")
        band_power_df = _read_csv("band-power.csv")
        band_power_df["time_seconds"] = pd.to_numeric(band_power_df["window_end_ns"]) / 1e9
        quality_events_df = _read_csv("quality-events.csv")
        quality_events_df["time_seconds"] = pd.to_numeric(quality_events_df["timestamp_ns"]) / 1e9
    else:
        psd_df = pd.DataFrame(REPORT_DATASET["psd"])
        band_power_df = _dataset_band_power()
        quality_events_df = _dataset_events()
    warnings = warning_messages()


if __name__ == "__main__" or "get_ipython" in globals():
    load_analysis_data()

# %% LEMON CELL: AVAILABLE_DATASETS
dataset_catalog = pd.DataFrame(
    [
        ("raw_df", "Raw signal", len(raw_df), ", ".join(raw_df.columns)),
        ("filtered_df", "Filtered signal", len(filtered_df), ", ".join(filtered_df.columns)),
        ("signal_df", "Raw and filtered signal", len(signal_df), ", ".join(signal_df.columns)),
        ("psd_df", "Stored PSD bins/windows", len(psd_df), ", ".join(psd_df.columns)),
        ("band_power_df", "Stored band-power results", len(band_power_df), ", ".join(band_power_df.columns)),
        ("quality_events_df", "Timestamped analysis events", len(quality_events_df), ", ".join(quality_events_df.columns)),
    ],
    columns=["variable", "contents", "rows", "columns"],
)
display(dataset_catalog)
display(signal_df.head())
display(psd_df.head())
display(band_power_df.head())
print(f"Data granularity: {DATA_GRANULARITY}")
print("Per-channel dictionaries: raw_by_channel, filtered_by_channel")
print("Metadata dictionaries: manifest, summary, warnings")

# %% LEMON CELL: OVERVIEW
def _overview_table():
    provenance = (manifest.get("source_metadata") or {}).get("provenance") or {}
    files = provenance.get("files_used") or list((manifest.get("source_hashes") or {}).keys())
    return pd.DataFrame(
        {"value": {
            "source files": ", ".join(_display_path(v) for v in files) or _display_path(manifest.get("source")),
            "source format": manifest.get("source_format"),
            "duration, s": summary.get("duration_seconds"),
            "channels": ", ".join(AVAILABLE_CHANNELS),
            "sample rate, Hz": SAMPLE_RATE_HZ,
            "sample-rate source": manifest.get("sample_rate_source"),
            "units": SIGNAL_UNITS,
            "continuous segments": summary.get("continuous_segment_count"),
            "sample counts": summary.get("sample_count_by_channel"),
        }}
    )


display(_overview_table())

# %% LEMON CELL: SIGNAL_FUNCTIONS
def select_interval(data, start_seconds, end_seconds, channel=None):
    """Return a copy of rows between start_seconds and end_seconds, optionally for one channel."""
    if start_seconds > end_seconds:
        raise ValueError("start_seconds must not exceed end_seconds")
    selected = data[data["time_seconds"].between(start_seconds, end_seconds, inclusive="both")]
    if channel is not None:
        selected = selected[selected["channel"] == channel]
    return selected.copy()


def _value_column(data):
    if "filtered_value" in data and "raw_value" not in data:
        return "filtered_value"
    if "raw_value" in data and "filtered_value" not in data:
        return "raw_value"
    if "filtered_value" in data:
        return "filtered_value"
    raise ValueError("data must contain raw_value or filtered_value")


def _segments(data, value_column):
    data = data.sort_values(["time_seconds", "sequence"], na_position="last")
    split = data[value_column].isna()
    times = data["time_seconds"].astype(float)
    split |= times.diff().lt(0) | times.diff().gt(2.5 / SAMPLE_RATE_HZ)
    if "sequence" in data and data["sequence"].notna().any():
        split |= data["sequence"].diff().fillna(1).ne(1)
    if "flags" in data:
        split |= data["flags"].fillna("").str.contains("LostSample|InvalidPacket|Disconnected", regex=True)
    groups = split.cumsum()
    return [part.dropna(subset=[value_column]) for _, part in data.groupby(groups) if part[value_column].notna().any()]


def _plot_on_axis(ax, data, channel, value_column, label, color):
    selected = data[data["channel"] == channel]
    if selected.empty:
        ax.text(0.5, 0.5, "No data in the selected interval", ha="center", va="center", transform=ax.transAxes)
        return
    if selected.get("is_aggregated", pd.Series(False, index=selected.index)).any():
        grouped = selected.groupby("time_seconds")[value_column].agg(["min", "max"])
        ax.vlines(grouped.index, grouped["min"], grouped["max"], color=color, linewidth=0.8, label=label)
    else:
        first = True
        for segment in _segments(selected, value_column):
            if len(segment) > MAX_PLOT_POINTS:
                bucket = np.floor(np.linspace(0, MAX_PLOT_POINTS - 1, len(segment))).astype(int)
                envelope = segment.assign(_plot_bucket=bucket).groupby("_plot_bucket").agg(
                    time_seconds=("time_seconds", "mean"),
                    minimum=(value_column, "min"),
                    maximum=(value_column, "max"),
                )
                ax.vlines(envelope["time_seconds"], envelope["minimum"], envelope["maximum"],
                          color=color, linewidth=0.7, label=label if first else None)
            else:
                ax.plot(segment["time_seconds"], segment[value_column], color=color,
                        linewidth=0.8, label=label if first else None)
            first = False
    ax.grid(alpha=0.25)


def plot_signal(data, channel, start_seconds=None, end_seconds=None):
    """Plot one channel from a raw or filtered DataFrame and return (fig, ax)."""
    start_seconds = data["time_seconds"].min() if start_seconds is None else start_seconds
    end_seconds = data["time_seconds"].max() if end_seconds is None else end_seconds
    selected = select_interval(data, start_seconds, end_seconds, channel)
    value_column = _value_column(data)
    label = "Filtered" if value_column == "filtered_value" else "Raw"
    color = "#0072B2" if value_column == "filtered_value" else "#6B7280"
    fig, ax = plt.subplots(figsize=(12, 4), constrained_layout=True)
    _plot_on_axis(ax, selected, channel, value_column, label, color)
    ax.set(title=f"{label} signal — {channel}", xlabel="Time, s", ylabel=SIGNAL_UNITS)
    if ax.get_legend_handles_labels()[0]:
        ax.legend()
    return fig, ax


def plot_raw_filtered(channel, start_seconds=None, end_seconds=None):
    """Plot stored raw and filtered values for one channel and return (fig, ax)."""
    start_seconds = RECORDING_START_SECONDS if start_seconds is None else start_seconds
    end_seconds = RECORDING_END_SECONDS if end_seconds is None else end_seconds
    fig, ax = plt.subplots(figsize=(12, 4), constrained_layout=True)
    _plot_on_axis(ax, select_interval(raw_df, start_seconds, end_seconds, channel), channel,
                  "raw_value", "Raw", "#6B7280")
    _plot_on_axis(ax, select_interval(filtered_df, start_seconds, end_seconds, channel), channel,
                  "filtered_value", "Filtered", "#0072B2")
    ax.set(title=f"Raw versus filtered — {channel}", xlabel="Time, s", ylabel=SIGNAL_UNITS)
    if ax.get_legend_handles_labels()[0]:
        ax.legend()
    return fig, ax


def export_interval(data, output_path, start_seconds, end_seconds, channel=None):
    """Write a selected interval to CSV and return the resulting pathlib.Path."""
    output_path = Path(output_path)
    select_interval(data, start_seconds, end_seconds, channel).to_csv(output_path, index=False)
    return output_path


def _save_report_figure(fig, filename):
    path = FIGURES / filename
    fig.savefig(path, dpi=150, bbox_inches="tight")
    display(fig)
    plt.close(fig)
    return path


def _signal_overview(data, value_column, title, color):
    fig, axes = plt.subplots(len(AVAILABLE_CHANNELS), 1, squeeze=False,
                             figsize=(12, 3.5 * len(AVAILABLE_CHANNELS)), constrained_layout=True)
    for channel, ax in zip(AVAILABLE_CHANNELS, axes[:, 0]):
        _plot_on_axis(ax, data, channel, value_column, title, color)
        ax.set(title=f"{title} — {channel}", xlabel="Time, s", ylabel=SIGNAL_UNITS)
        if ax.get_legend_handles_labels()[0]:
            ax.legend()
    return fig

# %% LEMON CELL: RAW
raw_figure = _signal_overview(raw_df, "raw_value", "Raw", "#6B7280")
_save_report_figure(raw_figure, "raw-overview.png")

# %% LEMON CELL: FILTERED
filtered_figure = _signal_overview(filtered_df, "filtered_value", "Filtered", "#0072B2")
_save_report_figure(filtered_figure, "filtered-overview.png")

# %% LEMON CELL: COMPARISON
comparison_figure, _ = plot_raw_filtered(CHANNEL, START_SECONDS, END_SECONDS)
_save_report_figure(comparison_figure, "raw-filtered-comparison.png")
print(f"Displayed channel {CHANNEL}, interval {START_SECONDS:g}–{END_SECONDS:g} s.")

# %% LEMON CELL: PSD_FUNCTIONS
def _unique_bands():
    grouped = {}
    for band in BAND_DEFINITIONS:
        bounds = (float(band["low_hz"]), float(band["high_hz"]))
        grouped.setdefault(bounds, []).append(str(band["name"]))
    result = []
    for (low, high), names in grouped.items():
        if "Alpha/Mu" in names or set(names) == {"Alpha", "Mu"}:
            label = "Alpha/Mu"
        else:
            label = "/".join(dict.fromkeys(names))
        result.append({"name": label, "low_hz": low, "high_hz": high})
    return result


def _band_color(name, index=0):
    if name in BAND_COLORS:
        return BAND_COLORS[name]
    for part in name.split("/"):
        if part in BAND_COLORS:
            return BAND_COLORS[part]
    fallback = ["#0072B2", "#6F4EAF", "#009E73", "#E69F00", "#D55E00"]
    return fallback[index % len(fallback)]


def _plot_psd_on_axis(ax, channel, min_hz, max_hz):
    selected = psd_df[psd_df["channel"] == channel].copy()
    for column in ["frequency_hz", "raw_psd", "filtered_psd"]:
        selected[column] = pd.to_numeric(selected[column], errors="coerce")
    selected = selected[selected["frequency_hz"].between(min_hz, max_hz)]
    mean_psd = selected.groupby("frequency_hz", as_index=False)[["raw_psd", "filtered_psd"]].mean()
    if mean_psd.empty:
        ax.text(0.5, 0.5, "No PSD values in the selected range", ha="center", va="center", transform=ax.transAxes)
    else:
        ax.plot(mean_psd["frequency_hz"], mean_psd["raw_psd"], color="#6B7280", label="Raw PSD")
        ax.plot(mean_psd["frequency_hz"], mean_psd["filtered_psd"], color="#0072B2", label="Filtered PSD")
    for index, band in enumerate(_unique_bands()):
        low, high = max(min_hz, band["low_hz"]), min(max_hz, band["high_hz"])
        if low < high:
            ax.axvspan(low, high, alpha=0.14, color=_band_color(band["name"], index), label=band["name"])
    handles, labels = ax.get_legend_handles_labels()
    unique = dict(zip(labels, handles))
    if unique:
        ax.legend(unique.values(), unique.keys(), fontsize=8, loc="best")
    ax.set(xlabel="Frequency, Hz", ylabel=f"PSD, {SIGNAL_UNITS}²/Hz", xlim=(min_hz, max_hz))
    ax.grid(alpha=0.25)


def plot_psd(channel, min_hz=0.5, max_hz=45.0):
    """Plot mean stored PSD for one channel without recomputing FFT; return (fig, ax)."""
    fig, ax = plt.subplots(figsize=(12, 4.5), constrained_layout=True)
    _plot_psd_on_axis(ax, channel, min_hz, max_hz)
    ax.set_title(f"Power spectral density — {channel}")
    return fig, ax

# %% LEMON CELL: PSD
psd_figure, psd_axes = plt.subplots(len(AVAILABLE_CHANNELS), 1, squeeze=False,
                                    figsize=(12, 4 * len(AVAILABLE_CHANNELS)), constrained_layout=True)
for psd_channel, psd_axis in zip(AVAILABLE_CHANNELS, psd_axes[:, 0]):
    _plot_psd_on_axis(psd_axis, psd_channel, PSD_MIN_HZ, PSD_MAX_HZ)
    psd_axis.set_title(f"Power spectral density — {psd_channel}")
psd_figure.suptitle("Mean of stored PSD windows; FFT is not recomputed")
_save_report_figure(psd_figure, "spectrum.png")

# %% LEMON CELL: BAND_FUNCTIONS
def _metric_column(metric):
    columns = {"absolute": "absolute", "relative": "relative", "baseline": "baseline_change_pct"}
    if metric not in columns:
        raise ValueError("metric must be absolute, relative, or baseline")
    return columns[metric]


def _metric_units(metric):
    return {"absolute": f"{SIGNAL_UNITS}²", "relative": "fraction of configured total",
            "baseline": "% change from baseline"}[metric]


def plot_band_power(channel, metric="relative", band=None):
    """Plot stored band-power history for one channel/metric and return (fig, ax)."""
    column = _metric_column(metric)
    selected = band_power_df[band_power_df["channel"] == channel].copy()
    if band is not None:
        selected = selected[selected["band"] == band]
    fig, ax = plt.subplots(figsize=(12, 4.5), constrained_layout=True)
    names = list(dict.fromkeys(selected.get("band", pd.Series(dtype=str)).dropna().astype(str)))
    plotted = False
    for index, name in enumerate(names):
        values = selected[selected["band"] == name].sort_values("time_seconds")
        values[column] = pd.to_numeric(values[column], errors="coerce")
        values = values.dropna(subset=[column])
        if values.empty:
            continue
        if REPORT_DATASET is None:
            gap_tolerance = 2.5 * float(manifest["spectral_config"]["hop_samples"]) / SAMPLE_RATE_HZ
        else:
            gap_tolerance = 1.5 * (RECORDING_END_SECONDS - RECORDING_START_SECONDS) / max(1, int(REPORT_DATASET["bins"]))
        groups = values["time_seconds"].diff().gt(gap_tolerance).cumsum()
        first = True
        for _, segment in values.groupby(groups):
            ax.plot(segment["time_seconds"], segment[column], color=_band_color(name, index),
                    linewidth=1.0, label=name if first else None)
            first = False
        plotted = True
    if plotted:
        ax.legend(loc="best", fontsize=8)
    else:
        state = "No baseline values are available" if metric == "baseline" else "No band-power values are available"
        ax.text(0.5, 0.5, state, ha="center", va="center", transform=ax.transAxes)
    suffix = f" — {band}" if band else ""
    ax.set(title=f"Band power — {channel} — {metric}{suffix}", xlabel="Time, s", ylabel=_metric_units(metric))
    ax.grid(alpha=0.25)
    return fig, ax


def _safe_filename(value):
    return re.sub(r"[^A-Za-z0-9_.-]+", "_", str(value)).strip("_") or "channel"

# %% LEMON CELL: BANDS
band_power_figures = {}
for band_channel in AVAILABLE_CHANNELS:
    for band_metric in ["absolute", "relative", "baseline"]:
        figure, _ = plot_band_power(band_channel, band_metric)
        filename = f"band-power-{_safe_filename(band_channel)}-{band_metric}.png"
        band_power_figures[(band_channel, band_metric)] = _save_report_figure(figure, filename)
display(pd.DataFrame({channel: summary["channels"][channel].get("band_statistics", {})
                      for channel in AVAILABLE_CHANNELS}).T)

# %% LEMON CELL: QUALITY_FUNCTIONS
def _quality_distribution():
    rows = []
    for channel in AVAILABLE_CHANNELS:
        for label, values in summary["channels"][channel].get("quality_distribution", {}).items():
            rows.append({"channel": channel, "quality": label,
                         "count": int(values.get("count", 0)), "fraction": float(values.get("fraction", 0.0))})
    return pd.DataFrame(rows)


def _quality_is_reliable():
    basis = str(manifest.get("statistics_basis", "")).lower()
    messages = " ".join(manifest.get("analysis_warnings", [])).lower()
    return "one-sample" not in basis and "one-sample" not in messages


def _display_quality_distribution(reported):
    reliable = _quality_is_reliable()
    shown = reported.copy()
    if not reliable:
        totals = shown.groupby("channel", as_index=False)["count"].sum()
        shown = totals.assign(quality="Unknown", fraction=1.0)
        print("Quality limitation: analysis used one-sample processing blocks. The reported Poor labels are retained in summary and signal_df, but are shown as Unknown here because they are not a reliable recording-level quality assessment.")
        display(reported.rename(columns={"quality": "reported_quality"}))
    display(shown)
    fig, ax = plt.subplots(figsize=(9, 4), constrained_layout=True)
    pivot = shown.pivot(index="channel", columns="quality", values="fraction").fillna(0.0)
    if pivot.empty:
        plt.close(fig)
        print("No quality distribution is available for this recording.")
        return None
    colors = {"Good": "#009E73", "Acceptable": "#E69F00", "Fair": "#E69F00",
              "Poor": "#D55E00", "Disconnected": "#CC79A7", "Unknown": "#9CA3AF"}
    pivot.plot.bar(ax=ax, color=[colors.get(name, "#0072B2") for name in pivot.columns])
    ax.set(title="Quality distribution" + (" (displayed as Unknown: insufficient basis)" if not reliable else ""),
           xlabel="Channel", ylabel="Fraction of samples", ylim=(0, 1))
    ax.grid(axis="y", alpha=0.25)
    ax.legend(title="Quality", bbox_to_anchor=(1.02, 1), loc="upper left")
    return fig


def _display_timestamped_events():
    timed_kinds = {"LostSample", "InvalidPacket", "Disconnected", "Outlier", "Saturation", "LowQuality"}
    events = quality_events_df[quality_events_df.get("kind", pd.Series(dtype=str)).isin(timed_kinds)].copy()
    if events.empty:
        print("No timestamped quality events are available for this recording.")
        return None
    fig, ax = plt.subplots(figsize=(12, 3.5), constrained_layout=True)
    for index, kind in enumerate(sorted(events["kind"].dropna().unique())):
        values = events[events["kind"] == kind]
        ax.scatter(values["time_seconds"], [kind] * len(values), s=18,
                   color=_band_color(kind, index), label=kind)
    ax.set(title="Timestamped quality events", xlabel="Time, s", ylabel="Event")
    ax.grid(axis="x", alpha=0.25)
    return fig

# %% LEMON CELL: QUALITY
quality_distribution_df = _quality_distribution()
quality_figure = _display_quality_distribution(quality_distribution_df)
if quality_figure is not None:
    _save_report_figure(quality_figure, "quality-distribution.png")
quality_timeline_figure = _display_timestamped_events()
if quality_timeline_figure is not None:
    _save_report_figure(quality_timeline_figure, "quality-timeline.png")
normalization = str(manifest.get("time_normalization", ""))
if "actual lost count unknown" in normalization.lower() or "not hardware-synchronized" in normalization.lower():
    print("Continuity limitation: continuity and cross-channel synchronization cannot be established reliably from this source. No empty continuity plot is drawn.")
else:
    print(f"Continuous segments reported by analysis: {summary.get('continuous_segment_count', 'unknown')}")

# %% LEMON CELL: PROCESSING
processing_rows = {
    "filter sequence": manifest.get("filter_sequence"),
    "filter order": manifest.get("filter_order"),
    "high-pass, Hz": manifest.get("processing_config", {}).get("high_pass_hz"),
    "low-pass, Hz": manifest.get("processing_config", {}).get("low_pass_hz"),
    "notch enabled": manifest.get("processing_config", {}).get("notch"),
    "notch, Hz": manifest.get("processing_config", {}).get("notch_hz"),
    "FFT window samples": manifest.get("spectral_config", {}).get("window_samples"),
    "FFT hop samples": manifest.get("spectral_config", {}).get("hop_samples"),
    "FFT window": manifest.get("spectral_config", {}).get("window"),
    "PSD aggregation": manifest.get("spectral_config", {}).get("aggregation"),
    "baseline": manifest.get("baseline"),
    "statistics basis": manifest.get("statistics_basis"),
}
display(pd.DataFrame({"value": processing_rows}))
display(pd.DataFrame(BAND_DEFINITIONS))

# %% LEMON CELL: WARNINGS
warnings = warning_messages()
for warning in warnings:
    print("WARNING:", warning)

# %% LEMON CELL: FURTHER_SELECT
fragment = select_interval(
    filtered_df,
    start_seconds=START_SECONDS,
    end_seconds=END_SECONDS,
    channel=CHANNEL,
)
fragment.head()

# Additional ready-to-edit examples using the settings above:
# psd_fig, psd_ax = plot_psd(CHANNEL, PSD_MIN_HZ, PSD_MAX_HZ)
# band_fig, band_ax = plot_band_power(CHANNEL, BAND_METRIC)

# %% LEMON CELL: FURTHER_CUSTOM
fig, ax = plt.subplots(figsize=(12, 4), constrained_layout=True)
if fragment.empty:
    ax.text(0.5, 0.5, "No data in the selected interval", ha="center", va="center", transform=ax.transAxes)
else:
    ax.plot(fragment["time_seconds"], fragment["filtered_value"], color="#0072B2")
ax.set_title("Custom signal fragment")
ax.set_xlabel("Time, s")
ax.set_ylabel(SIGNAL_UNITS)
ax.grid(alpha=0.25)
plt.show()

# %% LEMON CELL: FURTHER_COMPARE
raw_fragment = select_interval(raw_df, START_SECONDS, END_SECONDS, CHANNEL)
filtered_fragment = select_interval(filtered_df, START_SECONDS, END_SECONDS, CHANNEL)
fig, ax = plt.subplots(figsize=(12, 4), constrained_layout=True)
ax.plot(raw_fragment["time_seconds"], raw_fragment["raw_value"], color="#6B7280", alpha=0.65, label="Raw")
ax.plot(filtered_fragment["time_seconds"], filtered_fragment["filtered_value"], color="#0072B2", label="Filtered")
ax.set(title=f"Raw versus filtered — {CHANNEL}", xlabel="Time, s", ylabel=SIGNAL_UNITS)
ax.grid(alpha=0.25)
ax.legend()
plt.show()

# %% LEMON CELL: FURTHER_EXPORT
# Uncomment to export the selected interval. The AnalysisBundle is never modified.
# exported_path = export_interval(
#     filtered_df, "selected-interval.csv", START_SECONDS, END_SECONDS, CHANNEL
# )
# exported_path
