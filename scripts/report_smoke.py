#!/usr/bin/env python3
"""Offline generation always runs; execution requires an existing Python environment."""
import hashlib
import importlib.util
import json
import math
import pathlib
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
BINARY = ROOT / 'target/debug/lemon'


def fingerprints(path):
    result = {}
    for p in path.rglob('*'):
        if p.is_file():
            digest = hashlib.sha256()
            with p.open('rb') as f:
                for block in iter(lambda: f.read(65536), b''):
                    digest.update(block)
            result[str(p.relative_to(path))] = digest.hexdigest()
    return result


with tempfile.TemporaryDirectory(prefix='lemon-report-кириллица-') as temp:
    tmp = pathlib.Path(temp)
    source = tmp / 'signal.csv'
    with source.open('w') as f:
        f.write('time,ch1,ch2\n')
        for i in range(4096):
            if i == 1500:
                continue
            t = i/256
            v = sum(a*math.sin(2*math.pi*hz*t) for a,hz in [(2,6),(3,10),(1,20),(2,50)])
            f.write(f'{t},{v + (100 if i==2400 else 0)},{v*.7}\n')
    original = source.read_bytes()
    analysis = tmp / 'analysis'
    subprocess.run([BINARY,'analyze',source,'--output',analysis],cwd=ROOT,check=True,capture_output=True)
    before = fingerprints(analysis)
    report = tmp / 'отчёт с пробелом'
    subprocess.run([BINARY,'report','--analysis',analysis,'--output',report,'--no-execute'],cwd=ROOT,check=True,capture_output=True)
    book = json.loads((report/'report.ipynb').read_text())
    assert book['nbformat'] == 4 and book['nbformat_minor'] == 5
    for cell in book['cells']:
        assert cell['id']
        if cell['cell_type']=='code':
            assert cell['execution_count'] is None and not cell['outputs']
    assert json.loads((report/'report-manifest.json').read_text())['execution_status']=='not_executed'
    assert fingerprints(report/'analysis') == before, 'local report must contain full AnalysisBundle'
    assert fingerprints(analysis)==before and source.read_bytes()==original
    print('PASS report generation: schema fields, Unicode, portable local paths, unchanged input')
    import zipfile
    portable = tmp/'portable'
    subprocess.run([BINARY,'report','--analysis',analysis,'--output',portable,'--portable','--no-execute'],check=True,capture_output=True)
    archive=portable.with_suffix('.lemon-report.zip')
    relocated=tmp/'moved'
    with zipfile.ZipFile(archive) as z:
        assert 'analysis/processed.csv' in z.namelist()
        z.extractall(relocated)
    hashes=json.loads((relocated/'SHA256SUMS.json').read_text())
    for name,digest in hashes.items(): assert hashlib.sha256((relocated/name).read_bytes()).hexdigest()==digest
    for raw in [False,True]:
        colab=tmp/('colab-raw' if raw else 'colab')
        args=[BINARY,'report','--analysis',analysis,'--output',colab,'--colab']
        if raw: args.append('--include-raw')
        subprocess.run(args,check=True,capture_output=True)
        nb=json.loads((colab/'report-colab.ipynb').read_text())
        code='\n'.join(''.join(c['source']) for c in nb['cells'] if c['cell_type']=='code')
        assert 'fft(' not in code.lower() and 'welch(' not in code.lower()
        assert 'INSTALL_MISSING_REPORT_PACKAGES = False' in code
        assert source.as_posix() not in code and analysis.as_posix() not in code
        bootstrap=next(''.join(c['source']) for c in nb['cells'] if c['cell_type']=='code' and 'LEMON_PAYLOAD =' in ''.join(c['source']))
        # Decode and validate without requiring plotting dependencies.
        namespace={}
        exec(bootstrap,namespace)
        data=json.loads((namespace['LEMON_DATA_ROOT']/'report-dataset.json').read_text())
        assert data['summary']==json.loads((analysis/'summary.json').read_text())
        assert (namespace['LEMON_DATA_ROOT']/'processed.csv').exists()==raw
        assert data['raw'] and data['filtered'] and data['psd'] and data['events']
    import shutil, os
    # A deliberately large synthetic warning forces the bounded external-dataset path.
    large=tmp/'large-analysis'
    shutil.copytree(analysis,large)
    large_summary=json.loads((large/'summary.json').read_text())
    large_summary['warnings']=['synthetic warning '+('x'*1200000)]
    (large/'summary.json').write_text(json.dumps(large_summary))
    external=tmp/'external-colab'
    subprocess.run([BINARY,'report','--analysis',large,'--output',external,'--colab','--maximum-embedded-report-mb','1','--include-raw'],check=True,capture_output=True)
    em=json.loads((external/'report-manifest.json').read_text())
    assert not em['report_dataset_embedded'] and em['external_data_archive']
    assert (external/'colab-data.zip').is_file() and (external/'full-raw.zip').is_file()
    nb=json.loads((external/'report-colab.ipynb').read_text())
    bootstrap=next(''.join(c['source']) for c in nb['cells'] if c['cell_type']=='code' and 'LEMON_PAYLOAD =' in ''.join(c['source']))
    previous=os.getcwd()
    try:
        os.chdir(external)
        namespace={}
        exec(bootstrap,namespace)
        assert json.loads((namespace['LEMON_DATA_ROOT']/'report-dataset.json').read_text())['summary']==large_summary
    finally:
        os.chdir(previous)
    assert fingerprints(analysis)==before and source.read_bytes()==original
    print('PASS portable ZIP hashes/relocation and self-contained Colab dataset, with/without full raw')
    missing=[n for n in ['numpy','pandas','matplotlib','nbformat','nbclient','ipykernel'] if importlib.util.find_spec(n) is None]
    if missing:
        print('SKIP notebook execution: missing '+', '.join(missing))
    else:
        import nbformat
        nbformat.validate(nbformat.read(report/'report.ipynb',as_version=4))
        executed = tmp/'executed'
        subprocess.run([BINARY,'report','--analysis',analysis,'--output',executed,'--execute','--python',sys.executable],cwd=ROOT,check=True,timeout=1800)
        nb=nbformat.read(executed/'report.ipynb',as_version=4); nbformat.validate(nb)
        for cell in nb.cells:
            if cell.cell_type=='code':
                assert cell.execution_count is not None
                assert all(o.output_type!='error' for o in cell.outputs)
        assert len(list((executed/'figures').glob('*.png')))==6
        assert fingerprints(analysis)==before and source.read_bytes()==original
        print('PASS executed notebook: six PNG, every cell executed, no error, input unchanged')
