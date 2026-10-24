# Fixtures

`sample.json` is a small Wikidata dump (41 entities) used by the Rust,
CLI and Node tests. It covers every datatype (items, properties, lexemes,
forms, senses and entity schemas as values; strings, external ids, URLs,
Commons files, texts, times at day, month, year and decade precision in
the Gregorian and Julian calendars, quantities with and without units and
bounds, coordinates on Earth and elsewhere, maths, musical notation,
geo-shapes, tabular data), ranks (preferred, normal, deprecated),
qualifiers, references, `somevalue` and `novalue`, empty term lists
written as `[]`, multi-byte UTF-8 (escaped as `\uXXXX` with surrogate
pairs, as in the dumps), a lexeme line that starts with `pageid`, and a
small class hierarchy (music festival ⊂ festival ⊂ event, and a
subclass of music festival).

It is written like Wikimedia's dumper writes (PHP `json_encode`: non-ASCII
as `\uXXXX`, `/` as `\/`, one entity per line with a trailing comma but
the last), by:

```sh
node scripts/make-sample-dump.mjs
```

The facts in it are a mix of real ones and made-up ones (ids from
`Q5000` and `Q99000001` are invented); it is test data, not a source.

The same script writes the other fixtures:

- `places.json` (17 entities): places and what is in them, for the place
  index and referenced labels: a venue in a village in a Dutch
  municipality (P131 only), a record label with its headquarters abroad
  (P159), a P131 cycle, a deprecated P131, and items referenced both
  before and after the entity that refers to them (ids from `Q9000` are
  invented).
- `wikidatawiki-redirect.sql` and `wikidatawiki-page.sql`: the two tables
  of Wikidata's SQL dumps the redirect index joins, written like mysqldump
  writes them (items merged into items of `sample.json` and one it lacks,
  a double redirect, a property redirect, a talk page).
- `incremental.xml`: a day's incremental dump
  (`pages-meta-hist-incr.xml`), written like MediaWiki's XML export: an item
  edited twice, a merge (redirect), a discussion page, a property, a
  lexeme and a deleted revision text.

Benchmarks use the first 50 MB of a real dump instead; see
[bench/README.md](../bench/README.md).
