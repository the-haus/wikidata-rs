//! Count the entity lines of a dump and time the read.
//!
//!   cargo run --release -p wikidata-dump --example count_lines -- DUMP [THREADS] [--allow-truncated]

use std::time::Instant;

use wikidata_dump::{Dump, ReadOptions};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let path = args
        .first()
        .ok_or("usage: count_lines DUMP [THREADS] [--allow-truncated]")?;
    let threads = args.get(1).and_then(|t| t.parse().ok()).unwrap_or(0);
    let opts = ReadOptions {
        threads,
        allow_truncated: args.iter().any(|a| a == "--allow-truncated"),
    };
    let t = Instant::now();
    let dump = Dump::open(path.as_str())?;
    let chunks = dump.chunks(&opts)?;
    let stats = chunks.stats();
    let mut lines = 0u64;
    for c in chunks {
        lines += c?.lines().count() as u64;
    }
    let s = t.elapsed().as_secs_f64();
    let bin = stats.bytes_in.get() as f64;
    let bout = stats.bytes_out.load(std::sync::atomic::Ordering::Relaxed) as f64;
    println!(
        "{} lines, {:.1} MB in ({:.1} MB/s), {:.1} MB out ({:.1} MB/s), {} blocks, {:.2} s{}",
        lines,
        bin / 1e6,
        bin / 1e6 / s,
        bout / 1e6,
        bout / 1e6 / s,
        stats.blocks.load(std::sync::atomic::Ordering::Relaxed),
        s,
        if stats.truncated.load(std::sync::atomic::Ordering::Relaxed) {
            " (truncated)"
        } else {
            ""
        }
    );
    Ok(())
}
