//! Reading resumes where a chunk's lines end, with nothing lost or read twice.

use std::io::Write;

use wikidata_dump::{Dump, ReadOptions};

/// A JSON-array dump of `n` entity-like lines; every 50th is longer than
/// a 100 kB bzip2 block.
fn dump_text(n: usize) -> Vec<u8> {
    let mut out = b"[\n".to_vec();
    let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
    for i in 0..n {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let pad = if i % 50 == 7 { 150_000 } else { (x % 2000) as usize };
        let filler: String = (0..pad)
            .map(|k| (b'a' + ((x >> (k % 50)) % 26) as u8) as char)
            .collect();
        write!(out, "{{\"type\":\"item\",\"id\":\"Q{i}\",\"v\":\"{filler}\"}}").unwrap();
        out.extend_from_slice(if i + 1 < n { b",\n" } else { b"\n" });
    }
    out.extend_from_slice(b"]\n");
    out
}

fn lines(
    dump: &Dump,
    resume: Option<wikidata_dump::ResumePoint>,
) -> Vec<(Vec<String>, Option<wikidata_dump::ResumePoint>)> {
    let opts = ReadOptions {
        threads: 3,
        resume,
        ..Default::default()
    };
    dump.chunks(&opts)
        .unwrap()
        .map(|c| {
            let c = c.unwrap();
            let ids = c
                .lines()
                .map(|l| String::from_utf8_lossy(&l[..l.len().min(24)]).into_owned())
                .collect();
            (ids, c.resume)
        })
        .collect()
}

fn check(path: &std::path::Path, n: usize, min_chunks: usize) {
    let dump = Dump::open(path).unwrap();
    let chunks = lines(&dump, None);
    let all: Vec<String> = chunks.iter().flat_map(|c| c.0.clone()).collect();
    assert_eq!(all.len(), n);
    assert!(chunks.len() >= min_chunks, "{}", chunks.len());
    let mut done = 0;
    for (k, (ids, resume)) in chunks.iter().enumerate() {
        done += ids.len();
        let Some(r) = resume else { continue };
        if k % 3 != 0 && min_chunks > 5 {
            continue;
        }
        let rest: Vec<String> = lines(&dump, Some(*r)).into_iter().flat_map(|c| c.0).collect();
        assert_eq!(rest, all[done..], "resuming after chunk {k}");
    }
}

#[test]
fn bzip2_resumes_at_blocks() {
    let dir = std::env::temp_dir().join(format!("wikidata-dump-resume-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let text = dump_text(600);
    let mut e = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::new(1));
    e.write_all(&text).unwrap();
    let p = dir.join("a.json.bz2");
    std::fs::write(&p, e.finish().unwrap()).unwrap();
    check(&p, 600, 10);
    // Several streams, as pbzip2 and this package's extracts write.
    let mut multi = Vec::new();
    for part in text.chunks(text.len() / 4 + 1) {
        let mut e = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::new(1));
        e.write_all(part).unwrap();
        multi.extend(e.finish().unwrap());
    }
    let p = dir.join("multi.json.bz2");
    std::fs::write(&p, multi).unwrap();
    check(&p, 600, 10);
    // Plain JSON, read in 4 MB pieces.
    let p = dir.join("a.json");
    std::fs::write(&p, dump_text(3000)).unwrap();
    check(&p, 3000, 3);
}

#[test]
fn a_resume_point_of_another_file_fails() {
    let dir = std::env::temp_dir().join(format!("wikidata-dump-resume2-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut e = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::new(1));
    e.write_all(&dump_text(600)).unwrap();
    let p = dir.join("a.json.bz2");
    std::fs::write(&p, e.finish().unwrap()).unwrap();
    let dump = Dump::open(p.as_path()).unwrap();
    let r = lines(&dump, None).into_iter().find_map(|c| c.1).unwrap();
    let bad = wikidata_dump::ResumePoint { bit: r.bit + 3, ..r };
    let opts = ReadOptions {
        resume: Some(bad),
        ..Default::default()
    };
    let err = dump.chunks(&opts).unwrap().find_map(|c| c.err()).unwrap();
    assert!(err.to_string().contains("resume point"), "{err}");
}
