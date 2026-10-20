//! Decode every block of a bzip2 file with both decoders (this crate's
//! and libbzip2), check that they agree, and time them on one thread.
//!
//!   cargo run --release -p wikidata-dump --example bz2_compare -- FILE.bz2

use std::time::Instant;

use wikidata_dump::bz2::{decode_block_libbzip2, PieceKind, Scanner};

fn main() {
    let path = std::env::args().nth(1).expect("usage: bz2_compare FILE.bz2");
    let f = std::fs::File::open(&path).unwrap();
    let pieces: Vec<_> = Scanner::new(f)
        .unwrap()
        .map_while(|p| p.ok())
        .filter(|p| p.kind == PieceKind::Block)
        .collect();
    let (mut t_fast, mut t_lib, mut bytes) = (0f64, 0f64, 0usize);
    for p in &pieces {
        let t = Instant::now();
        let mut fast = Vec::new();
        wikidata_dump::bzdecode::decode_block(&p.bits, p.nbits, &mut fast).expect("fast decoder");
        t_fast += t.elapsed().as_secs_f64();
        let t = Instant::now();
        let lib = decode_block_libbzip2(p, 0).expect("libbzip2");
        t_lib += t.elapsed().as_secs_f64();
        assert!(fast == lib, "blocks differ at bit {}", p.bit_offset);
        bytes += lib.len();
    }
    println!(
        "{} blocks agree, {:.1} MB out; this crate {:.2} s ({:.1} MB/s), libbzip2 {:.2} s ({:.1} MB/s): {:.2}x",
        pieces.len(),
        bytes as f64 / 1e6,
        t_fast,
        bytes as f64 / 1e6 / t_fast,
        t_lib,
        bytes as f64 / 1e6 / t_lib,
        t_lib / t_fast
    );
}
