# wikidata-export

Stream filtered, normalised entities out of Wikidata dumps on every core.

```rust,no_run
use wikidata_export::{entities, extract, Dump, EntityOptions, Event, ExtractOptions, Filter, Flow};

let dump = Dump::open("wikidata-20261012-all.json.bz2")?;

// Entities in the Netherlands, normalised
let mut opts = EntityOptions::default();
opts.scan.filter = Some(Filter::parse_claim("P17=Q55")?.compile(None)?.into());
opts.model.languages = Some(vec!["nl".into(), "en".into()]);
entities(&dump, &opts, &mut |e| {
    if let Event::Records(rs) = e {
        for r in rs {
            println!("{} {:?}", r.entity.id, r.entity.label);
        }
    }
    Flow::Continue
})?;

// The same entities' original lines, as a small dump
let mut x = ExtractOptions::default();
x.scan.filter = Some(Filter::parse_claim("P17=Q55")?.compile(None)?.into());
extract(&dump, &x, "nl.jsonl.zst", &mut |_| Flow::Continue)?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

- `entities`: matching entities as `Record`s (the `Entity`, and its JSON
  serialised on the workers), in dump order, with `limit` and progress.
- `extract` / `raw_lines`: the matching original lines, to `.jsonl.zst`,
  `.json.bz2`, `.json.gz`, … (compressed on every core).
- `build_class_index`: the P279 hierarchy in one pass.
- `file_info`: entities by type, highest ids, the largest entity, sizes,
  bzip2 blocks and streams.
- Every task stops when the sink returns `Flow::Stop`, and leaves no
  thread behind.
