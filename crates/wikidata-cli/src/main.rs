//! `wikidata`: filter, extract and inspect Wikidata JSON dumps
//! (`wikidata-*-all.json.bz2`, `.gz`, `.zst`, plain, local or over HTTP).

use std::io::{BufWriter, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use wikidata_dump::download::{download, DownloadOptions};
use wikidata_export::{
    build_class_index, build_place_index, build_redirect_index, entities, extract, file_info, live_changes, Checkpoint,
    ClassIndex, Dump, EntityOptions, Event, ExportError, ExtractOptions, Filter, Flow, HttpOptions, Indexes, Input,
    LabelOptions, LiveOptions, ModelOptions, PlaceIndex, Progress, RedirectIndex, RedirectOptions, ScanOptions,
    WriteOptions,
};
use wikidata_model::Pid;

#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[derive(Parser)]
#[command(
    name = "wikidata",
    version,
    about = "Filter, extract and inspect Wikidata JSON dumps, fast"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// What a dump holds: entities by type, sizes, bzip2 blocks, timing.
    Info {
        #[command(flatten)]
        input: InputArgs,
        /// Print JSON.
        #[arg(long)]
        json: bool,
    },
    /// Stream matching entities as JSON lines: normalised (typed claims,
    /// derived fields) or, with --raw, the dump's own lines.
    Cat(Box<CatArgs>),
    /// Write matching entities to a smaller dump (.jsonl.zst, .json.bz2,
    /// .json.gz, …) that reads back like the original.
    Extract(Box<ExtractArgs>),
    /// Build the class index (P279 subclass-of hierarchy) that
    /// --instance-of --subclasses needs; or query one.
    Classes(Box<ClassesArgs>),
    /// Build the place index (each item's country, through P131 and
    /// P159) that --in-country needs; or query one.
    Places(Box<PlacesArgs>),
    /// Build the redirect index (merged items) from the SQL dumps
    /// (`…-redirect.sql.gz` and `…-page.sql.gz`); or query one.
    Redirects(Box<RedirectsArgs>),
    /// Download a dump: the newest complete one, from several mirrors at
    /// once, resumable, SHA-1 verified, renamed into place when done.
    Download(Box<DownloadArgs>),
    /// Print the entities edited on Wikidata as they are edited (one JSON
    /// line per change, from Wikimedia's EventStreams), resumable.
    Live(Box<LiveArgs>),
}

#[derive(Args)]
struct InputArgs {
    /// The dump: a path or an http(s):// URL.
    input: String,
    /// Another URL of the same file (repeatable): read in ranges from all
    /// of them at once.
    #[arg(long = "mirror", value_name = "URL")]
    mirrors: Vec<String>,
    /// HTTP connections per server when reading in ranges (default 2
    /// with mirrors).
    #[arg(long)]
    connections: Option<usize>,
    /// Number of threads (0: all cores).
    #[arg(long, default_value_t = 0)]
    threads: usize,
    /// Stop after this many matching entities.
    #[arg(long)]
    limit: Option<u64>,
    /// Read a truncated dump (a partial download) up to its last whole
    /// entity instead of failing.
    #[arg(long)]
    allow_truncated: bool,
    /// Also build the class index during this pass (as `wikidata classes`
    /// would) and save it to this file, when the pass is not stopped.
    #[arg(long, value_name = "FILE")]
    build_classes: Option<PathBuf>,
    /// Also build the place index during this pass (as `wikidata places`
    /// would) and save it to this file, when the pass is not stopped.
    #[arg(long, value_name = "FILE")]
    build_places: Option<PathBuf>,
    /// `cat`: save a checkpoint to this file as the pass goes (bzip2 and
    /// plain JSON), to continue an interrupted pass with --resume.
    #[arg(long, value_name = "FILE")]
    checkpoint: Option<PathBuf>,
    /// With --checkpoint: blocks (chunks) between checkpoints.
    #[arg(long, value_name = "N", default_value_t = 1000)]
    checkpoint_every: u64,
    /// `cat`: continue the pass a checkpoint file was saved by (same
    /// input, same filter); the output is what came after it.
    #[arg(long, value_name = "FILE")]
    resume: Option<PathBuf>,
    /// No progress on stderr.
    #[arg(short, long)]
    quiet: bool,
}

#[derive(Args, Default)]
struct FilterArgs {
    /// Only these ids (comma-separated, repeatable): Q42,P31,L7.
    #[arg(long = "id", value_delimiter = ',')]
    ids: Vec<String>,
    /// Only the ids listed in this file (one per line, or separated by
    /// commas or spaces).
    #[arg(long)]
    ids_file: Option<PathBuf>,
    /// Only these types: item, property, lexeme (comma-separated).
    #[arg(long = "type", value_delimiter = ',')]
    types: Vec<String>,
    /// Has a statement on any of these properties (comma-separated).
    #[arg(long = "has", value_delimiter = ',')]
    has: Vec<String>,
    /// A claim: P17=Q55 (or P17=Q55,Q31; or P17 for any value). Repeatable.
    #[arg(long = "claim")]
    claims: Vec<String>,
    /// Instance of (P31) one of these classes (comma-separated).
    #[arg(long, value_delimiter = ',')]
    instance_of: Vec<String>,
    /// With --instance-of: or of a subclass (needs --classes).
    #[arg(long)]
    subclasses: bool,
    /// A class index file (from `wikidata classes`).
    #[arg(long)]
    classes: Option<PathBuf>,
    /// Has a sitelink to any of these sites (comma-separated): nlwiki.
    #[arg(long = "sitelink", value_delimiter = ',')]
    sitelinks: Vec<String>,
    /// In one of these countries (comma-separated): by P17, the P131
    /// chain or P159 (needs --places).
    #[arg(long, value_delimiter = ',')]
    in_country: Vec<String>,
    /// A place index file (from `wikidata places`).
    #[arg(long)]
    places: Option<PathBuf>,
    /// Ids with their known revision, one `Q42 2000000001` per line:
    /// unchanged ones are skipped (but count as found).
    #[arg(long, value_name = "FILE")]
    revisions_file: Option<PathBuf>,
    /// A redirect index (from `wikidata redirects`): ids also match the
    /// items they were merged into, and missing ids are sorted into
    /// merged, deleted and newer.
    #[arg(long, value_name = "FILE")]
    redirects: Option<PathBuf>,
    /// Match any of the filters above instead of all of them.
    #[arg(long)]
    any: bool,
    /// A filter as JSON, or a file holding one (combined with the flags).
    #[arg(long)]
    filter: Option<String>,
}

#[derive(Args)]
struct ModelArgs {
    /// Languages to keep, in fallback order for label/description:
    /// nl,en,mul (default: all, fallback en,mul).
    #[arg(long, value_delimiter = ',')]
    languages: Vec<String>,
    /// Sitelinks to keep: nlwiki,enwiki (default: all; "none" for none).
    #[arg(long, value_delimiter = ',')]
    sites: Vec<String>,
    /// Properties to keep claims for: P31,P17 (default: all; "none" for
    /// none; derived fields are computed anyway).
    #[arg(long, value_delimiter = ',')]
    properties: Vec<String>,
    /// Keep references.
    #[arg(long)]
    references: bool,
    /// Drop qualifiers.
    #[arg(long)]
    no_qualifiers: bool,
    /// Keep only the qualifiers of these properties: P585,P580,P582
    /// (default: all).
    #[arg(long, value_delimiter = ',', conflicts_with = "no_qualifiers")]
    qualifiers: Vec<String>,
    /// Keep deprecated statements.
    #[arg(long)]
    deprecated: bool,
    /// Keep only the best-ranked statements of each property.
    #[arg(long)]
    best_rank: bool,
    /// Drop aliases.
    #[arg(long)]
    no_aliases: bool,
    /// Drop descriptions.
    #[arg(long)]
    no_descriptions: bool,
    /// No derived fields.
    #[arg(long)]
    no_derived: bool,
}

#[derive(Args)]
struct CatArgs {
    #[command(flatten)]
    input: InputArgs,
    #[command(flatten)]
    filter: FilterArgs,
    #[command(flatten)]
    model: ModelArgs,
    /// Output file (default: stdout).
    #[arg(short, long)]
    output: Option<PathBuf>,
    /// The dump's own lines instead of normalised entities.
    #[arg(long)]
    raw: bool,
    /// Also collect the labels, in these languages (comma-separated), of
    /// the items the matching entities refer to (written to
    /// --labels-output).
    #[arg(long, value_delimiter = ',', requires = "labels_output")]
    referenced_labels: Vec<String>,
    /// Where --referenced-labels writes `{"Q5":{"en":"human"},…}`.
    #[arg(long, value_name = "FILE")]
    labels_output: Option<PathBuf>,
}

#[derive(Args)]
struct PlacesArgs {
    /// The dump to build from (a path or URL); omit with --index.
    input: Option<String>,
    /// Write the index here.
    #[arg(short, long)]
    output: Option<PathBuf>,
    /// Load this index instead of building one.
    #[arg(long)]
    index: Option<PathBuf>,
    /// Print the countries of these items (comma-separated).
    #[arg(long, value_delimiter = ',')]
    country_of: Vec<String>,
    /// Also build the class index in the same pass.
    #[arg(long, value_name = "FILE")]
    build_classes: Option<PathBuf>,
    /// Number of threads (0: all cores).
    #[arg(long, default_value_t = 0)]
    threads: usize,
    /// Read a truncated dump up to its last whole entity.
    #[arg(long)]
    allow_truncated: bool,
    /// No progress on stderr.
    #[arg(short, long)]
    quiet: bool,
}

#[derive(Args)]
struct RedirectsArgs {
    /// The `redirect` table's SQL dump (`wikidatawiki-<date>-redirect.sql.gz`, a path or URL).
    #[arg(long, value_name = "FILE|URL", requires = "page_sql")]
    redirect_sql: Option<String>,
    /// The `page` table's SQL dump (`wikidatawiki-<date>-page.sql.gz`).
    #[arg(long, value_name = "FILE|URL")]
    page_sql: Option<String>,
    /// Write the index here.
    #[arg(short, long)]
    output: Option<PathBuf>,
    /// Load this index instead of building one.
    #[arg(long)]
    index: Option<PathBuf>,
    /// Print where these items redirect to (comma-separated).
    #[arg(long, value_delimiter = ',')]
    resolve: Vec<String>,
    /// Print every redirect as `{"from":…,"to":…}` lines.
    #[arg(long)]
    list: bool,
    /// Number of threads (0: all cores).
    #[arg(long, default_value_t = 0)]
    threads: usize,
    /// No progress on stderr.
    #[arg(short, long)]
    quiet: bool,
}

#[derive(Args)]
struct DownloadArgs {
    /// The dump's date (default: the newest complete one).
    #[arg(long)]
    date: Option<String>,
    /// Where to put it.
    #[arg(long, default_value = ".")]
    dir: PathBuf,
    /// Which file of the dump.
    #[arg(long, default_value = "all.json.bz2")]
    file: String,
    /// SQL tables of the database dump instead (comma-separated):
    /// redirect,page for `wikidata redirects`.
    #[arg(long, value_delimiter = ',')]
    tables: Vec<String>,
    /// Only print the newest complete dump (its date and files) as JSON.
    #[arg(long)]
    latest: bool,
    /// The dumps' directory (default: the entity dumps, or with --tables
    /// the database dumps, on dumps.wikimedia.org).
    #[arg(long)]
    base: Option<String>,
    /// The same directory on a mirror (repeatable), e.g.
    /// https://dumps.wikimedia.your.org/wikidatawiki/entities/
    #[arg(long = "mirror", value_name = "URL")]
    mirrors: Vec<String>,
    /// Connections per server.
    #[arg(long, default_value_t = 2)]
    connections: usize,
    /// Range size in MiB.
    #[arg(long, default_value_t = 8)]
    range_mib: u64,
    /// Skip the SHA-1 check.
    #[arg(long)]
    no_verify: bool,
    /// No progress on stderr.
    #[arg(short, long)]
    quiet: bool,
}

#[derive(Args)]
struct LiveArgs {
    /// Only these entities (comma-separated, repeatable).
    #[arg(long = "id", value_delimiter = ',')]
    ids: Vec<String>,
    /// Only the entities listed in this file.
    #[arg(long)]
    ids_file: Option<PathBuf>,
    /// Resume after this event id (the `eventId` of the last change).
    #[arg(long)]
    last_event_id: Option<String>,
    /// Start at this time (2026-10-24T00:00:00Z); about a week back at most.
    #[arg(long)]
    since: Option<String>,
    /// Namespaces (comma-separated): 0 items, 120 properties, 146 lexemes.
    #[arg(long, value_delimiter = ',', default_value = "0")]
    namespace: Vec<i64>,
    /// Stop after this many changes.
    #[arg(long)]
    limit: Option<u64>,
}

#[derive(Args)]
struct ExtractArgs {
    #[command(flatten)]
    input: InputArgs,
    #[command(flatten)]
    filter: FilterArgs,
    /// Output: .jsonl.zst (recommended), .json.bz2, .json.gz, .jsonl, …
    #[arg(short, long)]
    output: PathBuf,
    /// Compression level (default: zstd 9, bzip2 9, gzip 6).
    #[arg(long)]
    level: Option<u32>,
    /// Replace an existing output.
    #[arg(long)]
    overwrite: bool,
}

#[derive(Args)]
struct ClassesArgs {
    /// The dump to build from (a path or URL); omit with --index.
    input: Option<String>,
    /// Write the index here.
    #[arg(short, long)]
    output: Option<PathBuf>,
    /// Load this index instead of building one.
    #[arg(long)]
    index: Option<PathBuf>,
    /// Print the subclasses of these classes (comma-separated).
    #[arg(long, value_delimiter = ',')]
    subclasses_of: Vec<String>,
    /// Number of threads (0: all cores).
    #[arg(long, default_value_t = 0)]
    threads: usize,
    /// Read a truncated dump up to its last whole entity.
    #[arg(long)]
    allow_truncated: bool,
    /// No progress on stderr.
    #[arg(short, long)]
    quiet: bool,
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    match Cli::parse().command {
        Command::Info { input, json } => info(input, json),
        Command::Cat(a) => cat(*a),
        Command::Extract(a) => extract_cmd(*a),
        Command::Classes(a) => classes(*a),
        Command::Places(a) => places(*a),
        Command::Redirects(a) => redirects(*a),
        Command::Download(a) => download_cmd(*a),
        Command::Live(a) => live(*a),
    }
}

fn open(input: &str) -> Result<Dump> {
    Dump::open(input).with_context(|| format!("opening {input}"))
}

fn open_input(a: &InputArgs) -> Result<Dump> {
    let http = HttpOptions {
        mirrors: a.mirrors.clone(),
        connections: a.connections,
        ..Default::default()
    };
    Dump::open_with(a.input.as_str(), http).with_context(|| format!("opening {}", a.input))
}

fn ids_from_file(path: &Path) -> Result<Vec<String>> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(text
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect())
}

impl FilterArgs {
    fn build(&self) -> Result<Option<Filter>> {
        let mut parts = Vec::new();
        let mut ids = self.ids.clone();
        if let Some(f) = &self.ids_file {
            ids.extend(ids_from_file(f)?);
        }
        if !ids.is_empty() {
            parts.push(Filter::Ids(ids));
        }
        if !self.types.is_empty() {
            let types = self
                .types
                .iter()
                .map(|t| wikidata_model::EntityType::parse(t).with_context(|| format!("unknown type {t:?}")))
                .collect::<Result<_>>()?;
            parts.push(Filter::Types(types));
        }
        if !self.has.is_empty() {
            let ps = self
                .has
                .iter()
                .map(|p| Pid::parse(p).with_context(|| format!("not a property: {p:?}")))
                .collect::<Result<_>>()?;
            parts.push(Filter::HasProperty(ps));
        }
        for c in &self.claims {
            parts.push(Filter::parse_claim(c)?);
        }
        if !self.instance_of.is_empty() {
            parts.push(Filter::InstanceOf {
                classes: self.instance_of.clone(),
                subclasses: self.subclasses,
            });
        } else if self.subclasses {
            bail!("--subclasses goes with --instance-of");
        }
        if !self.sitelinks.is_empty() {
            parts.push(Filter::Sitelink(self.sitelinks.clone()));
        }
        if !self.in_country.is_empty() {
            parts.push(Filter::InCountry(self.in_country.clone()));
        }
        if let Some(path) = &self.revisions_file {
            let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
            let mut ids = Vec::new();
            let mut revisions = Vec::new();
            for (n, line) in text.lines().enumerate().filter(|(_, l)| !l.trim().is_empty()) {
                let mut f = line
                    .split(|c: char| c.is_whitespace() || c == ',')
                    .filter(|s| !s.is_empty());
                let (Some(id), Some(rev)) = (f.next(), f.next().and_then(|r| r.parse::<u64>().ok())) else {
                    bail!("{}:{}: expected `Q42 2000000001`", path.display(), n + 1);
                };
                ids.push(id.to_string());
                revisions.push(rev);
            }
            parts.push(Filter::Revisions { ids, revisions });
        }
        let mut f = match parts.len() {
            0 => None,
            _ if self.any => Some(Filter::any(parts)),
            _ => Some(Filter::all(parts)),
        };
        if let Some(j) = &self.filter {
            let text = if j.trim_start().starts_with(['{', '[']) {
                j.clone()
            } else {
                std::fs::read_to_string(j).with_context(|| format!("reading {j}"))?
            };
            let g = Filter::parse_json(&text)?;
            f = Some(match f {
                Some(f) => Filter::All(vec![f, g]),
                None => g,
            });
        }
        Ok(f)
    }

    fn compile(&self) -> Result<Option<Arc<wikidata_export::CompiledFilter>>> {
        let Some(f) = self.build()? else { return Ok(None) };
        let index = match &self.classes {
            Some(p) => Some(ClassIndex::load(p)?),
            None if f.needs_class_index() => {
                bail!("--subclasses needs a class index: build one with `wikidata classes DUMP -o classes.wdc`, then pass --classes classes.wdc")
            }
            None => None,
        };
        let places = match &self.places {
            Some(p) => Some(Arc::new(PlaceIndex::load(p)?)),
            None if f.needs_place_index() => {
                bail!("--in-country needs a place index: build one with `wikidata places DUMP -o places.wdp`, then pass --places places.wdp")
            }
            None => None,
        };
        let redirects = match &self.redirects {
            Some(p) => Some(Arc::new(RedirectIndex::load(p)?)),
            None => None,
        };
        let indexes = Indexes {
            classes: index.as_ref(),
            places,
            redirects,
        };
        Ok(Some(Arc::new(f.compile_with(&indexes)?)))
    }
}

fn list_or_none(v: &[String]) -> Option<Vec<String>> {
    match v {
        [] => None,
        [n] if n == "none" => Some(Vec::new()),
        v => Some(v.to_vec()),
    }
}

impl ModelArgs {
    fn build(&self) -> Result<ModelOptions> {
        let properties = match list_or_none(&self.properties) {
            None => None,
            Some(ps) => Some(
                ps.iter()
                    .map(|p| Pid::parse(p).with_context(|| format!("not a property: {p:?}")))
                    .collect::<Result<_>>()?,
            ),
        };
        let qualifier_properties = match list_or_none(&self.qualifiers) {
            None => None,
            Some(ps) => Some(
                ps.iter()
                    .map(|p| Pid::parse(p).with_context(|| format!("not a property: {p:?}")))
                    .collect::<Result<_>>()?,
            ),
        };
        Ok(ModelOptions {
            languages: list_or_none(&self.languages),
            qualifier_properties,
            sitelinks: list_or_none(&self.sites),
            properties,
            references: self.references,
            qualifiers: !self.no_qualifiers,
            deprecated: self.deprecated,
            best_rank: self.best_rank,
            aliases: !self.no_aliases,
            descriptions: !self.no_descriptions,
            derived: !self.no_derived,
            ..Default::default()
        })
    }
}

fn scan_options(a: &InputArgs, filter: Option<Arc<wikidata_export::CompiledFilter>>) -> ScanOptions {
    ScanOptions {
        filter,
        threads: a.threads,
        limit: a.limit,
        allow_truncated: a.allow_truncated,
        class_index: a.build_classes.clone(),
        place_index: a.build_places.clone(),
        checkpoint_every: a.checkpoint.as_ref().map(|_| a.checkpoint_every),
        resume: None,
    }
}

/// The checkpoint `--resume` names.
fn read_checkpoint(a: &InputArgs) -> Result<Option<Arc<Checkpoint>>> {
    let Some(path) = &a.resume else { return Ok(None) };
    let bytes = std::fs::read(path).with_context(|| format!("{}", path.display()))?;
    Ok(Some(Arc::new(
        Checkpoint::from_bytes(&bytes).with_context(|| format!("{}", path.display()))?,
    )))
}

/// Write a checkpoint through a temporary file, so `path` always holds a
/// whole one.
fn save_checkpoint(path: &Path, cp: &Checkpoint) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    std::fs::write(&tmp, cp.to_bytes())?;
    std::fs::rename(&tmp, path)
}

fn human_bytes(n: u64) -> String {
    let n = n as f64;
    if n >= 1e9 {
        format!("{:.2} GB", n / 1e9)
    } else if n >= 1e6 {
        format!("{:.1} MB", n / 1e6)
    } else if n >= 1e3 {
        format!("{:.1} kB", n / 1e3)
    } else {
        format!("{n} B")
    }
}

fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Progress on stderr: rewritten in place on a terminal, every 10 s
/// otherwise.
struct Reporter {
    quiet: bool,
    tty: bool,
    last_line: std::time::Instant,
    shown: bool,
}

impl Reporter {
    fn new(quiet: bool) -> Self {
        Self {
            quiet,
            tty: std::io::stderr().is_terminal(),
            last_line: std::time::Instant::now(),
            shown: false,
        }
    }

    fn line(p: &Progress) -> String {
        let s = (p.elapsed_ms as f64 / 1000.0).max(1e-3);
        let pct = p
            .bytes_total
            .filter(|&t| t > 0)
            .map(|t| {
                let f = p.bytes_in as f64 / t as f64;
                let eta = if f > 0.0 { s / f - s } else { 0.0 };
                format!(
                    " ({:.1}%, {}m{:02}s left)",
                    f * 100.0,
                    (eta / 60.0) as u64,
                    (eta % 60.0) as u64
                )
            })
            .unwrap_or_default();
        format!(
            "{} read{pct} · {} entities · {} matched · {:.1} MB/s in · {:.0} MB/s out · {:.0} entities/s",
            human_bytes(p.bytes_in),
            thousands(p.entities),
            thousands(p.matched),
            p.bytes_in as f64 / 1e6 / s,
            p.bytes_out as f64 / 1e6 / s,
            p.entities as f64 / s
        )
    }

    fn progress(&mut self, p: &Progress) {
        if self.quiet {
            return;
        }
        if self.tty {
            eprint!("\r\x1b[K{}", Self::line(p));
            self.shown = true;
        } else if self.last_line.elapsed().as_secs() >= 10 {
            self.last_line = std::time::Instant::now();
            eprintln!("{}", Self::line(p));
        }
    }

    fn done(&mut self) {
        if self.shown {
            eprintln!();
            self.shown = false;
        }
    }
}

fn info(a: InputArgs, json: bool) -> Result<()> {
    let dump = open_input(&a)?;
    let mut rep = Reporter::new(a.quiet || json);
    let info = file_info(&dump, &scan_options(&a, None), &mut |p| {
        rep.progress(&p);
        Flow::Continue
    })?;
    rep.done();
    if json {
        println!("{}", serde_json::to_string_pretty(&info)?);
        return Ok(());
    }
    let s = &info.scan;
    let secs = (s.elapsed_ms as f64 / 1000.0).max(1e-3);
    println!("File:         {}", info.input);
    if let Some(size) = info.size {
        println!("Size:         {} ({} bytes)", human_bytes(size), thousands(size));
    }
    println!("Compression:  {}", s.compression);
    if s.compression == "bzip2" {
        println!(
            "bzip2:        {} streams, {} blocks",
            thousands(s.streams),
            thousands(s.blocks)
        );
    }
    println!("Decompressed: {}", human_bytes(s.bytes_out));
    println!("Entities:     {}", thousands(s.entities));
    let t = &s.types;
    for (name, n, max) in [
        ("items", t.items, info.max_ids.item),
        ("properties", t.properties, info.max_ids.property),
        ("lexemes", t.lexemes, info.max_ids.lexeme),
        ("mediainfo", t.mediainfo, info.max_ids.mediainfo),
        ("entity schemas", t.entity_schemas, info.max_ids.entity_schema),
    ] {
        if n > 0 {
            println!("  {name:14}{:>14}  (highest id number {max})", thousands(n));
        }
    }
    println!(
        "Largest:      {} ({})",
        info.largest.id,
        human_bytes(info.largest.bytes)
    );
    if let Some(c) = &s.class_index {
        println!(
            "Class index:  {} ({} classes, {} subclass-of edges)",
            c.path,
            thousands(c.classes),
            thousands(c.edges)
        );
    }
    println!(
        "Time:         {:.1} s ({:.1} MB/s in, {:.0} MB/s out, {:.0} entities/s){}",
        secs,
        s.bytes_in as f64 / 1e6 / secs,
        s.bytes_out as f64 / 1e6 / secs,
        s.entities as f64 / secs,
        if s.truncated { " — TRUNCATED input" } else { "" }
    );
    Ok(())
}

fn is_broken_pipe(e: &ExportError) -> bool {
    e.to_string().contains("Broken pipe")
}

fn cat(a: CatArgs) -> Result<()> {
    let dump = open_input(&a.input)?;
    let filter = a.filter.compile()?;
    let out: Box<dyn Write> = match &a.output {
        Some(p) => Box::new(std::fs::File::create(p).with_context(|| format!("creating {}", p.display()))?),
        None => Box::new(std::io::stdout().lock()),
    };
    let mut out = BufWriter::with_capacity(1 << 20, out);
    let mut rep = Reporter::new(a.input.quiet || a.output.is_none() && std::io::stdout().is_terminal());
    let mut io_error = None;
    let checkpoint = a.input.checkpoint.clone();
    let mut sink = |e: Event<wikidata_export::Record>| -> Flow {
        match e {
            Event::Progress(p) => rep.progress(&p),
            Event::Records(rs) => {
                for r in rs {
                    if let Err(e) = out.write_all(&r.json).and_then(|_| out.write_all(b"\n")) {
                        io_error = Some(e);
                        return Flow::Stop;
                    }
                }
            }
            Event::Checkpoint(cp) => {
                // The records before it are written first.
                let saved = out
                    .flush()
                    .and_then(|_| save_checkpoint(checkpoint.as_deref().unwrap(), &cp));
                if let Err(e) = saved {
                    io_error = Some(e);
                    return Flow::Stop;
                }
            }
        }
        Flow::Continue
    };
    let mut scan = scan_options(&a.input, filter);
    scan.resume = read_checkpoint(&a.input)?;
    let referenced = (!a.referenced_labels.is_empty()).then(|| LabelOptions {
        languages: a.referenced_labels.clone(),
    });
    let stats = if a.raw {
        raw_lines(&dump, scan, &mut sink)
    } else {
        let opts = EntityOptions {
            scan,
            model: a.model.build()?,
            json: true,
            summary_only: true,
            referenced_labels: referenced,
            ..Default::default()
        };
        entities(&dump, &opts, &mut sink)
    };
    let stats = match stats {
        Err(e) if is_broken_pipe(&e) => return Ok(()),
        r => r?,
    };
    rep.done();
    if let Some(e) = io_error {
        if e.kind() == std::io::ErrorKind::BrokenPipe {
            return Ok(());
        }
        return Err(e.into());
    }
    match out.flush() {
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => return Ok(()),
        r => r?,
    }
    if let (Some(path), Some(labels)) = (&a.labels_output, &stats.referenced_labels) {
        std::fs::write(path, serde_json::to_vec(labels)?).with_context(|| format!("writing {}", path.display()))?;
    }
    if !a.input.quiet {
        eprintln!("{}", serde_json::to_string(&stats)?);
    }
    Ok(())
}

/// `cat --raw`: the original lines, through the same filtered scan as
/// `extract`, as records whose JSON is the line itself.
fn raw_lines(
    dump: &Dump,
    scan: ScanOptions,
    sink: &mut dyn FnMut(Event<wikidata_export::Record>) -> Flow,
) -> Result<wikidata_export::ScanStats, ExportError> {
    wikidata_export::raw_lines(dump, &scan, &mut |e| match e {
        Event::Progress(p) => sink(Event::Progress(p)),
        Event::Checkpoint(cp) => sink(Event::Checkpoint(cp)),
        Event::Records(lines) => sink(Event::Records(
            lines
                .into_iter()
                .map(|json| wikidata_export::Record {
                    entity: Default::default(),
                    json,
                    matched_by: Vec::new(),
                    merged_from: Vec::new(),
                })
                .collect(),
        )),
    })
}

fn extract_cmd(a: ExtractArgs) -> Result<()> {
    if a.output.exists() && !a.overwrite {
        bail!("{} exists (use --overwrite to replace it)", a.output.display());
    }
    let dump = open_input(&a.input)?;
    let opts = ExtractOptions {
        scan: scan_options(&a.input, a.filter.compile()?),
        write: WriteOptions {
            level: a.level,
            threads: a.input.threads,
            ..Default::default()
        },
        format: None,
    };
    let mut rep = Reporter::new(a.input.quiet);
    let stats = extract(&dump, &opts, &a.output, &mut |p| {
        rep.progress(&p);
        Flow::Continue
    })?;
    rep.done();
    if !a.input.quiet {
        eprintln!(
            "Wrote {} of {} entities to {} ({}) in {:.1} s",
            thousands(stats.scan.matched),
            thousands(stats.scan.entities),
            stats.output,
            human_bytes(stats.bytes_written),
            stats.scan.elapsed_ms as f64 / 1000.0
        );
    }
    println!("{}", serde_json::to_string(&stats)?);
    Ok(())
}

fn classes(a: ClassesArgs) -> Result<()> {
    let index = match (&a.index, &a.input) {
        (Some(p), _) => ClassIndex::load(p)?,
        (None, Some(input)) => {
            let dump = open(input)?;
            let mut rep = Reporter::new(a.quiet);
            let opts = ScanOptions {
                threads: a.threads,
                allow_truncated: a.allow_truncated,
                ..Default::default()
            };
            let (index, stats) = build_class_index(&dump, &opts, &mut |p| {
                rep.progress(&p);
                Flow::Continue
            })?;
            rep.done();
            if !a.quiet {
                eprintln!(
                    "{} classes with {} subclass-of edges, from {} entities in {:.1} s",
                    thousands(index.len() as u64),
                    thousands(index.edges() as u64),
                    thousands(stats.entities),
                    stats.elapsed_ms as f64 / 1000.0
                );
            }
            index
        }
        (None, None) => bail!("give a dump to build the index from, or --index to load one"),
    };
    if let Some(out) = &a.output {
        index.save(out)?;
        if !a.quiet {
            let size = std::fs::metadata(out).map(|m| m.len()).unwrap_or(0);
            eprintln!("Wrote {} ({})", out.display(), human_bytes(size));
        }
    }
    if !a.subclasses_of.is_empty() {
        let mut all: Vec<u32> = index.subclasses_of_ids(&a.subclasses_of).into_iter().collect();
        all.sort_unstable();
        let mut out = BufWriter::new(std::io::stdout().lock());
        for q in all {
            writeln!(out, "Q{q}")?;
        }
        out.flush()?;
    } else if a.output.is_none() {
        println!(
            "{{\"source\":{},\"classes\":{},\"edges\":{}}}",
            serde_json::to_string(index.source())?,
            index.len(),
            index.edges()
        );
    }
    Ok(())
}

fn places(a: PlacesArgs) -> Result<()> {
    let index = match (&a.index, &a.input) {
        (Some(p), _) => PlaceIndex::load(p)?,
        (None, Some(input)) => {
            let dump = open(input)?;
            let mut rep = Reporter::new(a.quiet);
            let opts = ScanOptions {
                threads: a.threads,
                allow_truncated: a.allow_truncated,
                class_index: a.build_classes.clone(),
                ..Default::default()
            };
            let (index, stats) = build_place_index(&dump, &opts, &mut |p| {
                rep.progress(&p);
                Flow::Continue
            })?;
            rep.done();
            if !a.quiet {
                eprintln!(
                    "{} items with a country ({} sets of countries, {} in memory), from {} entities in {:.1} s",
                    thousands(index.len() as u64),
                    thousands(index.sets() as u64),
                    human_bytes(index.heap_bytes() as u64),
                    thousands(stats.entities),
                    stats.elapsed_ms as f64 / 1000.0
                );
                if let Some(c) = &stats.class_index {
                    eprintln!("Class index:  {} ({} classes)", c.path, thousands(c.classes));
                }
            }
            index
        }
        (None, None) => bail!("give a dump to build the index from, or --index to load one"),
    };
    if let Some(out) = &a.output {
        index.save(out)?;
        if !a.quiet {
            let size = std::fs::metadata(out).map(|m| m.len()).unwrap_or(0);
            eprintln!("Wrote {} ({})", out.display(), human_bytes(size));
        }
    }
    if !a.country_of.is_empty() {
        let mut out = BufWriter::new(std::io::stdout().lock());
        for id in &a.country_of {
            writeln!(out, "{id}\t{}", index.country_of(id).join(","))?;
        }
        out.flush()?;
    } else if a.output.is_none() {
        println!(
            "{{\"source\":{},\"items\":{},\"sets\":{}}}",
            serde_json::to_string(index.source())?,
            index.len(),
            index.sets()
        );
    }
    Ok(())
}

fn redirects(a: RedirectsArgs) -> Result<()> {
    let index = match (&a.index, &a.redirect_sql, &a.page_sql) {
        (Some(p), _, _) => RedirectIndex::load(p)?,
        (None, Some(r), Some(p)) => {
            let mut rep = Reporter::new(a.quiet);
            let opts = RedirectOptions {
                threads: a.threads,
                ..Default::default()
            };
            let (index, stats) = build_redirect_index(&Input::parse(r), &Input::parse(p), &opts, &mut |p| {
                rep.progress(&p);
                Flow::Continue
            })?;
            rep.done();
            if !a.quiet {
                eprintln!(
                    "{} item redirects ({} redirect rows to items, {} pages) in {:.1} s",
                    thousands(index.len() as u64),
                    thousands(stats.redirect_rows),
                    thousands(stats.page_rows),
                    stats.elapsed_ms as f64 / 1000.0
                );
            }
            index
        }
        _ => bail!("give --redirect-sql and --page-sql to build the index, or --index to load one"),
    };
    if let Some(out) = &a.output {
        index.save(out)?;
        if !a.quiet {
            let size = std::fs::metadata(out).map(|m| m.len()).unwrap_or(0);
            eprintln!("Wrote {} ({})", out.display(), human_bytes(size));
        }
    }
    let mut out = BufWriter::new(std::io::stdout().lock());
    for id in &a.resolve {
        writeln!(out, "{id}\t{}", index.resolve_id(id).unwrap_or_default())?;
    }
    if a.list {
        for (f, t) in index.pairs() {
            writeln!(out, "{{\"from\":\"Q{f}\",\"to\":\"Q{t}\"}}")?;
        }
    }
    if a.resolve.is_empty() && !a.list && a.output.is_none() {
        writeln!(
            out,
            "{{\"source\":{},\"redirects\":{}}}",
            serde_json::to_string(index.source())?,
            index.len()
        )?;
    }
    out.flush()?;
    Ok(())
}

fn download_cmd(a: DownloadArgs) -> Result<()> {
    use wikidata_dump::download::{
        download_sql, latest_entity_dump, latest_sql_dump, DownloadStats, DumpStatus, SqlDownloadOptions, ENTITY_DUMPS,
        WIKI_DUMPS,
    };
    let sql = !a.tables.is_empty();
    let base = a
        .base
        .clone()
        .unwrap_or_else(|| (if sql { WIKI_DUMPS } else { ENTITY_DUMPS }).to_string());
    let http = HttpOptions {
        connections: Some(a.connections),
        range_size: a.range_mib.max(1) << 20,
        ..Default::default()
    };
    if a.latest {
        let s: DumpStatus = if sql {
            latest_sql_dump(&base, &a.tables, &http)?
        } else {
            latest_entity_dump(&base, &a.file, &http)?
        };
        let files: Vec<serde_json::Value> = s
            .files
            .iter()
            .map(|f| serde_json::json!({"name": f.name, "url": f.url, "size": f.size, "sha1": f.sha1}))
            .collect();
        println!("{}", serde_json::json!({"date": s.date, "files": files}));
        return Ok(());
    }
    let report = |stats: &DownloadStats| {
        if a.quiet {
            return;
        }
        if stats.already_done {
            eprintln!("{} is already there", stats.path.display());
        } else {
            eprintln!(
                "Wrote {} ({}, {} fetched, {} resumed) in {:.1} s{}",
                stats.path.display(),
                human_bytes(stats.bytes),
                human_bytes(stats.bytes_fetched),
                human_bytes(stats.bytes_resumed),
                stats.elapsed_ms as f64 / 1000.0,
                stats
                    .sha1
                    .as_ref()
                    .map(|s| format!(", SHA-1 {s} verified"))
                    .unwrap_or_default()
            );
        }
    };
    if sql {
        let opts = SqlDownloadOptions {
            base,
            mirrors: a.mirrors.clone(),
            date: a.date.clone(),
            tables: a.tables.clone(),
            dir: a.dir.clone(),
            http,
            verify: !a.no_verify,
        };
        let quiet = a.quiet;
        for stats in download_sql(&opts, &mut |p| {
            if !quiet && std::io::stderr().is_terminal() {
                eprint!(
                    "\r\x1b[K{}: {} of {}",
                    p.file,
                    human_bytes(p.bytes_done),
                    human_bytes(p.bytes_total)
                );
            }
            true
        })? {
            if !quiet && std::io::stderr().is_terminal() {
                eprintln!();
            }
            report(&stats);
            println!("{}", stats.path.display());
        }
        return Ok(());
    }
    let opts = DownloadOptions {
        base,
        mirrors: a.mirrors.clone(),
        date: a.date.clone(),
        file: a.file.clone(),
        dir: a.dir.clone(),
        http,
        verify: !a.no_verify,
    };
    let tty = std::io::stderr().is_terminal();
    let mut last = std::time::Instant::now();
    let stats = download(&opts, &mut |p| {
        if a.quiet || (!tty && last.elapsed().as_secs() < 10) {
            return true;
        }
        last = std::time::Instant::now();
        let s = (p.elapsed_ms as f64 / 1000.0).max(1e-3);
        let line = format!(
            "{} of {} ({:.1}%) · {:.1} MB/s · hashed {} · per server {}",
            human_bytes(p.bytes_done),
            human_bytes(p.bytes_total),
            p.bytes_done as f64 * 100.0 / p.bytes_total.max(1) as f64,
            p.bytes_fetched as f64 / 1e6 / s,
            human_bytes(p.bytes_hashed),
            p.per_server
                .iter()
                .map(|&b| human_bytes(b))
                .collect::<Vec<_>>()
                .join(" / ")
        );
        if tty {
            eprint!("\r\x1b[K{line}");
        } else {
            eprintln!("{line}");
        }
        true
    })?;
    if tty && !a.quiet {
        eprintln!();
    }
    report(&stats);
    println!("{}", stats.path.display());
    Ok(())
}

fn live(a: LiveArgs) -> Result<()> {
    let mut ids = a.ids.clone();
    if let Some(f) = &a.ids_file {
        ids.extend(ids_from_file(f)?);
    }
    let set = (!ids.is_empty()).then(|| {
        let b = wikidata_filter::IdSetBuilder::new();
        for id in &ids {
            if let Some(k) = wikidata_filter::id_key(id) {
                b.insert(k);
            }
        }
        Arc::new(b.freeze())
    });
    let opts = LiveOptions {
        ids: set,
        last_event_id: a.last_event_id,
        since: a.since,
        namespaces: a.namespace,
        ..Default::default()
    };
    let mut out = std::io::stdout().lock();
    let mut n = 0u64;
    let limit = a.limit.unwrap_or(u64::MAX);
    let mut failed = None;
    live_changes(&opts, &|| false, &mut |changes| {
        for c in changes {
            let line = serde_json::to_string(&c).unwrap_or_default();
            if let Err(e) = writeln!(out, "{line}").and_then(|_| out.flush()) {
                failed = Some(e);
                return Flow::Stop;
            }
            n += 1;
            if n >= limit {
                return Flow::Stop;
            }
        }
        Flow::Continue
    })?;
    match failed {
        Some(e) if e.kind() != std::io::ErrorKind::BrokenPipe => Err(e.into()),
        _ => Ok(()),
    }
}
