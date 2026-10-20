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
