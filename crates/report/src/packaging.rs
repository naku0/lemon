use super::{cell, Notebook, ReportError};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
};
use zip::{write::SimpleFileOptions, ZipWriter};

pub(crate) fn hash(path: &Path) -> Result<String, String> {
    let mut f = File::open(path).map_err(|e| e.to_string())?;
    let mut h = Sha256::new();
    let mut b = [0; 65536];
    loop {
        let n = f.read(&mut b).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        h.update(&b[..n]);
    }
    Ok(format!("{:x}", h.finalize()))
}
fn files(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    for e in fs::read_dir(dir).map_err(|e| e.to_string())? {
        let e = e.map_err(|e| e.to_string())?;
        let kind = e.file_type().map_err(|e| e.to_string())?;
        if kind.is_symlink() {
            return Err("report/archive: symlinks forbidden".into());
        }
        if kind.is_dir() {
            files(root, &e.path(), out)?;
        } else if kind.is_file() {
            out.push(
                e.path()
                    .strip_prefix(root)
                    .map_err(|e| e.to_string())?
                    .to_path_buf(),
            );
        }
    }
    out.sort();
    Ok(())
}
pub(crate) fn zip_files(root: &Path, paths: &[PathBuf], target: &Path) -> Result<(), String> {
    let mut zip = ZipWriter::new(File::create(target).map_err(|e| e.to_string())?);
    for path in paths {
        zip.start_file(
            path.to_string_lossy().replace('\\', "/"),
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated),
        )
        .map_err(|e| e.to_string())?;
        std::io::copy(
            &mut File::open(root.join(path)).map_err(|e| e.to_string())?,
            &mut zip,
        )
        .map_err(|e| e.to_string())?;
    }
    zip.finish().map_err(|e| e.to_string())?;
    Ok(())
}
pub(crate) fn portable(root: &Path, target: &Path, overwrite: bool) -> Result<(), String> {
    if target.exists() && !overwrite {
        return Err(format!(
            "report/archive: {} exists; use --overwrite",
            target.display()
        ));
    }
    fs::write(root.join("README.md"),"# LEMON report\n\nFull AnalysisBundle and sample-level Raw/Filtered: analysis/processed.csv. Open report.ipynb from this directory.\n\nCreate an environment: python -m venv .venv\nActivate: source .venv/bin/activate (Windows: .venv\\Scripts\\activate)\nInstall: python -m pip install -r report-requirements.txt\nExecute: jupyter nbconvert --execute --inplace report.ipynb\n\nReport Dataset means presentation aggregates, not a complete recording. This archive contains full sample-level data. No cognitive or medical interpretation is provided. Relative paths are portable. SHA256SUMS.json lists hashes of the other files.\n").map_err(|e|e.to_string())?;
    let mut list = Vec::new();
    files(root, root, &mut list)?;
    let hashes: Result<std::collections::BTreeMap<_, _>, String> = list
        .iter()
        .filter(|p| p.to_string_lossy() != "SHA256SUMS.json")
        .map(|p| Ok((p.to_string_lossy().replace('\\', "/"), hash(&root.join(p))?)))
        .collect();
    fs::write(
        root.join("SHA256SUMS.json"),
        serde_json::to_vec_pretty(&hashes?).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    if !list.contains(&PathBuf::from("SHA256SUMS.json")) {
        list.push("SHA256SUMS.json".into());
    }
    if let Ok(meta) = fs::symlink_metadata(target) {
        if !meta.file_type().is_file() {
            return Err("report/archive: refusing non-regular output".into());
        }
    }
    let staging = super::ReportStaging::new(target)?;
    let temp = staging.0.join("archive.zip");
    let result = (|| {
        zip_files(root, &list, &temp)?;
        let backup = staging.0.join("previous.zip");
        if target.exists() {
            fs::rename(target, &backup).map_err(|e| e.to_string())?;
        }
        if let Err(error) = fs::rename(&temp, target) {
            if backup.exists() {
                fs::rename(&backup, target).map_err(|restore| {
                    format!(
                        "report/archive: publish failed ({error}); restoring previous archive failed ({restore}); backup: {}",
                        backup.display()
                    )
                })?;
            }
            return Err(format!("report/archive: publish failed: {error}"));
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}
pub(crate) fn colab(
    root: &Path,
    analysis: &Path,
    include_raw: bool,
    limit: u64,
    book: &mut Notebook,
) -> Result<Value, ReportError> {
    let mut bins = 1200;
    let dataset_path = root.join("report-dataset.json");
    loop {
        let dataset = super::dataset::build(analysis, bins)?;
        fs::write(
            &dataset_path,
            serde_json::to_vec(&dataset).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        if fs::metadata(&dataset_path)
            .map_err(|e| e.to_string())?
            .len()
            * 4
            / 3
            < limit
            || bins <= 32
        {
            break;
        }
        bins = (bins / 2).max(32);
    }
    let mut list = vec![PathBuf::from("report-dataset.json")];
    let dataset_size = fs::metadata(&dataset_path)
        .map_err(|e| e.to_string())?
        .len();
    let raw_size = fs::metadata(analysis.join("processed.csv"))
        .map_err(|e| e.to_string())?
        .len();
    let full = include_raw && (dataset_size + raw_size + 1024).saturating_mul(4) / 3 <= limit;
    let mut raw_archive = None;
    if include_raw {
        eprintln!(
            "LEMON: full processed.csv size: {raw_size} bytes; embedding limit: {limit} bytes"
        );
        if full {
            fs::copy(analysis.join("processed.csv"), root.join("processed.csv"))
                .map_err(|e| e.to_string())?;
            list.push("processed.csv".into());
        } else {
            let p = root.join("full-raw.zip");
            zip_files(analysis, &["processed.csv".into()], &p)?;
            raw_archive = Some("full-raw.zip");
        }
    }
    let archive = root.join("colab-data.zip");
    zip_files(root, &list, &archive)?;
    let size = fs::metadata(&archive).map_err(|e| e.to_string())?.len();
    let digest = hash(&archive)?;
    let embedded = size.saturating_add(2) / 3 * 4 <= limit
        && dataset_size + if full { raw_size } else { 0 } <= limit;
    let encoded = if embedded {
        STANDARD.encode(fs::read(&archive).map_err(|e| e.to_string())?)
    } else {
        String::new()
    };
    let config = json!({"payload":"","sha256":digest,"max_bytes":if embedded{limit}else{dataset_size+if full{raw_size}else{0}+1024},"archive":"colab-data.zip"});
    // Only JSON encoded again into base64 is inserted into executable source.
    let config = STANDARD.encode(serde_json::to_vec(&config).map_err(|e| e.to_string())?);
    let bootstrap = format!(
        "import base64, json\nLEMON_PAYLOAD = json.loads(base64.b64decode('{config}'))\nLEMON_PAYLOAD['payload'] = '{encoded}'\n{}",
        include_str!("../python/unpack.py")
    );
    book.cells.insert(1,cell("markdown",&format!("## Data availability\n\nThis notebook contains a Report Dataset prepared by LEMON for visualization of previously calculated results. The numerical analysis was performed by LEMON using the processing configuration recorded in the embedded manifest.\n\nFull sample-level data:\n- Embedded: {}\n- Separate archive required: {}\n\nReport Dataset embedded: {}. Min/max bins: {}. No filtering, FFT, band power or quality recalculation is performed.\n\nThis report presents numerical signal characteristics. It does not provide neurophysiological, cognitive or medical interpretation.",if full&&embedded{"yes"}else{"no"},if raw_archive.is_some()||!embedded{"yes"}else{"no"},embedded,bins)));
    book.cells.insert(2, cell("code", &bootstrap));
    book.cells.insert(
        3,
        cell("code", include_str!("../python/optional_dependencies.py")),
    );
    for (i, c) in book.cells.iter_mut().enumerate() {
        c.id = format!("lemon-{i}");
    }
    if embedded {
        fs::remove_file(&archive).map_err(|e| e.to_string())?;
    }
    Ok(
        json!({"report_dataset_embedded":embedded,"embedded_compressed_bytes":if embedded{size}else{0},"embedded_uncompressed_bytes":if embedded{dataset_size+if full{raw_size}else{0}}else{0},"embedded_sha256":digest,"external_data_archive":if !embedded{Some("colab-data.zip")}else{raw_archive},"external_raw_archive":raw_archive,"full_raw_included":include_raw,"full_raw_embedded":full&&embedded}),
    )
}
