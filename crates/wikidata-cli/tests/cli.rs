//! Run the `wikidata` binary end to end.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_wikidata"))
}

fn sample() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/sample.json")
}

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("wikidata-cli-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn run(args: &[&str]) -> (bool, String, String) {
    let o = bin().args(args).output().unwrap();
    (
        o.status.success(),
        String::from_utf8(o.stdout).unwrap(),
        String::from_utf8(o.stderr).unwrap(),
    )
}

fn ids(jsonl: &str) -> Vec<String> {
    jsonl
        .lines()
        .map(|l| {
            serde_json::from_str::<serde_json::Value>(l).unwrap()["id"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect()
}

#[test]
fn info() {
    let s = sample();
    let (ok, out, _) = run(&["info", s.to_str().unwrap(), "--json"]);
    assert!(ok);
    let j: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(j["entities"], 41);
    assert_eq!(j["types"]["lexemes"], 2);
    let (ok, out, _) = run(&["info", s.to_str().unwrap(), "-q"]);
    assert!(ok);
    assert!(out.contains("Entities:"), "{out}");
    assert!(out.contains("items"));
}

#[test]
fn cat_filters_and_shapes() {
    let s = sample();
    let s = s.to_str().unwrap();
    let (ok, out, _) = run(&["cat", s, "-q", "--claim", "P17=Q55", "--languages", "nl,en"]);
    assert!(ok);
    let got = ids(&out);
    assert_eq!(got.len(), 9);
    assert_eq!(&got[..3], ["Q55", "Q1072", "Q1074"]);
    let first: serde_json::Value = serde_json::from_str(out.lines().next().unwrap()).unwrap();
    assert_eq!(first["label"]["text"], "Nederland");
    assert_eq!(first["claims"]["P17"][0]["value"]["id"], "Q55");

    let (ok, out, _) = run(&[
        "cat",
        s,
        "-q",
        "--id",
        "Q42,L7",
        "--properties",
        "none",
        "--sites",
        "none",
    ]);
    assert!(ok);
    assert_eq!(ids(&out), ["Q42", "L7"]);
    let q42: serde_json::Value = serde_json::from_str(out.lines().next().unwrap()).unwrap();
    assert_eq!(q42["claims"], serde_json::json!({}));
    assert_eq!(q42["sitelinks"], serde_json::json!({}));
    assert_eq!(q42["externalIds"]["viaf"], "113230702");

    // Counts beside the pruned sitelinks, and every Wikipedia for humans.
    let (ok, out, _) = run(&[
        "cat",
        s,
        "-q",
        "--id",
        "Q31,Q42",
        "--sites",
        "enwiki",
        "--sites-for",
        "Q5",
        "--sites-for-group",
        "wikipedia",
        "--sitelink-counts",
    ]);
    assert!(ok);
    let rows: Vec<serde_json::Value> = out.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    let keys = |v: &serde_json::Value| v["sitelinks"].as_object().unwrap().keys().cloned().collect::<Vec<_>>();
    assert_eq!(keys(&rows[0]), ["enwiki"]);
    assert_eq!(
        (rows[0]["sitelinkCount"].as_u64(), rows[0]["wikipediaCount"].as_u64()),
        (Some(4), Some(3))
    );
    assert_eq!(keys(&rows[1]), ["enwiki", "nlwiki"]);
    let (ok, _, err) = run(&["cat", s, "-q", "--sites-for", "Q5", "--sites-for-group", "wiki"]);
    assert!(!ok);
    assert!(err.contains("unknown site group"), "{err}");

    let (ok, out, _) = run(&["cat", s, "-q", "--type", "property", "--raw"]);
    assert!(ok);
    assert!(out.lines().all(|l| l.starts_with("{\"type\":\"property\"")));
    assert_eq!(out.lines().count(), 2);

    let (ok, out, _) = run(&[
        "cat",
        s,
        "-q",
        "--has",
        "P434",
        "--sitelink",
        "nlwiki",
        "--any",
        "--limit",
        "3",
    ]);
    assert!(ok);
    assert_eq!(ids(&out), ["Q31", "Q55", "Q42"]);

    let (ok, out, _) = run(&["cat", s, "-q", "--filter", r#"{"instanceOf":["Q6256"]}"#]);
    assert!(ok);
    assert_eq!(ids(&out), ["Q31", "Q55"]);

    let d = dir("ids");
    let f = d.join("ids.txt");
    std::fs::write(&f, "Q31\nQ404\n P31 ").unwrap();
    let (ok, out, _) = run(&["cat", s, "-q", "--ids-file", f.to_str().unwrap()]);
    assert!(ok);
    assert_eq!(ids(&out), ["Q31", "P31"]);
}

#[test]
fn extract_and_classes() {
    let s = sample();
    let s = s.to_str().unwrap();
    let d = dir("extract");
    let out = d.join("nl.jsonl.zst");
    let o = out.to_str().unwrap();
    let (ok, stdout, err) = run(&["extract", s, "-q", "--claim", "P17=Q55", "-o", o]);
    assert!(ok, "{err}");
    let stats: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(stats["matched"], 9);
    assert_eq!(stats["outputCompression"], "zstd");
    let (ok, _, err) = run(&["extract", s, "-q", "-o", o]);
    assert!(!ok && err.contains("exists"));
    let (ok, back, _) = run(&["cat", o, "-q", "--raw"]);
    assert!(ok);
    assert_eq!(back.lines().count(), 9);

    let idx = d.join("classes.wdc");
    let (ok, _, err) = run(&["classes", s, "-q", "-o", idx.to_str().unwrap()]);
    assert!(ok, "{err}");
    let (ok, out, _) = run(&[
        "classes",
        "--index",
        idx.to_str().unwrap(),
        "--subclasses-of",
        "Q132241",
    ]);
    assert!(ok);
    assert_eq!(out.lines().collect::<Vec<_>>(), ["Q132241", "Q868557", "Q99000001"]);
    let (ok, out, _) = run(&[
        "cat",
        s,
        "-q",
        "--instance-of",
        "Q868557",
        "--subclasses",
        "--classes",
        idx.to_str().unwrap(),
    ]);
    assert!(ok);
    assert_eq!(ids(&out), ["Q1072", "Q1073"]);
    let (ok, _, err) = run(&["cat", s, "-q", "--instance-of", "Q868557", "--subclasses"]);
    assert!(!ok && err.contains("class index"), "{err}");

    // The same index, built during other passes.
    let alone = std::fs::read(&idx).unwrap();
    let during = d.join("during.wdc");
    let x = d.join("lexemes.jsonl");
    let (ok, stdout, err) = run(&[
        "extract",
        s,
        "-q",
        "--type",
        "lexeme",
        "-o",
        x.to_str().unwrap(),
        "--build-classes",
        during.to_str().unwrap(),
    ]);
    assert!(ok, "{err}");
    let stats: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(stats["classIndex"]["edges"], 3);
    assert_eq!(std::fs::read(&during).unwrap(), alone);
    let info = d.join("info.wdc");
    let (ok, out, _) = run(&["info", s, "-q", "--build-classes", info.to_str().unwrap()]);
    assert!(ok);
    assert!(out.contains("Class index:"), "{out}");
    assert_eq!(std::fs::read(&info).unwrap(), alone);
    let cat = d.join("cat.wdc");
    let (ok, _, _) = run(&["cat", s, "-q", "--limit", "1", "--build-classes", cat.to_str().unwrap()]);
    assert!(ok);
    assert!(!cat.exists(), "a stopped pass saves no index");
}

#[test]
fn errors_and_pipes() {
    let d = dir("errors");
    let data = std::fs::read(sample()).unwrap();
    let cut = d.join("cut.json");
    let end = data[..data.len() / 2].iter().rposition(|&b| b == b'\n').unwrap() + 1;
    std::fs::write(&cut, &data[..end]).unwrap();
    let (ok, _, err) = run(&["info", cut.to_str().unwrap(), "-q"]);
    assert!(!ok && err.contains("truncated"), "{err}");
    let (ok, out, _) = run(&["info", cut.to_str().unwrap(), "-q", "--allow-truncated"]);
    assert!(ok && out.contains("TRUNCATED"));
    let (ok, _, err) = run(&["info", "/nonexistent.json.bz2"]);
    assert!(!ok && err.contains("nonexistent"));
    let (ok, _, err) = run(&["cat", sample().to_str().unwrap(), "--claim", "Q5=Q1"]);
    assert!(!ok && err.contains("property"), "{err}");

    // A closed pipe ends the command quietly.
    let mut big = b"[\n".to_vec();
    let lines: Vec<&[u8]> = data
        .split(|&b| b == b'\n')
        .filter(|l| l.first() == Some(&b'{'))
        .map(|l| l.strip_suffix(b",").unwrap_or(l))
        .collect();
    for i in 0..4000 {
        if i > 0 {
            big.extend(b",\n");
        }
        big.extend(lines[i % lines.len()]);
    }
    big.extend(b"\n]\n");
    let p = d.join("big.json");
    std::fs::File::create(&p).unwrap().write_all(&big).unwrap();
    let mut child = bin()
        .args(["cat", p.to_str().unwrap(), "-q"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut first = [0u8; 100];
    std::io::Read::read_exact(&mut stdout, &mut first).unwrap();
    drop(stdout);
    let o = child.wait_with_output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
}

fn fixture(name: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
        .to_str()
        .unwrap()
        .to_string()
}

#[test]
fn places_and_in_country() {
    let d = dir("places");
    let index = d.join("places.wdp");
    let classes = d.join("classes.wdc");
    let p = fixture("places.json");
    let (ok, _, err) = run(&[
        "places",
        &p,
        "-o",
        index.to_str().unwrap(),
        "--build-classes",
        classes.to_str().unwrap(),
    ]);
    assert!(ok, "{err}");
    assert!(classes.exists());
    let (ok, out, _) = run(&[
        "places",
        "--index",
        index.to_str().unwrap(),
        "--country-of",
        "Q9003,Q9004",
    ]);
    assert!(ok);
    assert_eq!(out, "Q9003\tQ55\nQ9004\tQ55,Q145\n");
    let (ok, out, _) = run(&[
        "cat",
        &p,
        "--in-country",
        "Q145",
        "--places",
        index.to_str().unwrap(),
        "-q",
    ]);
    assert!(ok);
    assert_eq!(ids(&out), ["Q145", "Q9004", "Q9005"]);
    let (ok, _, err) = run(&["cat", &p, "--in-country", "Q145", "-q"]);
    assert!(!ok && err.contains("place index"), "{err}");
}

#[test]
fn redirects_revisions_and_misses() {
    let d = dir("redirects");
    let index = d.join("redirects.wdr");
    let (ok, _, err) = run(&[
        "redirects",
        "--redirect-sql",
        &fixture("wikidatawiki-redirect.sql"),
        "--page-sql",
        &fixture("wikidatawiki-page.sql"),
        "-o",
        index.to_str().unwrap(),
    ]);
    assert!(ok, "{err}");
    let (ok, out, _) = run(&[
        "redirects",
        "--index",
        index.to_str().unwrap(),
        "--resolve",
        "Q1,Q9,Q42",
    ]);
    assert!(ok);
    assert_eq!(out, "Q1\tQ42\nQ9\tQ42\nQ42\t\n");
    // Known revisions: Q31 unchanged, Q55 changed; Q1 merged, Q3 deleted.
    let revs = d.join("known.txt");
    std::fs::write(&revs, "Q31 2000000001\nQ55 1\nQ1 1\nQ3 1\n").unwrap();
    let s = sample();
    let o = bin()
        .args([
            "cat",
            s.to_str().unwrap(),
            "--revisions-file",
            revs.to_str().unwrap(),
            "--redirects",
            index.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(o.status.success());
    assert_eq!(ids(&String::from_utf8(o.stdout).unwrap()), ["Q55", "Q42"]);
    let stats: serde_json::Value =
        serde_json::from_str(String::from_utf8(o.stderr).unwrap().lines().last().unwrap()).unwrap();
    assert_eq!(stats["unchanged"], 1);
    assert_eq!(stats["merged"][0]["to"], "Q42");
    assert_eq!(stats["deleted"], serde_json::json!(["Q3"]));
}

#[test]
fn checkpoints_resume_and_referenced_labels() {
    let d = dir("checkpoint");
    // A bzip2 dump of many blocks.
    let lines: Vec<String> = std::fs::read_to_string(sample())
        .unwrap()
        .lines()
        .filter(|l| l.starts_with('{'))
        .map(|l| l.trim_end_matches(',').to_string())
        .collect();
    let mut text = String::from("[\n");
    for i in 0..3000 {
        text.push_str(&lines[i % lines.len()]);
        text.push_str(if i + 1 < 3000 { ",\n" } else { "\n" });
    }
    text.push_str("]\n");
    let mut e = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::new(1));
    e.write_all(text.as_bytes()).unwrap();
    let dump = d.join("big.json.bz2");
    std::fs::write(&dump, e.finish().unwrap()).unwrap();
    let dump = dump.to_str().unwrap();
    let (ok, all, _) = run(&["cat", dump, "--claim", "P17=Q55", "--properties", "none", "-q"]);
    assert!(ok);
    let cp = d.join("pass.ckpt");
    let (ok, part, err) = run(&[
        "cat",
        dump,
        "--claim",
        "P17=Q55",
        "--properties",
        "none",
        "-q",
        "--checkpoint",
        cp.to_str().unwrap(),
        "--checkpoint-every",
        "3",
        "--limit",
        "200",
    ]);
    assert!(ok, "{err}");
    assert!(cp.exists());
    let (ok, rest, err) = run(&[
        "cat",
        dump,
        "--claim",
        "P17=Q55",
        "--properties",
        "none",
        "-q",
        "--resume",
        cp.to_str().unwrap(),
    ]);
    assert!(ok, "{err}");
    // The checkpoint lies within the first run's output: what was written
    // before it, then the resumed rest, is the whole.
    let all: Vec<&str> = all.lines().collect();
    let rest: Vec<&str> = rest.lines().collect();
    let done = all.len() - rest.len();
    assert!(done > 0 && done <= part.lines().count());
    assert_eq!(all[done..], rest[..]);
    // Referenced labels.
    let out = d.join("labels.json");
    let (ok, _, err) = run(&[
        "cat",
        &fixture("places.json"),
        "--id",
        "Q9009",
        "--properties",
        "P131",
        "--referenced-labels",
        "nl",
        "--labels-output",
        out.to_str().unwrap(),
        "-q",
    ]);
    assert!(ok, "{err}");
    assert_eq!(
        std::fs::read_to_string(out).unwrap(),
        r#"{"Q9002":{"nl":"Schaesberg"}}"#
    );
}
