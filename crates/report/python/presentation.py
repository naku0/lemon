"""Presentation only: stored PSD/band power, bounded full-file CSV scans."""
import csv
import json
import math
from pathlib import Path

MAX_BINS = 1200
MAX_COMPARISON = 10000
MAX_FIELD = 1024 * 1024
csv.field_size_limit(MAX_FIELD)


def load_json(path):
    if path.stat().st_size > (128 if path.name == "report-dataset.json" else 4) * 1024 * 1024:
        raise ValueError(f'{path}: JSON exceeds 4 MiB')
    return json.loads(path.read_text(encoding='utf-8'))


def records(path):
    if globals().get('DATASET') is not None:
        if path.name == 'spectrum.csv':
            yield from DATASET['psd']
            return
        if path.name == 'quality-events.csv':
            for event in DATASET['events']:
                ch, kind, b, count, first, last = event
                yield {'channel': ch, 'kind': kind, 'timestamp_ns': int(first*1e9), 'count': count, 'interval_end_ns': int(last*1e9)}
            return
    # Streaming rows; reject oversized lines before CSV allocation.
    with path.open(encoding='utf-8', newline='') as stream:
        def lines():
            while True:
                line = stream.readline(MAX_FIELD + 1)
                if not line:
                    return
                if len(line) > MAX_FIELD:
                    raise ValueError(f'{path}: CSV line exceeds 1 MiB')
                yield line
        yield from csv.DictReader(lines())


def number(value):
    if value is None or value == '':
        return None
    value = float(value)
    return value if math.isfinite(value) else None


def initialize():
    global ROOT, FIG, manifest, summary, channels, units, start, end, DATASET, MAX_BINS
    DATASET = None
    if "LEMON_DATA_ROOT" in globals():
        ROOT = LEMON_DATA_ROOT
        DATASET = load_json(ROOT / "report-dataset.json")
        manifest, summary = DATASET["manifest"], DATASET["summary"]
        channels = list(summary["channels"])
        units = manifest.get("units") or "a.u."
        start, end, MAX_BINS = DATASET["start"], DATASET["end"], DATASET["bins"]
        FIG = ROOT / "figures"
        FIG.mkdir(exist_ok=True)
        return
    ROOT = Path(load_json(Path('report-data.json'))['analysis_path'])
    FIG = Path('figures')
    FIG.mkdir(exist_ok=True)
    manifest = load_json(ROOT / 'manifest.json')
    summary = load_json(ROOT / 'summary.json')
    if manifest.get('analysis_format_version') != 1:
        raise ValueError('Unsupported AnalysisBundle version')
    channels = list(summary['channels'])
    if not 1 <= len(channels) <= 2:
        raise ValueError('Expected one or two channels')
    units = manifest.get('units') or 'a.u.'
    start, end = math.inf, -math.inf
    for row in records(ROOT / 'processed.csv'):
        t = int(row['timestamp_ns']) / 1e9
        start, end = min(start, t), max(end, t)
    if not math.isfinite(start):
        start, end = 0., 1.
    if end <= start:
        end = start + 1 / manifest['sample_rate_hz']


def bucket(t):
    return max(0, min(MAX_BINS - 1, int((t-start)/(end-start)*MAX_BINS)))


def aggregate(store, key, value):
    if value is None:
        return
    if key not in store:
        store[key] = [value, value]
    else:
        store[key][0] = min(store[key][0], value)
        store[key][1] = max(store[key][1], value)


def envelope(column):
    if globals().get("DATASET") is not None:
        return {(ch, b): [lo, hi] for ch, b, lo, hi in DATASET[column]}
    result = {}
    for row in records(ROOT / 'processed.csv'):
        aggregate(result, (row['channel'], bucket(int(row['timestamp_ns']) / 1e9)), number(row[column]))
    return result


def bars(ax, data, channel, label, color):
    # Disjoint vertical min/max strokes never connect samples across gaps,
    # even when a gap falls inside a display bin. Horizontal extent is not implied.
    points = sorted((b, lo, hi) for (ch, b), (lo, hi) in data.items() if ch == channel)
    if points:
        x = [start + (b+.5)*(end-start)/MAX_BINS for b, _, _ in points]
        ax.vlines(x, [v[1] for v in points], [v[2] for v in points], color=color, label=label, linewidth=.8)
        ax.scatter(x, [(v[1]+v[2])/2 for v in points], color=color, s=1)


def save(fig, filename):
    import matplotlib.pyplot as plt
    from IPython.display import Image, display
    fig.tight_layout()
    fig.savefig(FIG / filename, dpi=150)
    plt.close(fig)
    display(Image(filename=str(FIG / filename)))


def panels(title):
    import matplotlib.pyplot as plt
    fig, axes = plt.subplots(len(channels), 1, squeeze=False, figsize=(12, 3*len(channels)))
    fig.suptitle(title)
    return fig, axes[:, 0]


def table(data):
    import pandas as pd
    from IPython.display import display
    display(pd.DataFrame({'value': {k: str(v) for k, v in data.items()}}))


def overview():
    metadata = manifest.get('source_metadata', {})
    table({'recording_name': metadata.get('config', {}).get('session', {}).get('name'),
           'source_metadata_started_unix_ms': metadata.get('started_unix_ms')})
    table({**{k: manifest.get(k) for k in ['source', 'source_format', 'created_at', 'channels', 'units', 'sample_rate_hz', 'sample_rate_source', 'source_hash', 'lemon_version']}, **{k: summary.get(k) for k in ['duration_seconds', 'sample_count_by_channel', 'continuous_segment_count']}})


def provenance():
    table({k: manifest.get(k) for k in ['source_metadata', 'time_normalization', 'source_hashes', 'import_warnings']})


def processing():
    table({k: manifest.get(k) for k in ['processing_config', 'filter_sequence', 'filter_order', 'spectral_config', 'configured_bands', 'baseline', 'statistics_basis']})


def waveform(column):
    fig, axes = panels(column.title() + ' — full-recording min/max bins (disjoint strokes)')
    data = envelope(column)
    event_bins = set()
    for r in records(ROOT / 'quality-events.csv'):
        if r['kind'] in ['LostSample', 'InvalidPacket', 'Disconnected', 'Saturation', 'Outlier']:
            event_bins.add((r['channel'], bucket(int(r['timestamp_ns']) / 1e9)))
    for ch, ax in zip(channels, axes):
        bars(ax, data, ch, column, '#0072B2' if column == 'filtered' else '#777777')
        for event_channel, b in sorted(event_bins):
            if event_channel not in ('', ch):
                continue
            ax.axvline(start+(b+.5)*(end-start)/MAX_BINS, color='#D55E00', alpha=.07)
        ax.set(xlabel='Time (s)', ylabel=f'{ch} ({units})', xlim=(start,end))
        ax.legend()
    save(fig, column+'-overview.png')


def comparison():
    if globals().get('DATASET') is not None:
        fig, axes = panels('Raw / Filtered envelope comparison — presentation bins, not sample-level data')
        for ch, ax in zip(channels, axes):
            bars(ax, envelope('raw'), ch, 'Raw envelope', '#777777')
            bars(ax, envelope('filtered'), ch, 'Filtered envelope', '#0072B2')
            ax.set(xlabel='Time (s)', ylabel=f'{ch} ({units})', xlim=(start, end))
            ax.legend()
        save(fig, 'raw-filtered-comparison.png')
        return
    selected = {ch: [] for ch in channels}
    previous, finished = {}, set()
    fs = manifest['sample_rate_hz']
    for r in records(ROOT / 'processed.csv'):
        ch = r['channel']
        if ch in finished: continue
        t, seq, raw, filtered = int(r['timestamp_ns'])/1e9, int(r['sequence']), number(r['raw']), number(r['filtered'])
        discontinuous = ch in previous and (seq != previous[ch][0]+1 or not 0 < t-previous[ch][1] <= 1.5/fs)
        flagged = any(f in r['flags'].split('|') for f in ['LostSample', 'InvalidPacket', 'Disconnected'])
        if selected[ch] and (filtered is None or discontinuous or flagged or t-selected[ch][0][0]>5 or len(selected[ch])>=MAX_COMPARISON):
            finished.add(ch)
        elif filtered is not None:
            selected[ch].append((t, raw, filtered))
        previous[ch] = (seq,t)
    fig, axes = panels('First valid continuous segment, at most 5 s / 10000 samples')
    for ch,ax in zip(channels,axes):
        p=selected[ch]
        if p:
            ax.plot([v[0] for v in p],[v[1] for v in p],color='#777777',alpha=.5,label='Raw')
            ax.plot([v[0] for v in p],[v[2] for v in p],color='#0072B2',label='Filtered')
            ax.legend()
        ax.set(xlabel='Time (s)',ylabel=f'{ch} ({units})')
    save(fig,'raw-filtered-comparison.png')


def spectrum():
    totals = {}
    for r in records(ROOT/'spectrum.csv'):
        key=(r['channel'], float(r['frequency_hz']))
        if key not in totals:
            if len(totals)>=262144: raise ValueError('Too many PSD bins')
            totals[key]=[0.,0.,0]
        a=totals[key]; a[0]+=float(r['raw_psd']); a[1]+=float(r['filtered_psd']); a[2]+=1
    fig,axes=panels('Mean of every stored PSD window; no FFT recomputation')
    for ch,ax in zip(channels,axes):
        p=sorted((f,a) for (c,f),a in totals.items() if c==ch)
        ax.plot([f for f,a in p],[a[0]/a[2] for f,a in p],color='#777777',label='Raw PSD')
        ax.plot([f for f,a in p],[a[1]/a[2] for f,a in p],color='#0072B2',label='Filtered PSD')
        seen=set()
        for b in manifest['configured_bands']:
            bounds=(b['low_hz'],b['high_hz'])
            if bounds not in seen: ax.axvspan(*bounds,alpha=.07,color='#009E73',label=b['name'])
            seen.add(bounds)
        cfg=manifest['processing_config']
        if cfg['notch']: ax.axvline(cfg['notch_hz'],color='#D55E00',linestyle=':',label='Notch')
        peak=summary['channels'][ch].get('dominant_peak_frequency_hz')
        if peak is not None: ax.axvline(peak,color='#CC79A7',linestyle='--',label=f'Summary peak {peak:g} Hz')
        ax.set(xlabel='Frequency (Hz)',ylabel=f'{ch}: {units}²/Hz'); ax.legend(fontsize=8)
    save(fig,'spectrum.png')


def bands():
    import matplotlib.pyplot as plt
    measures=['absolute','relative','baseline_change_pct']
    values={m:{} for m in measures}; names=set()
    if globals().get('DATASET') is not None:
        names = set(DATASET['band_names'])
        values = {m: {(ch, b): [lo, hi] for ch, b, lo, hi in DATASET['bands'][m]} for m in measures}
    else:
        for r in records(ROOT/'band-power.csv'):
            name=r['band']; names.add(name)
            if len(names)>32: raise ValueError('Too many bands')
            key=(r['channel']+' / '+name,bucket(int(r['window_end_ns'])/1e9))
            for m in measures: aggregate(values[m],key,number(r[m]))
    fig,axs=plt.subplots(len(channels),3,squeeze=False,figsize=(16,3*len(channels)))
    colors=['#0072B2','#D55E00','#009E73','#CC79A7']
    for i,ch in enumerate(channels):
        for j,m in enumerate(measures):
            ax=axs[i,j]
            for k,name in enumerate(sorted(names)): bars(ax,values[m],ch+' / '+name,name,colors[k%len(colors)])
            ax.set(xlabel='Time (s)',ylabel=f'{ch}: {m}',xlim=(start,end))
            if values[m]: ax.legend(fontsize=8)
            else: ax.text(.2,.5,'No baseline / no complete windows',transform=ax.transAxes)
    save(fig,'band-power-history.png')
    for ch in channels: table({ch:summary['channels'][ch]['band_statistics']})


def quality():
    fig,axes=panels('Quality events (time bins); engineering heuristic')
    events=set()
    allowed={'LostSample','InvalidPacket','Disconnected','Outlier','Saturation','LowQuality','ChannelMismatch','InsufficientData','ImportWarning','SegmentStart'}
    for r in records(ROOT/'quality-events.csv'):
        kind=r['kind'] if r['kind'] in allowed else 'Other'
        events.add((r['channel'],kind,bucket(int(r['timestamp_ns'])/1e9)))
        if len(events)>100000: raise ValueError('Too many channel/event categories')
    for ch,ax in zip(channels,axes):
        for kind in sorted(allowed|{'Other'}):
            x=[start+(b+.5)*(end-start)/MAX_BINS for c,k,b in sorted(events) if k==kind and c in ['',ch]]
            if x: ax.scatter(x,[kind]*len(x),s=8)
        ax.set(xlabel='Time (s)',ylabel=ch,xlim=(start,end))
        table({ch:summary['channels'][ch]['quality_distribution']})
    save(fig,'quality-timeline.png')
    table({k:summary.get(k) for k in ['lost_sample_count','invalid_packet_count','outlier_count','saturation_count','disconnection_count','continuous_segment_count']})


def technical_summary():
    for ch in channels:
        v=summary['channels'][ch]
        print(f'Channel {ch}: {summary["valid_filtered_count_by_channel"][ch]} valid filtered samples.')
        print('Filtered spectral peak (aggregation defined in manifest):',v.get('dominant_peak_frequency_hz'),'Hz')
        print('Change of power around configured notch (% reduction):',v.get('mains_reduction_percent'))
        if ch in summary.get('dominant_band_by_channel',{}): print(summary['dominant_band_by_channel'][ch])


def warning_messages():
    items=manifest.get('analysis_warnings',[])+manifest.get('import_warnings',[])+summary.get('warnings',[])
    if 'estimated' in str(manifest.get('sample_rate_source','')).lower(): items.append('Sample rate is estimated, not a hardware specification.')
    if not manifest.get('units'): items.append('Units unknown; amplitude shown in a.u.')
    if not any(number(v) is not None for row in (manifest.get('baseline') or []) for v in row): items.append('No baseline is configured.')
    if summary.get('lost_sample_count', 0): items.append('Missing samples were detected; gaps are not filled with zeros.')
    if any(c.get('quality_distribution', {}).get('Poor', {}).get('count', 0) for c in summary.get('channels', {}).values()): items.append('Some samples were marked Poor by the configured engineering heuristic.')
    ranges=manifest.get('configured_bands',[])
    if any(a['low_hz']<b['high_hz'] and b['low_hz']<a['high_hz'] for i,a in enumerate(ranges) for b in ranges[i+1:]): items.append('Overlapping ranges are not independent. Alpha/Mu semantics require electrode and experimental context.')
    items += ['Signal quality is a LEMON engineering heuristic and is not a clinically validated metric.', 'LEMON is not a medical device.']
    return list(dict.fromkeys(items))


def warnings():
    for text in warning_messages(): print('WARNING:', text)
