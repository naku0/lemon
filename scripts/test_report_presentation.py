"""Tests for notebook research helpers; plotting tests skip without report dependencies."""
from pathlib import Path
import importlib.util
import tempfile
import unittest

SOURCE = Path(__file__).resolve().parents[1] / "crates/report/python/presentation.py"
HAS_DEPS = all(importlib.util.find_spec(name) for name in ("numpy", "pandas", "matplotlib"))

if HAS_DEPS:
    import pandas as pd

    def source_cell(name):
        text = SOURCE.read_text(encoding="utf-8")
        marker = f"# %% LEMON CELL: {name}\n"
        body = text.split(marker, 1)[1]
        return body.split("# %% LEMON CELL: ", 1)[0]

    presentation = {"__name__": "presentation_test"}
    exec(source_cell("IMPORTS"), presentation)
    presentation.update({
        "AVAILABLE_CHANNELS": ["A0"],
        "RECORDING_START_SECONDS": 0.0,
        "RECORDING_END_SECONDS": 10.0,
        "SAMPLE_RATE_HZ": 10.0,
        "SIGNAL_UNITS": "uV",
        "BAND_DEFINITIONS": [],
    })
    for section in ["USER_SETTINGS", "SIGNAL_FUNCTIONS", "PSD_FUNCTIONS", "BAND_FUNCTIONS", "QUALITY_FUNCTIONS"]:
        exec(source_cell(section), presentation)


class PresentationSource(unittest.TestCase):
    def test_notebook_source_is_split_into_semantic_cells(self):
        text = SOURCE.read_text(encoding="utf-8")
        for name in [
            "IMPORTS", "USER_SETTINGS", "LOAD_DATA", "AVAILABLE_DATASETS", "SIGNAL_FUNCTIONS",
            "PSD_FUNCTIONS", "BAND_FUNCTIONS", "QUALITY_FUNCTIONS", "FURTHER_EXPORT",
        ]:
            self.assertIn(f"# %% LEMON CELL: {name}", text)
        self.assertIn("def select_interval", text)
        self.assertIn("def plot_raw_filtered", text)
        self.assertIn("def export_interval", text)
        self.assertNotIn("fft(", text.lower())


@unittest.skipUnless(HAS_DEPS, "numpy/pandas/matplotlib are not installed")
class PresentationFunctions(unittest.TestCase):
    def setUp(self):
        presentation["SAMPLE_RATE_HZ"] = 10.0
        presentation["SIGNAL_UNITS"] = "uV"
        presentation["BAND_DEFINITIONS"] = [
            {"name": "Alpha", "low_hz": 8.0, "high_hz": 13.0},
            {"name": "Mu", "low_hz": 8.0, "high_hz": 13.0},
            {"name": "Theta", "low_hz": 4.0, "high_hz": 8.0},
        ]
        presentation["BAND_COLORS"] = {
            "Alpha/Mu": "#009E73",
            "Theta": "#6F4EAF",
        }

    def test_select_interval_returns_copy_and_export_is_explicit(self):
        frame = pd.DataFrame({
            "time_seconds": [0.0, 1.0, 2.0],
            "channel": ["A0", "A0", "A1"],
            "filtered_value": [1.0, 2.0, 3.0],
        })
        selected = presentation["select_interval"](frame, 0.5, 1.5, "A0")
        self.assertEqual(selected["filtered_value"].tolist(), [2.0])
        selected.loc[:, "filtered_value"] = 9.0
        self.assertEqual(frame.loc[1, "filtered_value"], 2.0)
        with tempfile.TemporaryDirectory() as directory:
            path = presentation["export_interval"](frame, Path(directory) / "part.csv", 0.0, 1.0, "A0")
            self.assertTrue(path.is_file())

    def test_equal_alpha_mu_bounds_are_one_colored_region(self):
        bands = presentation["_unique_bands"]()
        self.assertEqual(len(bands), 2)
        self.assertEqual([band["name"] for band in bands].count("Alpha/Mu"), 1)
        self.assertNotEqual(
            presentation["_band_color"]("Theta"),
            presentation["_band_color"]("Alpha/Mu"),
        )

    def test_signal_plot_splits_a_gap(self):
        frame = pd.DataFrame({
            "timestamp_ns": [0, 100_000_000, 2_000_000_000, 2_100_000_000],
            "time_seconds": [0.0, 0.1, 2.0, 2.1],
            "sequence": [0, 1, 20, 21],
            "channel": ["A0"] * 4,
            "filtered_value": [1.0, 2.0, 3.0, 4.0],
            "quality": ["Unknown"] * 4,
            "flags": [""] * 4,
            "is_aggregated": [False] * 4,
        })
        fig, ax = presentation["plot_signal"](frame, "A0")
        self.assertEqual(len(ax.lines), 2)
        presentation["plt"].close(fig)

    def test_band_power_plot_splits_a_gap(self):
        presentation["REPORT_DATASET"] = None
        presentation["manifest"] = {"spectral_config": {"hop_samples": 1}}
        presentation["band_power_df"] = pd.DataFrame({
            "channel": ["A0"] * 4,
            "band": ["Theta"] * 4,
            "time_seconds": [0.0, 0.1, 1.0, 1.1],
            "relative": [0.1, 0.2, 0.3, 0.4],
        })
        fig, ax = presentation["plot_band_power"]("A0", "relative")
        self.assertEqual(len(ax.lines), 2)
        presentation["plt"].close(fig)

    def test_one_sample_quality_is_displayed_as_unreliable(self):
        presentation["manifest"] = {
            "statistics_basis": "quality: one-sample Processor per channel",
            "analysis_warnings": [],
        }
        self.assertFalse(presentation["_quality_is_reliable"]())


if __name__ == "__main__":
    unittest.main()
