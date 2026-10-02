"""No dependencies needed for ingestion/envelope tests; plotting is exercised by report_smoke."""
import importlib.util
from pathlib import Path
import tempfile
import unittest

spec=importlib.util.spec_from_file_location('presentation',Path(__file__).resolve().parents[1]/'crates/report/python/presentation.py')
p=importlib.util.module_from_spec(spec)
spec.loader.exec_module(p)

class Presentation(unittest.TestCase):
    def test_envelope_keeps_peak_and_never_fills_missing(self):
        with tempfile.TemporaryDirectory() as d:
            p.ROOT=Path(d);p.start=0.;p.end=2.
            (p.ROOT/'processed.csv').write_text('timestamp_ns,channel,raw,filtered\n0,C3,1,1\n1000,C3,900,\n2000,C3,-7,-7\n1000000000,C3,3,\n')
            self.assertEqual(p.envelope('raw')[('C3',0)],[-7.,900.])
            self.assertEqual(p.envelope('filtered')[('C3',0)],[-7.,1.])
            self.assertNotIn(('C3',600),p.envelope('filtered'))
    def test_entire_file_is_consumed_with_bounded_envelope(self):
        with tempfile.TemporaryDirectory() as d:
            p.ROOT=Path(d);p.start=0.;p.end=2.
            with (p.ROOT/'processed.csv').open('w') as f:
                f.write('timestamp_ns,channel,raw,filtered\n')
                for i in range(1000010): f.write(f'{i*1000},C3,{999 if i==1000009 else 1},\n')
            data=p.envelope('raw')
            self.assertLessEqual(len(data),p.MAX_BINS)
            self.assertEqual(max(v[1] for v in data.values()),999)
    def test_warnings_follow_actual_conditions(self):
        p.manifest = {'baseline': [[]], 'units': 'uV', 'sample_rate_source': 'confirmed', 'configured_bands': []}
        p.summary = {'channels': {}, 'lost_sample_count': 0}
        clean = p.warning_messages()
        self.assertTrue(any('No baseline' in v for v in clean))
        self.assertFalse(any('Missing samples' in v or 'marked Poor' in v for v in clean))
        p.summary = {'channels': {'C3': {'quality_distribution': {'Poor': {'count': 2}}}}, 'lost_sample_count': 1}
        affected = p.warning_messages()
        self.assertTrue(any('Missing samples' in v for v in affected))
        self.assertTrue(any('marked Poor' in v for v in affected))

    def test_numeric_missing_is_not_zero(self):
        self.assertIsNone(p.number(''));self.assertIsNone(p.number('nan'));self.assertEqual(p.number('0'),0.)

if __name__=='__main__': unittest.main()
