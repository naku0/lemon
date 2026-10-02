# Safe extraction of LEMON-generated data, never executable files.
import hashlib
import io
import sys
import tempfile
import zipfile
from pathlib import Path, PurePosixPath


def unpack_report(payload, checksum, maximum, destination):
    if len(payload) > maximum:
        raise ValueError('LEMON archive exceeds size limit')
    if hashlib.sha256(payload).hexdigest() != checksum:
        raise ValueError('LEMON archive checksum mismatch')
    with zipfile.ZipFile(io.BytesIO(payload)) as archive:
        entries = archive.infolist()
        if not 1 <= len(entries) <= 2:
            raise ValueError('Unexpected archive file count')
        seen = set()
        total = 0
        for entry in entries:
            name = PurePosixPath(entry.filename)
            if entry.filename not in {'report-dataset.json', 'processed.csv'} or name.is_absolute() or '..' in name.parts or '\\' in entry.filename:
                raise ValueError('Unsafe archive path')
            if entry.filename in seen or entry.is_dir() or ((entry.external_attr >> 16) & 0o170000) == 0o120000:
                raise ValueError('Duplicate, directory or symlink in archive')
            seen.add(entry.filename)
            total += entry.file_size
            if total > maximum:
                raise ValueError('Uncompressed data exceeds size limit')
        for entry in entries:
            written = 0
            with archive.open(entry) as source, (destination / entry.filename).open('xb') as target:
                while True:
                    chunk = source.read(65536)
                    if not chunk:
                        break
                    written += len(chunk)
                    if written > entry.file_size or written > maximum:
                        raise ValueError('Decompressed size mismatch')
                    target.write(chunk)
    return destination


def load_embedded_report(config):
    if config['payload']:
        if len(config['payload']) > config['max_bytes']:
            raise ValueError('Encoded payload exceeds size limit')
        payload = base64.b64decode(config['payload'], validate=True)
    else:
        try:
            from google.colab import files
        except ImportError:
            archive_path = Path(config['archive'])
            if not archive_path.is_file():
                raise FileNotFoundError('Place colab-data.zip beside this notebook before Run all')
            if archive_path.stat().st_size > config['max_bytes']:
                raise ValueError('Archive too large')
            payload = archive_path.read_bytes()
        else:
            uploaded = files.upload()
            if len(uploaded) != 1:
                raise ValueError('Upload exactly one LEMON colab-data.zip')
            payload = next(iter(uploaded.values()))
    directory = Path(tempfile.mkdtemp(prefix='lemon-report-'))
    return unpack_report(payload, config['sha256'], config['max_bytes'], directory)

LEMON_DATA_ROOT = load_embedded_report(LEMON_PAYLOAD)
