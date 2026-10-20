#![allow(dead_code)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use wikidata_export::{entities, Dump, EntityOptions, Event, Flow, Record, ScanStats};

pub fn sample_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/sample.json")
}

pub fn sample() -> Vec<u8> {
    std::fs::read(sample_path()).unwrap()
}

/// The entity lines of the sample, without commas.
pub fn sample_lines() -> Vec<Vec<u8>> {
    sample()
        .split(|&b| b == b'\n')
        .map(|l| l.strip_suffix(b",").unwrap_or(l).to_vec())
        .filter(|l| l.first() == Some(&b'{'))
        .collect()
}

static N: AtomicU64 = AtomicU64::new(0);

/// A fresh temporary directory.
pub fn tmp() -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "wikidata-rs-test-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

pub fn bz2(data: &[u8], level: u32) -> Vec<u8> {
    let mut e = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::new(level));
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

pub fn write(dir: &Path, name: &str, data: &[u8]) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, data).unwrap();
    p
}

/// Every record of a dump, with `opts`.
pub fn collect(dump: &Dump, opts: &EntityOptions) -> (Vec<Record>, ScanStats) {
    let mut out = Vec::new();
    let stats = entities(dump, opts, &mut |e| {
        if let Event::Records(rs) = e {
            out.extend(rs);
        }
        Flow::Continue
    })
    .unwrap();
    (out, stats)
}

pub fn ids(rs: &[Record]) -> Vec<String> {
    rs.iter().map(|r| r.entity.id.clone()).collect()
}
