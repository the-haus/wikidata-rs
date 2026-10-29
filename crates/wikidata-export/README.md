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

- `entities`: matching entities as `Record`s (the `Entity`, its JSON
  serialised on the workers, and the named filters it matched), in dump
  order, with `limit` and progress. `languages_by` / `sitelinks_by`
  (`ByClaim`) choose each entity's languages and sitelinks by its claims
  (`P17` → `nl` for a Dutch item, `de` for a German one); a sitelinks
  rule's `all_for` keeps more sites (every Wikipedia) for the items of
  some classes.
- `extract` / `raw_lines`: the matching original lines, to `.jsonl.zst`,
  `.json.bz2`, `.json.gz`, … (compressed on every core).
- `build_class_index` / `build_place_index`: the P279 hierarchy, or every
  item's countries, in one pass (both in the same pass if asked); or, with
  `ScanOptions::{class_index, place_index}`, as by-products of any other
  task.
- `build_redirect_index`: the SQL dumps' `redirect` and `page` tables
  joined into item redirects, streamed and parsed on all cores.
- With `ids` in the filter, a complete pass reports the requested ids the
  dump does not hold (`ScanStats::missing_ids`), and with a redirect index
  sorts them (`ScanStats::misses`); entities whose revision is known and
  unchanged are skipped (`ScanStats::unchanged`).
- `ScanOptions::checkpoint_every` sends `Event::Checkpoint`s; a
  `Checkpoint` (serialisable) resumes a scan (`ScanOptions::resume`) with
  exactly the records after it.
- `EntityOptions::referenced_labels`: the labels of the items the matches
  refer to, in two stages (`labels`).
- `live_changes`: Wikidata's edits from EventStreams, filtered by ids,
  resumable.
- `file_info`: entities by type, highest ids, the largest entity, sizes,
  bzip2 blocks and streams.
- Every task stops when the sink returns `Flow::Stop`, and leaves no
  thread behind.
