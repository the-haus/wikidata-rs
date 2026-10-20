//! Compare the JSON parsers on real dump lines (one thread), checking that
//! they agree.
//!
//!   bzip2 -dc head.json.bz2 > head.json
//!   cargo run --release -p wikidata-model --example parse_bench -- head.json

use std::time::Instant;

use wikidata_model::{parse_entity_with, Backend, Compiled, ModelOptions, Pid};

fn main() {
    let path = std::env::args().nth(1).expect("usage: parse_bench DUMP.json");
    let data = std::fs::read(&path).unwrap();
    let lines: Vec<&[u8]> = data
        .split(|&b| b == b'\n')
        .map(|l| l.strip_suffix(b",").unwrap_or(l))
        .filter(|l| l.first() == Some(&b'{') && l.last() == Some(&b'}'))
        .collect();
    let bytes: usize = lines.iter().map(|l| l.len()).sum();
    println!("{} lines, {:.1} MB", lines.len(), bytes as f64 / 1e6);
    let cases = [
        (
            "everything",
            ModelOptions {
                references: true,
                deprecated: true,
                ..Default::default()
            },
        ),
        ("defaults (all languages, no references)", ModelOptions::default()),
        (
            "nl,en,mul + 6 properties",
            ModelOptions {
                languages: Some(vec!["nl".into(), "en".into(), "mul".into()]),
                sitelinks: Some(vec!["nlwiki".into(), "enwiki".into()]),
                properties: Some([31, 17, 625, 571, 856, 434].map(Pid).to_vec()),
                ..Default::default()
            },
        ),
        (
            "ids + derived only",
            ModelOptions {
                languages: Some(vec![]),
                sitelinks: Some(vec![]),
                properties: Some(vec![]),
                ..Default::default()
            },
        ),
    ];
    for (name, opts) in cases {
        let c = Compiled::new(opts);
        let mut backends = vec![("serde_json", Backend::SerdeJson)];
        #[cfg(feature = "sonic")]
        backends.push(("sonic-rs", Backend::Sonic));
        let mut results = Vec::new();
        for (bname, b) in &backends {
            let mut best = f64::MAX;
            let mut out = Vec::new();
            for _ in 0..3 {
                out.clear();
                let t = Instant::now();
                for l in &lines {
                    out.push(parse_entity_with(l, &c, *b).unwrap());
                }
                best = best.min(t.elapsed().as_secs_f64());
            }
            println!(
                "{name:44} {bname:10} {:7.0} MB/s {:8.0} entities/s",
                bytes as f64 / 1e6 / best,
                lines.len() as f64 / best
            );
            results.push(out);
        }
        if results.len() == 2 {
            for (a, b) in results[0].iter().zip(&results[1]) {
                if a != b {
                    let (ja, jb) = (a.to_json(), b.to_json());
                    let i = ja.bytes().zip(jb.bytes()).position(|(x, y)| x != y).unwrap_or(0);
                    let s = i.saturating_sub(120);
                    panic!(
                        "parsers disagree on {}:\n{}\n{}",
                        a.id,
                        &ja[s..(i + 80).min(ja.len())],
                        &jb[s..(i + 80).min(jb.len())]
                    );
                }
            }
        }
    }
}
