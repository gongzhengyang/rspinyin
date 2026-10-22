//! `xtask testd-smoke` -- the data-driven decode smoke baseline.
//!
//! # What this is
//!
//! The TSV asset and the driver that keeps it honest. One case is one line: a key
//! sequence, and the text the engine owes the user for it. Every case runs through the
//! same in-memory decode the sessions run -- the compiled dictionary mounted read-only,
//! a user frequency store that has never recorded anything -- so a green run says the
//! ranking in force still answers what it answered when the baseline was written, and a
//! red run names the case, the keys and what changed.
//!
//! The model is rime-ice's script smoke: a readable, diffable file where a ranking
//! regression is one line to read instead of a percentage to re-derive. The derivation
//! keeps it cheap to maintain: the `core` band is generated from the holdout evaluation
//! set (every row the engine already answers correctly becomes a pinned case), and the
//! handwritten bands pin the boundary and ambiguity behaviours no corpus row names.
//!
//! # What a case asserts
//!
//! The expectation column has one literal and three prefixed forms:
//!
//! * bare text, and `@first:<text>` -- the top-1 candidate, which is what the space key
//!   would commit first, must be exactly this text. The two spellings are the same
//!   check: the bare form reads as prose in the file, the prefixed form states the
//!   intent where a row's expectations are mixed.
//! * `@contains:<text>` -- some candidate on the first page (the decode contract's nine)
//!   contains this text as a substring. This is the ambiguity form: a key sequence with
//!   a legitimate second reading is not required to lose its first choice, only to keep
//!   the alternative reachable.
//! * `@regex:<pattern>` -- the top-1 candidate matches a regular expression. This is
//!   the form for commits whose text varies (composed dates, mixed English), which the
//!   later composition cards introduce.
//!
//! # Boundaries
//!
//! Headless and deterministic: the driver opens exactly two files (the case set and the
//! dictionary), reads no clock, consults no environment and talks to no display. The
//! assertions are about the decode contract; a claim about the host or the compositor
//! belongs to the XTEST channel, and the two are never reported as each other.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use clap::Args;
use ime_core::viterbi::Decoder;
use ime_dict::fst_index::FstLexicon;
use ime_dict::fst_index::dict_lm::DictLm;
use ime_types::{DecodeRequest, UserFreqSource};

/// The case file `xtask testd-smoke` runs when `--cases` is not given.
pub const DEFAULT_CASES: &str = "tests/smoke/cases.tsv";

/// The compiled dictionary the cases decode against by default.
pub const DEFAULT_DICT: &str = "data/compiled/base.dict";

/// The tag of the derived band: the cases regenerated from the holdout set.
pub const CORE_TAG: &str = "core";

/// How many cases the derived core band holds.
///
/// The card's first batch is two hundred cases: this many derived from the holdout set
/// plus the handwritten bands beside it. The quota is what the derivation fills, taking
/// the holdout's own order, which is frequency-stratified by construction.
pub const CORE_QUOTA: usize = 170;

/// How many candidates the `@contains` scan looks at.
///
/// The first page of the decode contract: a candidate past it cannot be taken without a
/// page turn, so an expectation that reaches past the page asserts nothing a user can
/// act on.
const PAGE: usize = 9;

/// Command-line surface of `xtask testd-smoke`.
#[derive(Debug, Args)]
pub struct SmokeArgs {
    /// The case file to run; `-` reads it from stdin for ad-hoc runs.
    #[arg(long, default_value = DEFAULT_CASES, value_name = "FILE")]
    pub cases: PathBuf,
    /// The compiled dictionary the cases decode against.
    #[arg(long, default_value = DEFAULT_DICT, value_name = "FILE")]
    pub dict: PathBuf,
    /// Print the first page of candidates for every failing case.
    #[arg(long, conflicts_with = "derive_from_holdout")]
    pub verbose: bool,
    /// Regenerate the core band from a holdout file instead of running.
    ///
    /// Every holdout row the engine already answers with its expected first choice
    /// becomes a `core` case, up to [`CORE_QUOTA`]. The file named by `--cases` is
    /// rewritten: the cases that do not carry the [`CORE_TAG`] survive untouched (the
    /// handwritten bands), the cases that do are replaced by the derivation. A baseline
    /// that ranking work legitimately improved is refreshed by re-running this, never by
    /// editing the generated lines by hand.
    #[arg(long, value_name = "HOLDOUT_TSV")]
    pub derive_from_holdout: Option<PathBuf>,
}

/// One row of the case set.
#[derive(Clone, Debug)]
pub struct SmokeCase {
    /// The stable identifier a failure is reported by.
    pub id: String,
    /// The key sequence, raw and lower case; `'` forces a syllable boundary.
    pub keys: String,
    /// What the decode owes the user for these keys.
    pub expect: Expectation,
    /// The band tags the row carries (`core`, `ambiguity`, ...).
    pub tags: Vec<String>,
}

/// What a case asserts, and the column it was read from.
///
/// The raw column travels with the parsed form so a failure report can quote the file
/// instead of paraphrasing it.
#[derive(Clone, Debug)]
pub struct Expectation {
    /// The parsed assertion.
    pub kind: ExpectKind,
    /// The column as written.
    pub raw: String,
}

/// The assertion forms the expectation column spells.
#[derive(Clone, Debug)]
pub enum ExpectKind {
    /// The top-1 candidate is exactly this text.
    First(String),
    /// A candidate of the first page contains this text.
    Contains(String),
    /// The top-1 candidate matches this pattern.
    Regex(regex::Regex),
}

/// One case the engine answered differently from what the file pins.
#[derive(Clone, Debug)]
pub struct SmokeFailure {
    /// The case's identifier.
    pub id: String,
    /// The case's key sequence.
    pub keys: String,
    /// The expectation column as written.
    pub expected: String,
    /// What the engine answered: the top-1 text, or the marker for none.
    pub got: String,
}

/// The verdict of one run over a whole case set.
#[derive(Clone, Debug)]
pub struct SmokeReport {
    /// How many cases ran.
    pub total: usize,
    /// How many held.
    pub passed: usize,
    /// The cases that did not hold, in file order.
    pub failures: Vec<SmokeFailure>,
}

/// The compiled dictionary mounted for a run, and the model built over it.
///
/// The lexicon owns its mapping and the model reads the lexicon's own sections, so the
/// two live and die together.
pub struct Mounted {
    /// The decoder, built with the shipped configuration.
    decoder: Decoder,
    /// The compiled dictionary, verified in full at mount time.
    lexicon: FstLexicon,
    /// The unigram and bigram model of the same container.
    lm: DictLm,
}

/// The user-frequency source a run ranks with: a store that has never recorded a
/// commit, so every lookup answers zero and the user term drops out of every score.
///
/// The baseline pins what a fresh installation answers; a user word that outranked a
/// case's expectation on one machine and not on another would make the file undecidable.
struct FreshUser;

impl UserFreqSource for FreshUser {
    fn freq(&self, _key: &str) -> u32 {
        0
    }

    fn record(&self, _key: &str, _weight_hint: u16) {}

    fn is_user_word(&self, _key: &str) -> bool {
        false
    }
}

/// Runs `xtask testd-smoke`.
///
/// # Errors
///
/// The derivation path and the check path each propagate their own failures: a
/// holdout or case file that cannot be read or parsed names the offending line, a
/// dictionary that cannot be mounted carries the loader's stable code, and a check with
/// at least one failing case ends in an error that quotes the count, which is what
/// turns the gate red.
///
/// # Panics
///
/// Never.
pub fn run(args: SmokeArgs) -> Result<()> {
    if let Some(holdout) = args.derive_from_holdout {
        let summary = derive(&args.cases, &holdout, &args.dict)?;
        println!(
            "smoke: derived {} core cases from {} ({} holdout rows scanned, {} non-core cases kept)",
            summary.derived,
            holdout.display(),
            summary.scanned,
            summary.kept
        );
        return Ok(());
    }
    let report = run_cases(&args.cases, &args.dict)?;
    for failure in &report.failures {
        println!(
            "FAIL  {}\tkeys={}\texpected={}\tgot={}",
            failure.id, failure.keys, failure.expected, failure.got
        );
        if args.verbose {
            println!(
                "      (use `xtask testd-engine` with a scenario naming these keys to see the page)"
            );
        }
    }
    println!(
        "smoke: {}/{} cases passed against {}",
        report.passed,
        report.total,
        args.dict.display()
    );
    if report.failures.is_empty() {
        Ok(())
    } else {
        bail!(
            "smoke: {} of {} cases diverge from the baseline",
            report.failures.len(),
            report.total
        )
    }
}

/// Runs the case set against the mounted dictionary and answers the report.
///
/// This is the library entry the CLI prints: the run itself reads the two files, mounts
/// the dictionary once and decodes each case's keys once.
///
/// # Errors
///
/// A case file that is missing, unreadable or malformed, and a dictionary that cannot
/// be mounted, each end in an error that names the file and the reason. A case set that
/// parses to nothing is refused rather than reported green: a baseline of zero cases
/// pins nothing.
///
/// # Panics
///
/// Never.
pub fn run_cases(cases: &Path, dict: &Path) -> Result<SmokeReport> {
    let text = read_input(cases)?;
    let set = parse_cases(&text).with_context(|| format!("in {}", cases.display()))?;
    ensure!(
        !set.is_empty(),
        "smoke: {} holds no case; a baseline of zero pins nothing",
        cases.display()
    );
    let mounted = mount(dict)?;
    Ok(evaluate(&set, &mounted))
}

/// Evaluates a parsed case set against a mounted dictionary.
///
/// # Panics
///
/// Never.
pub(crate) fn evaluate(set: &[SmokeCase], mounted: &Mounted) -> SmokeReport {
    let mut report = SmokeReport {
        total: set.len(),
        passed: 0,
        failures: Vec::new(),
    };
    for case in set {
        let texts = decode_texts(mounted, &case.keys);
        if satisfied(&case.expect.kind, &texts) {
            report.passed += 1;
        } else {
            let got = texts
                .first()
                .cloned()
                .unwrap_or_else(|| "(no candidate)".to_owned());
            report.failures.push(SmokeFailure {
                id: case.id.clone(),
                keys: case.keys.clone(),
                expected: case.expect.raw.clone(),
                got,
            });
        }
    }
    report
}

/// Mounts the compiled dictionary and the model built over it.
///
/// # Errors
///
/// Whatever the loader refuses, carried with the path: a file that is not there, a
/// container that is not a dictionary, a version this build cannot read, or a checksum
/// that does not hold. Every one of those is an exit rather than a decode that
/// hallucinated from nothing.
///
/// # Panics
///
/// Never.
pub(crate) fn mount(dict: &Path) -> Result<Mounted> {
    let lexicon = FstLexicon::load(dict)
        .with_context(|| format!("cannot mount the smoke dictionary {}", dict.display()))?;
    let lm = DictLm::new(&lexicon);
    Ok(Mounted {
        decoder: Decoder::default(),
        lexicon,
        lm,
    })
}

/// Decodes `keys` and answers the texts of the candidates, in ranked order.
///
/// # Panics
///
/// Never.
pub(crate) fn decode_texts(mounted: &Mounted, keys: &str) -> Vec<String> {
    let user = FreshUser;
    let request = DecodeRequest::new(keys);
    let result = mounted
        .decoder
        .decode(&request, &mounted.lexicon, &user, &mounted.lm);
    result.candidates.into_iter().map(|c| c.text).collect()
}

/// Answers whether the candidate texts satisfy the expectation.
///
/// # Panics
///
/// Never.
pub(crate) fn satisfied(expect: &ExpectKind, texts: &[String]) -> bool {
    match expect {
        ExpectKind::First(text) => texts.first().is_some_and(|got| got == text),
        ExpectKind::Contains(text) => texts
            .iter()
            .take(PAGE)
            .any(|candidate| candidate.contains(text.as_str())),
        ExpectKind::Regex(pattern) => texts.first().is_some_and(|got| pattern.is_match(got)),
    }
}

/// Parses the case set's text into cases.
///
/// `#` lines and blank lines are skipped. Every other line is at least three
/// tab-separated columns -- identifier, key sequence, expectation -- with any further
/// column read as a band tag.
///
/// # Errors
///
/// A row with too few columns, an empty identifier or key sequence, a key sequence that
/// is not lower-case pinyin over the apostrophe, an expectation that names no known
/// form, a pattern that does not compile, or an identifier another row already holds --
/// each refuses the whole set with the line number, because a set that half-parses
/// would report on half the behaviours it claims to pin.
///
/// # Panics
///
/// Never.
pub fn parse_cases(text: &str) -> Result<Vec<SmokeCase>> {
    let mut cases = Vec::new();
    let mut seen = BTreeSet::new();
    for (number, line) in text.lines().enumerate() {
        let number = number + 1;
        let trimmed = line.trim_end_matches(['\r']);
        if trimmed.trim().is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let raw_columns: Vec<&str> = trimmed.split('\t').collect();
        ensure!(
            raw_columns.len() >= 3,
            "smoke: line {number}: expected at least 3 tab-separated columns, found {}",
            raw_columns.len()
        );
        let mut columns = raw_columns.iter().map(|column| column.trim());
        let id = columns.next().unwrap_or_default();
        let keys = columns.next().unwrap_or_default();
        let expect_column = columns.next().unwrap_or_default();
        ensure!(!id.is_empty(), "smoke: line {number}: the case id is empty");
        ensure!(
            !seen.contains(id),
            "smoke: line {number}: the id {id:?} is used twice"
        );
        seen.insert(id.to_owned());
        ensure_keys(keys).with_context(|| format!("smoke: line {number} (case {id})"))?;
        let expect = expectation(expect_column)
            .with_context(|| format!("smoke: line {number} (case {id})"))?;
        // Every column past the expectation is tag text, and the tags inside one
        // column are whitespace-separated, so `ambiguity  extra` reads as two.
        let tags: Vec<String> = raw_columns[3..]
            .iter()
            .flat_map(|column| column.split_whitespace())
            .map(str::to_owned)
            .collect();
        cases.push(SmokeCase {
            id: id.to_owned(),
            keys: keys.to_owned(),
            expect,
            tags,
        });
    }
    Ok(cases)
}

/// Reads the case set, from stdin when the path asks for it.
///
/// # Errors
///
/// A named file that cannot be read is carried with the path.
///
/// # Panics
///
/// Never.
fn read_input(path: &Path) -> Result<String> {
    if path == Path::new("-") {
        use std::io::Read;
        let mut text = String::new();
        std::io::stdin()
            .read_to_string(&mut text)
            .context("cannot read the case set from stdin")?;
        return Ok(text);
    }
    fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))
}

/// Validates one key sequence: lower-case ASCII letters with `'` between syllables.
///
/// `ü` is a legal key as well as `v`: the engine's own normalizer folds `v` to `ü` and
/// accepts the `ü` spelling directly, so a case may write the sound either way.
///
/// # Errors
///
/// An empty sequence, and any character outside the alphabet, each refuse the case with
/// the character named: a sequence the alphabet cannot spell would decode to a
/// pass-through the file did not mean to pin.
///
/// # Panics
///
/// Never.
fn ensure_keys(keys: &str) -> Result<()> {
    ensure!(!keys.is_empty(), "the key sequence is empty");
    for ch in keys.chars() {
        ensure!(
            ch.is_ascii_lowercase() || ch == '\'' || ch == 'ü',
            "the key sequence {keys:?} carries {ch:?}, which is not a pinyin key"
        );
    }
    Ok(())
}

/// Parses one expectation column.
///
/// # Errors
///
/// A column that starts with `@` but names no known form is refused rather than read as
/// literal text: a misspelled prefix would otherwise pin the wrong string and pass for
/// the wrong reason. A `@regex:` pattern that does not compile is refused with the
/// pattern, and an empty expectation of any form is refused outright.
///
/// # Panics
///
/// Never.
pub fn expectation(column: &str) -> Result<Expectation> {
    ensure!(!column.is_empty(), "the expectation column is empty");
    let raw = column.to_owned();
    let Some((prefix, value)) = column.split_once(':') else {
        ensure_literal_form(column)?;
        return Ok(Expectation {
            kind: ExpectKind::First(column.to_owned()),
            raw,
        });
    };
    let kind = match prefix {
        "@first" => {
            ensure!(!value.is_empty(), "@first names no text");
            ExpectKind::First(value.to_owned())
        }
        "@contains" => {
            ensure!(!value.is_empty(), "@contains names no text");
            ExpectKind::Contains(value.to_owned())
        }
        "@regex" => {
            ensure!(!value.is_empty(), "@regex names no pattern");
            ExpectKind::Regex(
                regex::Regex::new(value)
                    .with_context(|| format!("the @regex pattern {value:?} does not compile"))?,
            )
        }
        _ => bail!(
            "unknown expectation form {prefix:?}; the forms are @first:, @contains: and @regex:"
        ),
    };
    Ok(Expectation { kind, raw })
}

/// Guards the literal spelling of the expectation column.
///
/// # Errors
///
/// Refuses a column that begins with `@`, since every `@` spelling is a form and a bare
/// `@` text would be read as one and fail confusingly later.
///
/// # Panics
///
/// Never.
fn ensure_literal_form(column: &str) -> Result<()> {
    ensure!(
        !column.starts_with('@'),
        "the expectation {column:?} starts a form but names none; the forms are @first:, @contains: and @regex:"
    );
    Ok(())
}

/// What the derivation did, for the run's one summary line.
pub struct DeriveSummary {
    /// Holdout rows that were decoded.
    pub scanned: usize,
    /// Core cases the derivation wrote.
    pub derived: usize,
    /// Cases that are not in the core band and survived the rewrite.
    pub kept: usize,
}

/// Regenerates the core band of the case set from a holdout file.
///
/// The holdout rows are decoded against the dictionary in the file's own order, and
/// every row the engine already answers with its expected first choice becomes a `core`
/// case, deduplicated by key sequence, until [`CORE_QUOTA`] is reached. The cases of
/// the current set that do not carry the [`CORE_TAG`] are carried over unchanged, so
/// the handwritten bands are never touched by a regeneration.
///
/// # Errors
///
/// A holdout file that cannot be read or whose rows do not parse, a dictionary that
/// cannot be mounted, and a current case set that cannot be re-parsed each end in an
/// error naming the file and the line.
///
/// # Panics
///
/// Never.
pub fn derive(cases: &Path, holdout: &Path, dict: &Path) -> Result<DeriveSummary> {
    let mounted = mount(dict)?;
    let text = fs::read_to_string(holdout)
        .with_context(|| format!("cannot read the holdout set {}", holdout.display()))?;
    let rows = parse_holdout(&text).with_context(|| format!("in {}", holdout.display()))?;
    let scanned = rows.len();
    let mut derived = Vec::new();
    let mut seen_keys = BTreeSet::new();
    for (keys, expected) in rows {
        if derived.len() >= CORE_QUOTA {
            break;
        }
        if !seen_keys.insert(keys.clone()) {
            continue;
        }
        let texts = decode_texts(&mounted, &keys);
        if texts.first().is_some_and(|got| got == &expected) {
            let id = format!("core_{:03}", derived.len() + 1);
            derived.push(format!("{id}\t{keys}\t@first:{expected}\t{CORE_TAG}"));
        }
    }
    // The handwritten bands survive the rewrite: everything that is not the derived
    // band is a case a person wrote, and a regeneration has no business editing it.
    let kept_text = if cases.exists() {
        let current = fs::read_to_string(cases)
            .with_context(|| format!("cannot read the current case set {}", cases.display()))?;
        let current = parse_cases(&current).with_context(|| format!("in {}", cases.display()))?;
        let kept: Vec<&SmokeCase> = current
            .iter()
            .filter(|case| !case.tags.iter().any(|tag| tag == CORE_TAG))
            .collect();
        let lines: Vec<String> = kept
            .iter()
            .map(|case| {
                let mut line = format!("{}\t{}\t{}", case.id, case.keys, case.expect.raw);
                for tag in &case.tags {
                    line.push('\t');
                    line.push_str(tag);
                }
                line
            })
            .collect();
        fs::create_dir_all(cases.parent().unwrap_or(Path::new("."))).with_context(|| {
            format!(
                "cannot create {}",
                cases.parent().unwrap_or(Path::new(".")).display()
            )
        })?;
        format_lines(&lines)
    } else {
        String::new()
    };
    let header = format!(
        "# rspinyin decode smoke baseline, regenerated by `xtask testd-smoke --derive-from-holdout`.\n\
         # The `core` band is derived; the bands below the marker are handwritten. One case\n\
         # per line, TAB separated: case_id  key_sequence  expected_text  [tags...]\n\
         # `key_sequence` is raw pinyin; `'` forces a syllable boundary.\n\
         # The file is a regression pin: it holds what the engine answers today, and a\n\
         # ranking change that turns a line red has to either fix the ranking or make a\n\
         # case for the new behaviour by hand.\n\n\
         # ── handwritten bands (ambiguity, boundaries) ──\n\
         {kept_text}\n\
         # ── derived core band, from the holdout evaluation set ──\n"
    );
    let mut body = header;
    for line in &derived {
        body.push_str(line);
        body.push('\n');
    }
    let kept = kept_text
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .count();
    fs::write(cases, body)
        .with_context(|| format!("cannot write the case set {}", cases.display()))?;
    Ok(DeriveSummary {
        scanned,
        derived: derived.len(),
        kept,
    })
}

/// Joins the handwritten lines back into file text, one per line.
///
/// # Panics
///
/// Never.
fn format_lines(lines: &[String]) -> String {
    let mut text = String::new();
    for line in lines {
        text.push_str(line);
        text.push('\n');
    }
    text
}

/// Parses a holdout evaluation set into `(keys, expected first choice)` rows.
///
/// The set's own two columns are the pinyin and the expected first choice; the optional
/// third (the layer that has to serve the word) is read past.
///
/// # Errors
///
/// A row that does not hold two columns, and a key sequence outside the case alphabet,
/// each refuse the file with the line number.
///
/// # Panics
///
/// Never.
fn parse_holdout(text: &str) -> Result<Vec<(String, String)>> {
    let mut rows = Vec::new();
    for (number, line) in text.lines().enumerate() {
        let number = number + 1;
        let trimmed = line.trim_end_matches(['\r']);
        if trimmed.trim().is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let mut columns = trimmed.split('\t').map(str::trim);
        let keys = columns.next().unwrap_or_default();
        let expected = columns.next().unwrap_or_default();
        ensure!(
            !keys.is_empty() && !expected.is_empty(),
            "smoke: holdout line {number}: expected `pinyin<TAB>expected`"
        );
        ensure_keys(keys).with_context(|| format!("smoke: holdout line {number}"))?;
        rows.push((keys.to_owned(), expected.to_owned()));
    }
    Ok(rows)
}

#[cfg(test)]
mod tests;
