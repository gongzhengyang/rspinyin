//! `xtask dictc` -- the offline dictionary compiler.
//!
//! Responsibility: turn the whitelisted raw sources under `data/raw/` into the
//! container described by `ime_dict::format`, and report what it did. The
//! pipeline is: source allowlist -> L1 single-character readings -> word list
//! (L3a baseline keys) -> L3b frequency-targeted multi-key expansion -> L3c
//! polyphone weight correction -> ENTRIES/STRPOOL/UNIGRAM/WORDLIST/FST -> one
//! atomic write.
//!
//! Boundaries: this is a build-time tool. It is the only place that knows the raw
//! source formats, it never runs at IME runtime, and it holds no state between
//! runs. Every diagnostic is English and none of them contains user input.
//!
//! The module is split by responsibility: this root holds the command line, the
//! source allowlist and the pipeline, [`source`] reads the raw text formats, and
//! [`build`] assembles the container.
//!
//! # Why the expansion is bounded
//!
//! Expanding every word into the cartesian product of its characters' readings
//! costs about +176% FST keys and buys little over expanding only the words people
//! actually type: the highest-frequency band recovers most of the available
//! benefit for a fraction of the cost. The band is therefore a frequency
//! threshold, and the build fails when the measured key growth passes
//! `--max-key-growth`.

mod budget;
mod build;
mod manifest;
pub mod quality;
mod source;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use clap::Args;
use ime_dict::format::SectionKind;
use ime_dict::format::writer::DictWriter;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::dictc::build::{Band, Stats, compile};
use crate::dictc::source::{
    Skips, apply_polyphone, load_l1, load_polyphone, load_words, read_source, synth_words,
};

/// Default word list, relative to the repository root.
pub const DEFAULT_INPUT: &str = "data/raw/base.tsv";
/// Default output file, relative to the repository root.
pub const DEFAULT_OUTPUT: &str = "data/compiled/base.dict";
/// Default L3c weight-correction table.
pub const DEFAULT_POLYPHONE: &str = "data/raw/polyphone.tsv";
/// Default L1 source: one character per row, readings separated by commas.
pub const DEFAULT_L1: &str = "data/raw/pinyin-data.tsv";
/// Default L4 source: one word per row, word frequency in the second column.
pub const DEFAULT_L4: &str = "data/raw/jieba-dict.tsv";
/// Default source allowlist.
pub const DEFAULT_SOURCES: &str = "data/sources.toml";

/// Command-line surface of `xtask dictc`.
#[derive(Debug, Args)]
pub struct DictcArgs {
    /// Repository root; defaults to the directory containing the xtask crate.
    #[arg(long)]
    root: Option<PathBuf>,
    /// Word list to compile, in `word<TAB>reading<TAB>weight<TAB>flags` columns.
    #[arg(long, default_value = DEFAULT_INPUT)]
    input: PathBuf,
    /// Container to write.
    #[arg(long, default_value = DEFAULT_OUTPUT)]
    output: PathBuf,
    /// L3c weight-correction table, in `word<TAB>reading` columns.
    #[arg(long, default_value = DEFAULT_POLYPHONE)]
    polyphone: PathBuf,
    /// L1 source of single-character readings.
    #[arg(long, default_value = DEFAULT_L1)]
    l1: PathBuf,
    /// L4 source whose frequency ranking defines the expansion band.
    #[arg(long, default_value = DEFAULT_L4)]
    l4: PathBuf,
    /// Source allowlist.
    #[arg(long, default_value = DEFAULT_SOURCES)]
    sources: PathBuf,
    /// Expand the N highest-frequency words of the L4 source; 0 disables L3b.
    ///
    /// The default is tuned for the development subset committed to this repository,
    /// not for the full dictionary. ADR-0000 measured the production expansion at +23%
    /// of keys using the top 50k words of a 400k-word dictionary, and that is the value
    /// the release build matrix passes. The committed subset holds 5441 words drawn from
    /// across the frequency range, so the same cut admits 43% of it and expands to
    /// +38%, over the growth ceiling. Cutting at weight 3000 instead lands the subset at
    /// +22.5%, which is what makes it a useful stand-in for the real thing.
    #[arg(long, default_value_t = 2_856)]
    expand_top: u64,
    /// L3b cap on the number of keys generated per word.
    #[arg(long, default_value_t = 4)]
    expand_cap: usize,
    /// Absolute weight floor for the expansion band, overriding `--expand-top`.
    #[arg(long)]
    expand_min_weight: Option<u32>,
    /// Key-growth ceiling in percent; a larger growth fails the build.
    #[arg(long, default_value_t = 30.0)]
    max_key_growth: f64,
    /// Compile N synthetic words instead of reading the input file.
    #[arg(long, hide = true)]
    synth: Option<u32>,
}

/// One row of the source allowlist.
#[derive(Debug, Deserialize)]
struct Source {
    /// File name stem: the raw file is `data/raw/<id>.tsv`.
    id: String,
    /// `upstream` (hash-pinned download) or `derived` (generated here).
    kind: String,
    /// Licence identifier.
    spdx: String,
    /// Human-readable licence name.
    license: String,
    /// Source URL.
    url: String,
    /// Download date, empty until the file is fetched.
    #[serde(default)]
    retrieved: String,
    /// Pinned SHA256, empty for derived sources.
    #[serde(default)]
    sha256: String,
    /// Whether the licence permits redistribution of derived data.
    permissive: bool,
    /// ADR-0000 layer the source feeds: `L1`, `L2`, `L3`, `L4` or `L5`.
    ///
    /// The manifest records it, so a reader of a compiled dictionary can trace a weight
    /// back to the layer that supplied it without consulting this file.
    layer: String,
}

/// The parsed allowlist document.
#[derive(Debug, Deserialize)]
struct Allowlist {
    /// Registered sources.
    source: Vec<Source>,
}

/// Entry point for `xtask dictc`.
///
/// # Errors
/// Returns an error when a source is not on the allowlist or fails its hash pin,
/// when a polyphone row is not a legal syllable sequence, when the key growth
/// passes the ceiling, or when any file cannot be read or written. The error names
/// the file and row, so the offending input can be fixed directly.
pub fn run(args: DictcArgs) -> Result<()> {
    let root = resolve_root(args.root.as_deref())?;
    let allowlist = load_allowlist(&root.join(&args.sources))?;

    // The allowlist gate comes first: a source that is not registered, or whose
    // bytes do not match the pinned hash, must stop the build before any of it is
    // compiled, because a copyleft word list merged into base.dict cannot be
    // removed afterwards without rebuilding every weight and every rank.
    let l1_path = root.join(&args.l1);
    let l4_path = root.join(&args.l4);
    let polyphone_path = root.join(&args.polyphone);
    let input_path = root.join(&args.input);
    check_source(&allowlist, &l1_path)?;
    check_source(&allowlist, &l4_path)?;
    check_source(&allowlist, &polyphone_path)?;
    if args.synth.is_none() {
        check_source(&allowlist, &input_path)?;
    }

    let started = Instant::now();
    let l1 = load_l1(&l1_path)?;
    let band = build::band_threshold(&l4_path, args.expand_top, args.expand_min_weight)?;
    let parsed = Instant::now();
    let (words, skips) = match args.synth {
        Some(count) => (synth_words(count, &l1, band.threshold), Skips::default()),
        None => load_words(&input_path, &l1, band.threshold)?,
    };
    let polyphone = load_polyphone(&polyphone_path)?;
    let (corrections, unmatched) = apply_polyphone(&words, &polyphone);
    let mut compiled = compile(&words, band, args.expand_cap, &corrections)?;
    compiled.stats.polyphone_unmatched = unmatched;
    let built = Instant::now();

    // The section budgets are checked before anything is written: a container that is
    // over its ceiling is one to refuse, not one to emit and then complain about.
    let ledger = budget::enforce(&root, &compiled)?;

    let mut writer = DictWriter::new();
    writer.add_section(SectionKind::Fst, compiled.fst)?;
    writer.add_section(SectionKind::Entries, compiled.entries)?;
    writer.add_section(SectionKind::StrPool, compiled.strpool)?;
    writer.add_section(SectionKind::Unigram, compiled.unigram)?;
    writer.add_section(SectionKind::WordList, compiled.wordlist)?;
    let output_path = root.join(&args.output);
    writer.finish(&output_path)?;
    let written = Instant::now();

    // The manifest is written beside the container and describes what produced it: which
    // sources, at which layer, under which licence, and what the expansion band did. It
    // is the record that makes a compiled dictionary reproducible and auditable, so it is
    // written from the same values the container was built from rather than re-derived.
    let facts = manifest::BuildFacts {
        sources: vec![
            source_identity(&allowlist, &l1_path)?,
            source_identity(&allowlist, &l4_path)?,
            source_identity(&allowlist, &polyphone_path)?,
            source_identity(&allowlist, &input_path)?,
        ],
        expansion: manifest::ExpansionRecord {
            band_weight: band.threshold,
            band_rank: band.requested,
            // The manifest carries the cap as the `u32` the container records it in; a
            // cap past that range is one no container could describe.
            cap: u32::try_from(args.expand_cap)
                .context("dictc: --expand-cap is past the range a container records")?,
            band_words: compiled.stats.band_words,
            expanded_words: compiled.stats.expanded_words,
            added_keys: compiled.stats.expanded_keys,
        },
        keys: compiled.stats.keys,
    };
    manifest::record_build(&root, &output_path, &ledger, facts)?;

    report(
        &output_path,
        &compiled.stats,
        &skips,
        band,
        args.expand_cap,
        [
            parsed.duration_since(started),
            built.duration_since(parsed),
            written.duration_since(built),
            written.duration_since(started),
        ],
    )?;

    let growth = growth_percent(compiled.stats.keys, compiled.stats.baseline_keys());
    ensure!(
        growth <= args.max_key_growth,
        "dictc: key growth {growth:.1}% exceeds the {:.1}% ceiling; \
         lower --expand-top or --expand-cap",
        args.max_key_growth
    );
    Ok(())
}

/// Prints the build log: what went in, what came out, and what was dropped.
fn report(
    output: &Path,
    stats: &Stats,
    skips: &Skips,
    band: Band,
    cap: usize,
    times: [Duration; 4],
) -> Result<()> {
    let size = fs::metadata(output)
        .with_context(|| format!("reading back {}", output.display()))?
        .len();
    let digest = Sha256::digest(fs::read(output)?);
    println!(
        "dictc: {} words, {} keys, {} (key growth {:+.1}%)",
        stats.words,
        stats.keys,
        human_size(size),
        growth_percent(stats.keys, stats.baseline_keys())
    );
    println!(
        "dictc: shape 1/2/3/4/5+ = {}/{}/{}/{}/{}",
        stats.shape[0], stats.shape[1], stats.shape[2], stats.shape[3], stats.shape[4]
    );
    println!(
        "dictc: L3b band weight >= {} (top {} of the frequency source): {} words in band, \
         cap {cap}, {} words expanded, {} keys generated",
        band.threshold, band.requested, stats.band_words, stats.expanded_words, stats.expanded_keys
    );
    println!(
        "dictc: L3c polyphone corrections: {} keys added or reweighted, {} rows unmatched, \
         {} (key, word) pairs dropped at the 32-word ceiling",
        stats.polyphone_keys, stats.polyphone_unmatched, stats.truncated_pairs
    );
    println!(
        "dictc: {} polyphone-risk words (a character of the word has several readings)",
        stats.polyphone_risk
    );
    if !stats.expanded_examples.is_empty() {
        let sample: Vec<String> = stats
            .expanded_examples
            .iter()
            .map(|(word, keys)| format!("{word}={keys}"))
            .collect();
        println!("dictc: L3b sample, word=keys: {}", sample.join(" "));
    }
    if skips.total() > 0 {
        println!(
            "dictc: skipped {} rows (non-han {}, too-long {}, bad-weight {}, bad-flags {}, \
             bad-reading {}, unknown-char {}, duplicate {})",
            skips.total(),
            skips.non_han,
            skips.too_long,
            skips.bad_weight,
            skips.bad_flags,
            skips.bad_reading,
            skips.unknown_char,
            skips.duplicate
        );
        for example in &skips.examples {
            println!("dictc:   {example}");
        }
    }
    println!(
        "dictc: {} written, sha256 {}, parse {:.0}ms build {:.0}ms write {:.0}ms total {:.0}ms",
        output.display(),
        hex(&digest),
        times[0].as_secs_f64() * 1e3,
        times[1].as_secs_f64() * 1e3,
        times[2].as_secs_f64() * 1e3,
        times[3].as_secs_f64() * 1e3
    );
    Ok(())
}

/// Resolves the repository root.
fn resolve_root(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(root) = explicit {
        return Ok(root.to_path_buf());
    }
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .map(Path::to_path_buf)
        .context("xtask is expected to live in a subdirectory of the repository root")
}

/// Loads and schema-checks the source allowlist.
fn load_allowlist(path: &Path) -> Result<Allowlist> {
    let text =
        fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    let allowlist: Allowlist = toml::from_str(&text)
        .with_context(|| format!("{}: invalid allowlist document", path.display()))?;
    for source in &allowlist.source {
        ensure!(
            !source.id.is_empty()
                && !source.kind.is_empty()
                && !source.spdx.is_empty()
                && !source.license.is_empty()
                && !source.url.is_empty(),
            "{}: source `{}` is missing one of id/kind/spdx/license/url",
            path.display(),
            source.id
        );
        ensure!(
            source.permissive,
            "{}: source `{}` is not marked permissive; a copyleft source needs an ADR \
             before it may enter base.dict",
            path.display(),
            source.id
        );
        if source.kind == "upstream" {
            ensure!(
                !source.sha256.is_empty() && !source.retrieved.is_empty(),
                "{}: upstream source `{}` needs both `sha256` and `retrieved` before it \
                 can be compiled",
                path.display(),
                source.id
            );
        }
    }
    Ok(allowlist)
}

/// Enforces the allowlist for one input file.
///
/// The file's stem is its source id, which is what ties `data/raw/<id>.tsv` to the
/// registered entry. An unregistered file is refused outright, and the error names
/// the id so the caller can see which source is missing from the allowlist.
///
/// # Errors
/// Returns an error when the file's id is not registered, when the source is not
/// permissive, when an upstream source's file cannot be read, or when its SHA256 does
/// not match the pin. A file that is simply absent is reported as the stable
/// `dict/source/missing` diagnostic, because the raw sources are fetched rather than
/// committed and "not fetched yet" is the state a fresh checkout is in.
/// The manifest's identity for one source file, taken from the allowlist.
///
/// # Errors
///
/// Returns an error when the file is not a registered source: the manifest has to name
/// the id, the layer and the licence a compiled dictionary was built from, and a file
/// with none of those is one the build should not have read in the first place.
fn source_identity(allowlist: &Allowlist, path: &Path) -> Result<manifest::SourceIdentity> {
    let id = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .with_context(|| format!("{}: unusable file name", path.display()))?;
    let source = allowlist
        .source
        .iter()
        .find(|source| source.id == id)
        .with_context(|| format!("{}: source `{id}` is not registered", path.display()))?;
    Ok(manifest::SourceIdentity {
        id: source.id.clone(),
        kind: source.kind.clone(),
        layer: source.layer.clone(),
        spdx: source.spdx.clone(),
        path: path.to_path_buf(),
    })
}

fn check_source(allowlist: &Allowlist, path: &Path) -> Result<()> {
    let id = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .with_context(|| format!("{}: unusable file name", path.display()))?;
    let source = allowlist
        .source
        .iter()
        .find(|source| source.id == id)
        .with_context(|| {
            format!(
                "{}: source `{id}` is not registered in the allowlist; \
                 add it to data/sources.toml or remove the file",
                path.display()
            )
        })?;
    ensure!(
        source.permissive,
        "{}: source `{id}` is not permissive",
        path.display()
    );
    if source.kind != "upstream" {
        return Ok(());
    }
    let text = read_source(path)?;
    let digest = Sha256::digest(text.as_bytes());
    let actual = hex(&digest);
    ensure!(
        actual.eq_ignore_ascii_case(&source.sha256),
        "{}: source `{id}` sha256 mismatch: allowlist says {}, file hashes to {actual}",
        path.display(),
        source.sha256
    );
    Ok(())
}

/// Key growth in percent: how many keys the expansion added over the L3a baseline.
///
/// The denominator is the baseline *key* count, not the word count. Words collapse onto
/// shared keys — 5441 words can yield fewer distinct keys than there are words — so a
/// keys-per-word ratio mostly measures that collapse and can report zero growth while
/// the expansion is in fact adding hundreds of keys. The recorded figure for the full
/// dictionary (`+23%`, 337k → 414k) is a key-to-key comparison of the same shape.
fn growth_percent(keys: u64, baseline_keys: u64) -> f64 {
    if baseline_keys == 0 {
        return 0.0;
    }
    (keys.saturating_sub(baseline_keys)) as f64 * 100.0 / baseline_keys as f64
}

/// Renders a byte count for the build log.
fn human_size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.2}MiB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1}KiB", bytes as f64 / 1024.0)
    }
}

/// Renders a digest as lower-case hexadecimal.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    use ime_dict::format::reader::Reader;

    /// A scratch directory unique to this test process and tag.
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rspinyin-dictc-{tag}-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("creating the scratch directory");
        dir
    }

    /// The budget document a scratch repository root needs.
    ///
    /// The compiler reads the section ceilings it enforces from `docs/dev/budgets.json`
    /// relative to the root it was given, so a build rooted anywhere else needs a document
    /// of its own. The schema refuses one with a missing key, so this is the whole shape;
    /// the values are ones no assertion here depends on, and the only one that matters is
    /// `size_mb.base_dict`, which the container ceiling is derived from.
    const BUDGETS_FIXTURE: &str = r#"{
  "version": 1,
  "source": "fixture",
  "latency_ms": {"key_to_present_p50": 1.0, "key_to_present_p99": 1.0, "key_to_present_p99_144hz": 1.0, "decode_p99": 1.0, "decode_p999": 1.0, "raster_p99": 1.0, "first_key_to_visible_p99": 1.0, "addon_load": 1.0},
  "memory_mb": {"ui_rss": 1.0, "plugin_rss": 1.0, "dict_mmap_rss": 1.0},
  "cpu_pct": {"idle": 1.0, "typing_10cps": 1.0, "idle_redraw_count": 0, "idle_poll_timer_count": 0},
  "size_mb": {"so_stripped": 1.0, "base_dict": 20.0},
  "alloc_count": {"decode_steady": 1},
  "robustness": {"soak_hours": 1.0, "rss_drift_mb": 1.0, "pass_rate_pct": 1.0},
  "bench": {"passthrough_classify_ns": 1.0, "input_buffer_ops_us": 1.0, "decode_holdout_s": 1.0, "ui_wakeup_latency_us": 1.0},
  "net_sockets": 0
}"#;

    /// The L1 character table of the scratch repository.
    const FIXTURE_L1: &str = "中\tzhong\n国\tguo\n银\tyin\n行\txing,hang\n心\txin\n";

    /// The word list of the scratch repository.
    ///
    /// Three rows a compiler can use and one it cannot: `行` states a reading that is not
    /// a syllable sequence, which is the row the build has to count and skip rather than
    /// fail on.
    const FIXTURE_WORDS: &str = "中国\t\t5000\n银行\t\t4000\n中心\t\t3000\n行\tzzz\t10\n";

    /// The frequency ranking of the scratch repository, which places the band.
    const FIXTURE_FREQUENCIES: &str = "中\t5000\n国\t4000\n心\t3000\n";

    /// The correction table of the scratch repository: the reading `银行` actually has.
    const FIXTURE_POLYPHONE: &str = "银行\tyin'hang\n";

    /// Writes a scratch repository root holding the five files the compiler reads, and
    /// returns it.
    ///
    /// The root carries its own allowlist, raw sources and budget document, so a test can
    /// drive the whole command -- allowlist gate, parse, expand, correct, build, account,
    /// write -- and still depend on no committed data. The root is rebuilt from nothing on
    /// every call, so a leftover from an interrupted run cannot be mistaken for something
    /// this one wrote.
    fn scratch_repository(tag: &str) -> PathBuf {
        let root = scratch(tag);
        // A missing directory is the ordinary case and not a failure; a stale one would be.
        let _ = fs::remove_dir_all(&root);
        let raw = root.join("data/raw");
        let docs = root.join("docs/dev");
        fs::create_dir_all(&raw).expect("creating data/raw");
        fs::create_dir_all(&docs).expect("creating docs/dev");
        fs::write(raw.join("pinyin-data.tsv"), FIXTURE_L1).expect("writing the L1 source");
        fs::write(raw.join("base.tsv"), FIXTURE_WORDS).expect("writing the word list");
        fs::write(raw.join("jieba-dict.tsv"), FIXTURE_FREQUENCIES)
            .expect("writing the frequency source");
        fs::write(raw.join("polyphone.tsv"), FIXTURE_POLYPHONE)
            .expect("writing the correction table");
        fs::write(docs.join("budgets.json"), BUDGETS_FIXTURE).expect("writing the budgets");

        // The two upstream sources are hash-pinned exactly as the real allowlist pins the
        // fetched files; the two derived ones carry no pin, because the project generates
        // them and they change whenever the generator does.
        let allowlist = format!(
            "[[source]]\nid = \"pinyin-data\"\nkind = \"upstream\"\nlayer = \"L1\"\n\
             url = \"https://example.invalid/pinyin-data\"\nlicense = \"MIT\"\nspdx = \"MIT\"\n\
             retrieved = \"2026-09-29\"\nsha256 = \"{}\"\npermissive = true\n\n\
             [[source]]\nid = \"jieba-dict\"\nkind = \"upstream\"\nlayer = \"L4\"\n\
             url = \"https://example.invalid/jieba-dict\"\nlicense = \"MIT\"\nspdx = \"MIT\"\n\
             retrieved = \"2026-09-29\"\nsha256 = \"{}\"\npermissive = true\n\n\
             [[source]]\nid = \"base\"\nkind = \"derived\"\nlayer = \"L2\"\n\
             url = \"https://example.invalid/base\"\nlicense = \"Project-owned\"\n\
             spdx = \"MIT OR Apache-2.0\"\npermissive = true\n\n\
             [[source]]\nid = \"polyphone\"\nkind = \"derived\"\nlayer = \"L3c\"\n\
             url = \"https://example.invalid/polyphone\"\nlicense = \"Project-owned\"\n\
             spdx = \"MIT OR Apache-2.0\"\npermissive = true\n",
            hex(&Sha256::digest(FIXTURE_L1.as_bytes())),
            hex(&Sha256::digest(FIXTURE_FREQUENCIES.as_bytes())),
        );
        fs::write(root.join("data/sources.toml"), allowlist).expect("writing the allowlist");
        root
    }

    /// The arguments one scratch build uses, rooted at `root`.
    ///
    /// The band is asked for two entries of the frequency source, which puts the words
    /// weighted 4000 and above inside it. The growth ceiling is raised because the fixture
    /// is three words and its expansion moves the key count far more than a real word
    /// list's does; the ceiling a release build is held to is asserted by the compiler's
    /// own growth test.
    fn scratch_args(root: &Path) -> DictcArgs {
        DictcArgs {
            root: Some(root.to_path_buf()),
            input: PathBuf::from(DEFAULT_INPUT),
            output: PathBuf::from(DEFAULT_OUTPUT),
            polyphone: PathBuf::from(DEFAULT_POLYPHONE),
            l1: PathBuf::from(DEFAULT_L1),
            l4: PathBuf::from(DEFAULT_L4),
            sources: PathBuf::from(DEFAULT_SOURCES),
            expand_top: 2,
            expand_cap: 4,
            expand_min_weight: None,
            max_key_growth: 500.0,
            synth: None,
        }
    }

    #[test]
    fn test_run_compiles_a_reproducible_container_the_reader_accepts() {
        let root = scratch_repository("run-compile");
        let output = root.join(DEFAULT_OUTPUT);

        run(scratch_args(&root)).expect("the first build");
        let first = fs::read(&output).expect("reading the first container");
        run(scratch_args(&root)).expect("the second build");
        let second = fs::read(&output).expect("reading the second container");

        // Two runs over one input have to produce one container: `base.dict` is a shipped
        // artifact, and its digest is what a release records and what the update path
        // compares.
        assert_eq!(first, second, "the container must be reproducible");
        assert_eq!(hex(&Sha256::digest(&first)), hex(&Sha256::digest(&second)));
        assert_eq!(&first[0..4], b"RSPD", "the image is a container");

        // And what the compiler wrote has to be what the read path accepts with every
        // checksum verified, which is the contract the container exists to keep. The row
        // whose reading is not a syllable was counted and skipped rather than compiled.
        let reader = Reader::open(&output).expect("opening the compiled container");
        assert_eq!(reader.entry_count(), 3);
        assert_eq!(reader.key_words("zhong'guo").expect("lookup"), vec!["中国"]);
        // The row states no reading, so the baseline comes from the L1 table (`yin'xing`)
        // and the correction table adds the reading the word actually has.
        assert_eq!(reader.key_words("yin'xing").expect("lookup"), vec!["银行"]);
        assert_eq!(reader.key_words("yin'hang").expect("lookup"), vec!["银行"]);
        assert!(
            reader.key_words("xing").expect("lookup").is_empty(),
            "the skipped row left no key behind"
        );

        fs::remove_dir_all(&root).expect("cleaning up");
    }

    #[test]
    fn test_run_refuses_a_source_the_allowlist_does_not_register() {
        let root = scratch_repository("run-allowlist");
        // A file the allowlist never named, handed to the compiler as its word list.
        fs::write(root.join("data/raw/rogue.tsv"), FIXTURE_WORDS)
            .expect("writing the unregistered source");
        let mut args = scratch_args(&root);
        args.input = PathBuf::from("data/raw/rogue.tsv");

        let refusal = run(args).expect_err("an unregistered source must stop the build");
        assert!(
            refusal.to_string().contains("rogue"),
            "the refusal names the source: {refusal}"
        );
        assert!(
            !root.join(DEFAULT_OUTPUT).exists(),
            "nothing may be written before the gate passes"
        );
        fs::remove_dir_all(&root).expect("cleaning up");
    }

    #[test]
    fn test_run_refuses_a_source_whose_pinned_hash_does_not_match() {
        let root = scratch_repository("run-pin");
        // Rewriting a pinned source after the allowlist was written is what the pin is
        // for: the bytes the build would consume are no longer the bytes that were
        // reviewed, and a dictionary cannot be un-mixed once it has been compiled.
        fs::write(root.join("data/raw/pinyin-data.tsv"), "中\tzhong\n")
            .expect("rewriting the pinned source");

        let refusal = run(scratch_args(&root)).expect_err("a changed source must stop the build");
        assert!(
            refusal.to_string().contains("sha256 mismatch"),
            "the refusal names the pin: {refusal}"
        );
        fs::remove_dir_all(&root).expect("cleaning up");
    }

    /// One allowlist entry.
    fn source(id: &str, kind: &str, sha256: &str) -> Source {
        Source {
            id: id.to_owned(),
            kind: kind.to_owned(),
            spdx: "MIT".to_owned(),
            license: "MIT".to_owned(),
            url: "https://example.invalid".to_owned(),
            retrieved: "2026-09-29".to_owned(),
            sha256: sha256.to_owned(),
            permissive: true,
            layer: "L4".to_owned(),
        }
    }

    #[test]
    fn test_check_source_refuses_unregistered_and_mismatched_sources() {
        let dir = scratch("gate");
        let registered = dir.join("base.tsv");
        fs::write(&registered, "中国\n").expect("writing the fixture");
        let unregistered = dir.join("sogou.tsv");
        fs::write(&unregistered, "中国\n").expect("writing the fixture");

        let allowlist = Allowlist {
            source: vec![
                source("base", "derived", ""),
                source("pinyin-data", "upstream", &"0".repeat(64)),
            ],
        };

        check_source(&allowlist, &registered).expect("a derived source needs no hash");
        let refusal = check_source(&allowlist, &unregistered).expect_err("unregistered source");
        assert!(
            refusal.to_string().contains("sogou"),
            "the refusal names the source: {refusal}"
        );
        let missing = check_source(&allowlist, &dir.join("pinyin-data.tsv"))
            .expect_err("a missing pinned file must fail");
        assert!(
            missing.to_string().contains("dict/source/missing"),
            "a source that was never fetched reports the fetch diagnostic: {missing}"
        );
        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_check_source_refuses_a_hash_that_does_not_match() {
        let dir = scratch("hash");
        let path = dir.join("pinyin-data.tsv");
        fs::write(&path, "中\tzhong\n").expect("writing the fixture");
        let digest = hex(&Sha256::digest("中\tzhong\n".as_bytes()));
        let matching = Allowlist {
            source: vec![source("pinyin-data", "upstream", &digest)],
        };
        check_source(&matching, &path).expect("the pinned hash matches");

        let wrong = Allowlist {
            source: vec![source("pinyin-data", "upstream", &"a".repeat(64))],
        };
        let failure = check_source(&wrong, &path).expect_err("a wrong hash must be refused");
        assert!(failure.to_string().contains("sha256 mismatch"), "{failure}");
        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_load_allowlist_refuses_a_copyleft_source() {
        let dir = scratch("allow");
        let path = dir.join("sources.toml");
        fs::write(
            &path,
            "[[source]]\nid = \"luna\"\nkind = \"upstream\"\nspdx = \"LGPL-3.0\"\n\
             license = \"LGPL-3.0\"\nurl = \"https://example.invalid\"\n\
             retrieved = \"2026-09-29\"\nsha256 = \"abc\"\npermissive = false\n\
             layer = \"L2\"\n",
        )
        .expect("writing the fixture");
        let failure = load_allowlist(&path).expect_err("a copyleft source must be refused");
        assert!(failure.to_string().contains("luna"), "{failure}");

        fs::write(
            &path,
            "[[source]]\nid = \"base\"\nkind = \"derived\"\nspdx = \"MIT OR Apache-2.0\"\n\
             license = \"Project-owned\"\nurl = \"https://example.invalid\"\npermissive = true\n\
             layer = \"L2\"\n",
        )
        .expect("writing the fixture");
        let allowlist = load_allowlist(&path).expect("a derived source needs no pin");
        assert_eq!(allowlist.source.len(), 1);
        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_load_allowlist_requires_a_pin_for_an_upstream_source() {
        let dir = scratch("pin");
        let path = dir.join("sources.toml");
        fs::write(
            &path,
            "[[source]]\nid = \"jieba-dict\"\nkind = \"upstream\"\nspdx = \"MIT\"\n\
             license = \"MIT\"\nurl = \"https://example.invalid\"\npermissive = true\n\
             layer = \"L4\"\n",
        )
        .expect("writing the fixture");
        let failure = load_allowlist(&path).expect_err("an unpinned upstream source must fail");
        assert!(failure.to_string().contains("sha256"), "{failure}");
        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_growth_percent_and_the_guard_boundary() {
        assert_eq!(growth_percent(0, 0), 0.0);
        assert_eq!(growth_percent(100, 100), 0.0);
        assert_eq!(growth_percent(115, 100), 15.0);
        assert_eq!(growth_percent(130, 100), 30.0);
        assert!(growth_percent(131, 100) > 30.0);
    }

    #[test]
    fn test_hex_and_human_size_render_for_the_log() {
        assert_eq!(hex(&[0x00, 0x0F, 0xFF]), "000fff");
        assert_eq!(human_size(512), "0.5KiB");
        assert_eq!(human_size(2 * 1024 * 1024), "2.00MiB");
    }
}
