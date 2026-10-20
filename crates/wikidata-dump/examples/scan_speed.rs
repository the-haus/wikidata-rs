//! Time the bzip2 block scanner alone (no decoding).
use std::time::Instant;
fn main() {
    let path = std::env::args().nth(1).expect("usage: scan_speed FILE.bz2");
    let t = Instant::now();
    let f = std::fs::File::open(&path).unwrap();
    let mut n = 0u64;
    let mut bytes = 0u64;
    for p in wikidata_dump::bz2::Scanner::new(f).unwrap() {
        match p {
            Ok(p) => {
                n += 1;
                bytes += p.nbits / 8;
            }
            Err(e) => {
                eprintln!("{e}");
                break;
            }
        }
    }
    let s = t.elapsed().as_secs_f64();
    println!(
        "{n} pieces, {:.1} MB, {:.0} MB/s",
        bytes as f64 / 1e6,
        bytes as f64 / 1e6 / s
    );
}
