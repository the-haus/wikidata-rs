//! `wikidata`: filter, extract and inspect Wikidata JSON dumps
//! (`wikidata-*-all.json.bz2`, `.gz`, `.zst`, plain, local or over HTTP).

use std::io::{BufWriter, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use wikidata_export::{
    build_class_index, entities, extract, file_info, ClassIndex, Dump, EntityOptions, Event, ExportError,
    ExtractOptions, Filter, Flow, ModelOptions, Progress, ScanOptions, WriteOptions,
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
}

#[derive(Args)]
struct InputArgs {
    /// The dump: a path or an http(s):// URL.
    input: String,
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
    }
}

fn open(input: &str) -> Result<Dump> {
    Dump::open(input).with_context(|| format!("opening {input}"))
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
        Ok(Some(Arc::new(f.compile(index.as_ref())?)))
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
        Ok(ModelOptions {
            languages: list_or_none(&self.languages),
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
    }
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
    let dump = open(&a.input)?;
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
    let dump = open(&a.input.input)?;
    let filter = a.filter.compile()?;
    let out: Box<dyn Write> = match &a.output {
        Some(p) => Box::new(std::fs::File::create(p).with_context(|| format!("creating {}", p.display()))?),
        None => Box::new(std::io::stdout().lock()),
    };
    let mut out = BufWriter::with_capacity(1 << 20, out);
    let mut rep = Reporter::new(a.input.quiet || a.output.is_none() && std::io::stdout().is_terminal());
    let mut io_error = None;
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
        }
        Flow::Continue
    };
    let scan = scan_options(&a.input, filter);
    let stats = if a.raw {
        raw_lines(&dump, scan, &mut sink)
    } else {
        let opts = EntityOptions {
            scan,
            model: a.model.build()?,
            json: true,
            summary_only: true,
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
        Event::Records(lines) => sink(Event::Records(
            lines
                .into_iter()
                .map(|json| wikidata_export::Record {
                    entity: Default::default(),
                    json,
                    matched_by: Vec::new(),
                })
                .collect(),
        )),
    })
}

fn extract_cmd(a: ExtractArgs) -> Result<()> {
    if a.output.exists() && !a.overwrite {
        bail!("{} exists (use --overwrite to replace it)", a.output.display());
    }
    let dump = open(&a.input.input)?;
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
