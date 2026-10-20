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

Benchmarks use the first 50 MB of a real dump instead; see
[bench/README.md](../bench/README.md).
