"""Untrusted embedded-data extraction tests; stdlib only, no installations."""
import hashlib
import io
from pathlib import Path
import tempfile
import unittest
import zipfile

code = (Path(__file__).resolve().parents[1]/'crates/report/python/unpack.py').read_text().split('LEMON_DATA_ROOT =')[0]
namespace = {}
exec(code, namespace)
unpack = namespace['unpack_report']


def archive(name='report-dataset.json', data=b'{}', symlink=False):
    output = io.BytesIO()
    with zipfile.ZipFile(output, 'w', compression=zipfile.ZIP_DEFLATED) as z:
        info = zipfile.ZipInfo(name)
        if symlink:
            info.create_system = 3
            info.external_attr = 0o120777 << 16
        z.writestr(info, data)
    return output.getvalue()


class Unpack(unittest.TestCase):
    def extract(self, content, maximum=1024, digest=None):
        with tempfile.TemporaryDirectory() as d:
            result = unpack(content, digest or hashlib.sha256(content).hexdigest(), maximum, Path(d))
            return {p.name: p.read_bytes() for p in result.iterdir()}

    def test_valid(self):
        self.assertEqual(self.extract(archive()), {'report-dataset.json': b'{}'})

    def test_traversal_absolute_and_symlink(self):
        for name in ['../outside', '/outside', 'C:\\outside', 'unexpected.py']:
            with self.subTest(name=name), self.assertRaises(ValueError):
                self.extract(archive(name))
        with self.assertRaises(ValueError):
            self.extract(archive(symlink=True))

    def test_size_and_checksum(self):
        with self.assertRaises(ValueError):
            self.extract(archive(data=b'x'*2048), maximum=1024)
        with self.assertRaises(ValueError):
            self.extract(archive(), digest='0'*64)
        with self.assertRaises(zipfile.BadZipFile):
            self.extract(b'not zip')


if __name__ == '__main__':
    unittest.main()
