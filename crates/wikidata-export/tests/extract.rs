mod common;

use common::*;
use wikidata_export::{extract, Dump, EntityOptions, ExtractOptions, Filter, Flow, OutputFormat};

#[test]
fn extract_round_trips() {
    let d = tmp();
    let src = write(&d, "src.json.bz2", &bz2(&sample(), 9));
    let dump = Dump::open(src.as_path()).unwrap();
    let filter = Filter::parse_json(r#"{"any":[{"claim":"P17=Q55"},{"type":"lexeme"}]}"#)
        .unwrap()
        .compile(None)
        .unwrap();
    let lines = sample_lines();
    for (name, format, compression) in [
        ("subset.jsonl.zst", OutputFormat::JsonLines, "zstd"),
        ("subset.json.bz2", OutputFormat::JsonArray, "bzip2"),
        ("subset.json.gz", OutputFormat::JsonArray, "gzip"),
        ("subset.jsonl.gz", OutputFormat::JsonLines, "gzip"),
        ("subset.json", OutputFormat::JsonArray, "none"),
    ] {
        let out = d.join(name);
        let mut opts = ExtractOptions::default();
        opts.scan.filter = Some(filter.clone_arc());
        let stats = extract(&dump, &opts, &out, &mut |_| Flow::Continue).unwrap();
        assert_eq!(stats.scan.matched, 11, "{name}");
        assert_eq!(stats.format, format);
        assert_eq!(stats.output_compression, compression);
        assert_eq!(stats.bytes_written, std::fs::metadata(&out).unwrap().len());

        // It reads back: the same original lines, in order.
        let back = Dump::open(out.as_path()).unwrap();
        let (rs, rstats) = collect(&back, &EntityOptions::default());
        assert_eq!(rstats.entities, 11, "{name}");
        assert!(!rstats.truncated);
        let want: Vec<&Vec<u8>> = lines
            .iter()
            .filter(|l| ids(&rs).iter().any(|id| wikidata_model::peek(l).unwrap().id == id))
            .collect();
        assert_eq!(want.len(), 11);
        if compression == "none" && format == OutputFormat::JsonArray {
            // Byte for byte the dump's own layout.
            let text = std::fs::read(&out).unwrap();
            let mut expect = b"[\n".to_vec();
            expect.extend(want.iter().map(|l| l.as_slice()).collect::<Vec<_>>().join(&b",\n"[..]));
            expect.extend(b"\n]\n");
            assert!(text == expect);
        }
    }
    // An empty extract is still a valid dump.
    let out = d.join("none.json.zst");
    let mut opts = ExtractOptions::default();
    opts.scan.filter = Some(
        Filter::parse_json(r#"{"ids":"Q404"}"#)
            .unwrap()
            .compile(None)
            .unwrap()
            .into(),
    );
    let stats = extract(&dump, &opts, &out, &mut |_| Flow::Continue).unwrap();
    assert_eq!(stats.scan.matched, 0);
    let (rs, _) = collect(&Dump::open(out.as_path()).unwrap(), &EntityOptions::default());
    assert!(rs.is_empty());
}

trait CloneArc {
    fn clone_arc(&self) -> std::sync::Arc<wikidata_export::CompiledFilter>;
}

impl CloneArc for wikidata_export::CompiledFilter {
    fn clone_arc(&self) -> std::sync::Arc<wikidata_export::CompiledFilter> {
        // Compile again: a compiled filter is not Clone.
        std::sync::Arc::new(
            Filter::parse_json(r#"{"any":[{"claim":"P17=Q55"},{"type":"lexeme"}]}"#)
                .unwrap()
                .compile(None)
                .unwrap(),
        )
    }
}

#[test]
fn bzip2_extract_is_multi_stream_and_parallel() {
    // Big enough for several 900 kB streams.
    let d = tmp();
    let lines = sample_lines();
    let mut data = b"[\n".to_vec();
    let mut n = 0;
    while data.len() < 3_000_000 {
        for l in &lines {
            if n > 0 {
                data.extend(b",\n");
            }
            data.extend(l);
            n += 1;
        }
    }
    data.extend(b"\n]\n");
    let src = write(&d, "big.json", &data);
    let dump = Dump::open(src.as_path()).unwrap();
    let out = d.join("big.json.bz2");
    let stats = extract(&dump, &ExtractOptions::default(), &out, &mut |_| Flow::Continue).unwrap();
    assert_eq!(stats.scan.matched, n);
    let back = Dump::open(out.as_path()).unwrap();
    let info = wikidata_export::file_info(&back, &Default::default(), &mut |_| Flow::Continue).unwrap();
    assert_eq!(info.scan.entities, n);
    assert!(info.scan.streams >= 3, "{}", info.scan.streams);
    // The system's bzip2 reads it too, when installed.
    if let Ok(o) = std::process::Command::new("bzip2").arg("-dc").arg(&out).output() {
        assert!(o.status.success());
        assert!(o.stdout == data);
    }
}

#[test]
fn crlf_dump_extracts_like_lf() {
    // A dump with CRLF line endings reads, and extracts, the same as with LF.
    let d = tmp();
    let lf = sample();
    assert!(!lf.contains(&b'\r'), "fixtures/sample.json must be checked out with LF");
    let crlf: Vec<u8> = lf
        .iter()
        .flat_map(|&b| if b == b'\n' { vec![b'\r', b'\n'] } else { vec![b] })
        .collect();
    let mut outs = Vec::new();
    for (name, data) in [("lf.json", &lf), ("crlf.json", &crlf)] {
        let src = write(&d, name, data);
        let out = d.join(format!("out-{name}"));
        let stats = extract(
            &Dump::open(src.as_path()).unwrap(),
            &ExtractOptions::default(),
            &out,
            &mut |_| Flow::Continue,
        )
        .unwrap();
        assert_eq!(stats.scan.matched, 41, "{name}");
        outs.push(std::fs::read(&out).unwrap());
    }
    assert!(!outs[1].contains(&b'\r'));
    assert!(outs[0] == outs[1]);
}
