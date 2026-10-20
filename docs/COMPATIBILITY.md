# Compatibility

What of Wikidata's JSON dumps is read, and how it is normalised.

## Files

| Input | Read |
|---|---|
| `wikidata-YYYYMMDD-all.json.bz2` (one bzip2 stream, ~900 kB blocks) | yes, on every core |
| `latest-all.json.bz2`, `latest-lexemes.json.bz2`, other entity dumps | yes |
| multi-stream bzip2 (`pbzip2`, `lbzip2 -n`, this package's extracts) | yes, on every core |
| `.json.gz` (`latest-all.json.gz`), multi-member gzip | yes, one thread |
| `.json.zst`, multi-frame zstd | yes, one thread |
| plain `.json`, JSON lines (`.jsonl`, `.ndjson`) | yes |
| an `http(s)://` URL of any of these | yes, streamed and resumed on dropped connections |
| truncated files (partial downloads) | an error; or, with `allowTruncated` / `--allow-truncated`, read up to the last whole entity and flagged as `truncated` |
| randomised bzip2 blocks (bzip2 < 0.9.5) | yes, through libbzip2 |
| `.json` dumps with several entities on one line, or pretty-printed | no: one entity per line |
| the RDF dumps (`.ttl`, `.nt`) | no |

The array layout is the dumps' own: `[`, one entity per line followed by
a comma except the last, `]`. An array that does not end with `]` is
truncated. Empty lines are skipped.

## Entities

| Type | Read | Notes |
|---|---|---|
| item (`Q`) | all of it | |
| property (`P`) | all of it, plus `datatype` | |
| lexeme (`L`) | `lemmas`, `lexicalCategory`, `language`, `forms` (`representations`, `grammaticalFeatures`, `claims`), `senses` (`glosses`, `claims`), `claims` | lemmas and representations are not filtered by language (their codes are variants such as `en-gb`) |
| media info (`M`, Commons) | `labels`, `descriptions`, `statements` (read as `claims`) | |
| entity schema (`E`) | as a reference value | |

Kept from every entity: `id`, `type`, `labels`, `descriptions`,
`aliases`, `sitelinks` (`title`, `badges`, plus a page URL), `claims`,
`lastrevid`, `modified`. Not kept: `pageid`, `ns`, `title`.

Term lists written as `[]` (how PHP serialises an empty map) read as
empty maps.

## Statements

| Dump | Here |
|---|---|
| `mainsnak` | `value` (typed, below) |
| `rank` | `rank`; `deprecated` statements are dropped unless `deprecated: true`; `bestRank: true` keeps a property's `preferred` statements when there are any, else its `normal` ones |
| `qualifiers` | `qualifiers`: property → values (on by default) |
| `references` | `references`: a list of property → values (off by default) |
| `qualifiers-order`, `snaks-order`, statement `id`, reference `hash` | not kept |
| `snaktype` `somevalue` / `novalue` | `{"type": "someValue"}` / `{"type": "noValue"}` |
| a datatype this version does not know | its string value, or nothing for other shapes |

## Values

| Datatype | Value |
|---|---|
| `wikibase-item`, `wikibase-property`, `wikibase-lexeme`, `wikibase-form`, `wikibase-sense`, `entity-schema` | `{"type": "entity", "id": "Q5"}` (`P31`, `L7`, `L7-F1`, `L7-S1`, `E10`) |
| `string` | `{"type": "string", "value": …}` |
| `external-id` | `{"type": "externalId", "value": …}` |
| `url` | `{"type": "url", "value": …}` |
| `commonsMedia` | `{"type": "media", "file": "Foo.jpg", "url": "https://commons.wikimedia.org/wiki/File:Foo.jpg"}` |
| `monolingualtext` | `{"type": "monolingual", "text": …, "language": …}` |
| `time` | `{"type": "time", "time": "+1830-10-04T00:00:00Z", "iso": "1830-10-04", "precision": 11, "precisionName": "day", "year": 1830, "month": 10, "day": 4, "calendar": "gregorian"}` |
| `quantity` | `{"type": "quantity", "amount": 1.96, "unit": "Q11573", "lower": 1.95, "upper": 1.97}` (`unit: null` for a pure number) |
| `globe-coordinate` | `{"type": "coordinate", "lat": …, "lon": …, "precision": …, "globe": "Q2"}` |
| `math` | `{"type": "math", "value": …}` |
| `musical-notation` | `{"type": "musicalNotation", "value": …}` |
| `geo-shape`, `tabular-data` | `{"type": "geoShape" \| "tabularData", "value": "Data:….map", "url": …}` |

**Times.** `iso` is the date at its precision: `YYYY-MM-DD` at day
precision or finer, `YYYY-MM` at month precision, the year (`1830`,
`-0490`) at year precision and coarser (decade, century, … where `year`
is the value as recorded, e.g. `1990` for the 1990s). Month and day are
given only when the precision covers them, so `+1830-00-00` at year
precision has no month. Precision names: `billionYears` (0) …
`millennium` (6), `century`, `decade`, `year` (9), `month`, `day` (11),
`hour`, `minute`, `second`. Calendars: `gregorian` (Q1985727), `julian`
(Q1985786), or the item id of another. The time zone and the `before` /
`after` bounds are not kept.

**Quantities.** Amounts and bounds are numbers (doubles: exact for
integers up to 2^53). Units are item ids.

**Coordinates.** Doubles parsed exactly. `precision` is omitted when
recorded as `null`; `altitude` (always null) is not kept.

## Sitelinks

`url` is built for Wikipedia (`nlwiki`), Wikivoyage, Wikiquote,
Wikisource, Wikinews, Wikibooks, Wikiversity, Wiktionary
(`<language><project>`; `_` in the language becomes `-`: `zh_yuewiki` →
`zh-yue.wikipedia.org`), and for Commons, Wikispecies, Meta, MediaWiki,
Wikidata, multilingual Wikisource, Incubator, Outreach, Wikimania and
Wikifunctions. Titles are encoded as MediaWiki links are (spaces become
underscores). Unknown sites get `url: null`.

## Languages

`languages` keeps labels, descriptions, aliases and sense glosses in the
listed languages only, in that order, and is also the fallback chain for
`label` and `description` (the first language that has one). Without
`languages`, every language is kept and the chain is `en`, `mul`.

## Derived fields

From the truthy statements (preferred if any, else normal) of:

| Field | Property |
|---|---|
| `instanceOf` | P31 (item ids) |
| `subclassOf` | P279 |
| `country` | P17 |
| `coordinates` | P625, the first |
| `inception` | P571, year |
| `dissolved` | P576, year |
| `image` | P18, the file name |
| `website` | P856, the first |
| `externalIds` | the table in [`external.rs`](../crates/wikidata-model/src/external.rs): MusicBrainz (artist, work, release group, release, recording, label, area, place, event, series), Discogs (artist, master, label), Spotify (artist, album, track), Apple Music, Deezer, Last.fm, SoundCloud, Bandcamp, Songkick, Genius, AllMusic, OpenStreetMap (relation, way, node), GeoNames, ISNI, VIAF, GND, Library of Congress, Freebase, IMDb, YouTube channel, Instagram, X, Facebook — the first value of each, with a URL formatter |

## Filters

Claim filters (`hasProperty`, `claim`, `instanceOf`) look at the
statements kept by default: all but the deprecated ones. `instanceOf`
with `subclasses` follows the truthy P279 edges, like SPARQL's
`wdt:P31/wdt:P279*`. Claim values match entity ids, or the exact string
of a string-like value (string, external id, URL, file name, text).
