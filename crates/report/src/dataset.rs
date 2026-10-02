use super::ReportError;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    path::Path,
};
type Envelope = BTreeMap<(String, usize), [f64; 2]>;
fn number(s: &str) -> Option<f64> {
    s.parse::<f64>().ok().filter(|v| v.is_finite())
}
fn add(store: &mut Envelope, key: (String, usize), v: Option<f64>) {
    if let Some(v) = v {
        let a = store.entry(key).or_insert([v, v]);
        a[0] = a[0].min(v);
        a[1] = a[1].max(v);
    }
}
fn rows(
    path: &Path,
    mut f: impl FnMut(&BTreeMap<String, String>) -> Result<(), String>,
) -> Result<(), String> {
    let budget = std::rc::Rc::new(std::cell::Cell::new(0usize));
    let input = BoundedRead {
        file: File::open(path).map_err(|e| e.to_string())?,
        bytes: budget.clone(),
    };
    let mut reader = csv::Reader::from_reader(input);
    let headers = reader.headers().map_err(|e| e.to_string())?.clone();
    let mut iterator = reader.records();
    loop {
        budget.set(0);
        let Some(row) = iterator.next() else {
            break;
        };
        let row = row.map_err(|e| format!("report/dataset {}: {e}", path.display()))?;
        if row.iter().any(|v| v.len() > 1024 * 1024) {
            return Err("report/dataset: field too large".into());
        }
        let row = headers
            .iter()
            .zip(row.iter())
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        f(&row)?;
    }
    Ok(())
}
fn field<'a>(r: &'a BTreeMap<String, String>, k: &str) -> Result<&'a str, String> {
    r.get(k)
        .map(String::as_str)
        .ok_or_else(|| format!("report/dataset: missing {k}"))
}
fn time(r: &BTreeMap<String, String>, k: &str) -> Result<f64, String> {
    field(r, k)?
        .parse::<u64>()
        .map(|v| v as f64 / 1e9)
        .map_err(|e| e.to_string())
}
fn encoded(map: Envelope) -> Vec<Value> {
    map.into_iter()
        .map(|((ch, b), v)| json!([ch, b, v[0], v[1]]))
        .collect()
}
pub(crate) fn build(root: &Path, bins: usize) -> Result<Value, ReportError> {
    let manifest: Value =
        serde_json::from_reader(File::open(root.join("manifest.json")).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let summary: Value =
        serde_json::from_reader(File::open(root.join("summary.json")).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let channels = summary["channels"]
        .as_object()
        .ok_or("report/dataset: channels missing")?;
    if !(1..=2).contains(&channels.len()) {
        return Err("report/dataset: expected one or two channels".into());
    }
    let mut start = f64::INFINITY;
    let mut end = f64::NEG_INFINITY;
    rows(&root.join("processed.csv"), |r| {
        let t = time(r, "timestamp_ns")?;
        start = start.min(t);
        end = end.max(t);
        Ok(())
    })?;
    if !start.is_finite() {
        start = 0.;
        end = 1.;
    }
    if end <= start {
        end = start + 1.;
    }
    let bucket =
        |t: f64| (((t - start) / (end - start) * bins as f64).max(0.) as usize).min(bins - 1);
    let mut raw = Envelope::new();
    let mut filtered = Envelope::new();
    rows(&root.join("processed.csv"), |r| {
        let ch = field(r, "channel")?;
        if !channels.contains_key(ch) {
            return Err("report/dataset: unknown channel".into());
        }
        let b = bucket(time(r, "timestamp_ns")?);
        add(&mut raw, (ch.into(), b), number(field(r, "raw")?));
        add(&mut filtered, (ch.into(), b), number(field(r, "filtered")?));
        Ok(())
    })?;
    let mut psd: BTreeMap<(String, String), (f64, f64, u64)> = BTreeMap::new();
    rows(&root.join("spectrum.csv"), |r| {
        let ch = field(r, "channel")?;
        if !channels.contains_key(ch) {
            return Err("report/dataset: unknown PSD channel".into());
        }
        let frequency =
            number(field(r, "frequency_hz")?).ok_or("report/dataset: invalid frequency")?;
        let k = (ch.to_string(), frequency.to_string());
        if !psd.contains_key(&k) && psd.len() >= 262144 {
            return Err("report/dataset: too many PSD bins".into());
        }
        let a = psd.entry(k).or_default();
        a.0 += number(field(r, "raw_psd")?).ok_or("invalid raw PSD")?;
        a.1 += number(field(r, "filtered_psd")?).ok_or("invalid filtered PSD")?;
        a.2 += 1;
        Ok(())
    })?;
    let psd:Vec<_>=psd.into_iter().map(|((ch,f),(r,v,n))|json!({"channel":ch,"frequency_hz":f,"raw_psd":r/n as f64,"filtered_psd":v/n as f64})).collect();
    let mut bands: BTreeMap<String, Envelope> = ["absolute", "relative", "baseline_change_pct"]
        .into_iter()
        .map(|s| (s.to_string(), Envelope::new()))
        .collect();
    let mut names = BTreeSet::new();
    rows(&root.join("band-power.csv"), |r| {
        let name = field(r, "band")?;
        if name.len() > 128 || !channels.contains_key(field(r, "channel")?) {
            return Err("report/dataset: invalid band/channel label".into());
        }
        names.insert(name.to_string());
        if names.len() > 32 {
            return Err("report/dataset: too many bands".into());
        }
        let key = (
            format!("{} / {}", field(r, "channel")?, name),
            bucket(time(r, "window_end_ns")?),
        );
        for (m, values) in &mut bands {
            add(values, key.clone(), number(field(r, m)?));
        }
        Ok(())
    })?;
    let bands: BTreeMap<_, _> = bands.into_iter().map(|(m, v)| (m, encoded(v))).collect();
    let mut events: BTreeMap<(String, String, usize), (u64, f64, f64)> = BTreeMap::new();
    rows(&root.join("quality-events.csv"), |r| {
        let t = time(r, "timestamp_ns")?;
        let ch = field(r, "channel")?;
        let kind = field(r, "kind")?;
        if ch.len() > 128 || kind.len() > 128 {
            return Err("report/dataset: event label too long".into());
        }
        let entry = events
            .entry((ch.to_string(), kind.to_string(), bucket(t)))
            .or_insert((0, t, t));
        entry.0 += 1;
        entry.1 = entry.1.min(t);
        entry.2 = entry.2.max(t);
        if events.len() > 100000 {
            return Err("report/dataset: too many events".into());
        }
        Ok(())
    })?;
    let events: Vec<_> = events
        .into_iter()
        .map(|((ch, kind, b), (count, first, last))| json!([ch, kind, b, count, first, last]))
        .collect();
    Ok(
        json!({"report_dataset_version":1,"purpose":"Visualization only; not full sample-level data", "manifest":manifest,"summary":summary,"start":start,"end":end,"bins":bins,"raw":encoded(raw),"filtered":encoded(filtered),"psd":psd,"bands":bands,"band_names":names,"events":events}),
    )
}

struct BoundedRead {
    file: File,
    bytes: std::rc::Rc<std::cell::Cell<usize>>,
}
impl std::io::Read for BoundedRead {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let n = std::io::Read::read(&mut self.file, buffer)?;
        let bytes = self.bytes.get().saturating_add(n);
        if bytes > 2 * 1024 * 1024 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "CSV record exceeds 2 MiB",
            ));
        }
        self.bytes.set(bytes);
        Ok(n)
    }
}
