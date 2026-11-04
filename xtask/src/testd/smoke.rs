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
//! The expectation column has one literal and five prefixed forms:
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
//! * `@top3:<a>|<b>|...` -- order-sensitive membership in the top three: each named
//!   text matches some candidate of the first three, at strictly ascending positions.
//!   This is the ranking form: it pins that `a` outranks `b` without pinning either as
//!   the first choice, so a re-order inside a band turns it red while an unrelated
//!   candidate joining the page does not.
//! * `@today:<fmt>` -- the top-1 candidate is exactly the run's clock rendered through
//!   the format. The clock is injected ([`clock::Clock`]): the CLI reads the wall
//!   clock, the tests pin a fixed instant, so the check is deterministic. A `@today`
//!   row must carry the `flags:quick` column, because the quick panel is the only
//!   surface that renders the current time.
//! * `@empty` -- the candidate list is empty. Only an association row can answer it:
//!   a decode is never empty (the pass-through contract), and a recognized quick
//!   trigger always renders.
//!
//! # The v2 columns and the flags matrix
//!
//! A v1 row is `case_id, keys, expect[, tags...]` and stays exactly that. A v2 row
//! adds one column after the expectation, recognized by its `flags:` prefix:
//! `case_id, keys, expect, flags:<names>[, tags...]`. The names are the switches the
//! row turns on -- the fuzzy class bits and the abbreviation, correction and English
//! switches, spelled exactly as the scenario `[decode]` section spells them, plus the
//! scenario switches `quick`, `assoc` (association on) and `assoc-off` (the shipped
//! default, driven through the same session script) -- and a row that names none
//! decodes as a v1 row. The matrix band uses the column to pin each opt-in
//! behaviour's switched-on answer beside the shipped default for the same keys, so a
//! switch that starts leaking into the default turns red on the next run.
//!
//! # Boundaries
//!
//! Headless and deterministic: the driver opens at most three files (the case set, the
//! dictionary, and -- only when a row needs it -- the English dictionary), consults no
//! environment and talks to no display. The one clock read is behind
//! [`clock::Clock`], and a run that names no `@today:` row never makes it. The
//! assertions are about the engine contract; a claim about the host or the compositor
//! belongs to the XTEST channel, and the two are never reported as each other.

mod cli;
mod clock;
mod derive;
mod v2;

#[cfg(test)]
mod tests;

pub use crate::testd::smoke::cli::{SmokeArgs, run};
pub use crate::testd::smoke::clock::Clock;
pub use crate::testd::smoke::derive::derive;

// Plain `use`, not `pub use`: these are what this file's own functions name, and
// re-exporting the internals was a surface nothing consumed.
use crate::testd::smoke::clock::{format_civil, validate_format};
use crate::testd::smoke::v2::RowFlags;

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use ime_core::quick::CivilTime;
use ime_core::viterbi::Decoder;
use ime_core::viterbi::{DecodeScratch, DecodeSources};
use ime_dict::fst_index::FstLexicon;
use ime_dict::fst_index::dict_lm::DictLm;
use ime_types::{DecodeFlags, DecodeRequest, Lexicon, UserFreqSource};

/// The case file `xtask testd-smoke` runs when `--cases` is not given.
pub const DEFAULT_CASES: &str = "tests/smoke/cases.tsv";

/// The compiled dictionary the cases decode against by default.
pub const DEFAULT_DICT: &str = "data/compiled/base.dict";

/// The compiled English dictionary the `ENGLISH` rows decode against by default.
///
/// Mounted only when a row carries the switch: a run without such rows never opens
/// the file, which keeps the default path's file set and its duration where they were.
pub const DEFAULT_ENGLISH: &str = "data/compiled/eng.dict";

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

/// One row of the case set.
#[derive(Clone, Debug)]
pub struct SmokeCase {
    /// The stable identifier a failure is reported by.
    pub id: String,
    /// The key sequence, raw and lower case; `'` forces a syllable boundary.
    ///
    /// An association row's column holds a script instead: pinyin chunks interleaved
    /// with `<commit>` and `<assoc-select-N>` markers, which [`v2::parse_script`]
    /// reads.
    pub keys: String,
    /// What the decode owes the user for these keys.
    pub expect: Expectation,
    /// The switches the row turns on; empty for every v1 row.
    pub flags: RowFlags,
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
    /// The first three candidates hold these texts, in this relative order.
    Top3(Vec<String>),
    /// The top-1 candidate is the run's clock rendered through this format.
    Today(String),
    /// The candidate list is empty.
    Empty,
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
    /// The whole first page the answer came from, slash-joined, for the verbose
    /// report; the marker for none when the answer was empty.
    pub page: String,
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

/// The compiled dictionaries mounted for a run, and the model built over the main one.
///
/// The lexicon owns its mapping and the model reads the lexicon's own sections, so the
/// two live and die together. The English lexicon is mounted only when a row asked for
/// it, and stays `None` -- meaning "never queried" -- otherwise.
pub struct Mounted {
    /// The decoder, built with the shipped configuration.
    pub(super) decoder: Decoder,
    /// The compiled dictionary, verified in full at mount time.
    pub(super) lexicon: FstLexicon,
    /// The unigram and bigram model of the same container.
    pub(super) lm: DictLm,
    /// The English lexicon, when a row's requests carry the switch.
    pub(super) english: Option<FstLexicon>,
}

/// The user-frequency source a run ranks with: a store that has never recorded a
/// commit, so every lookup answers zero and the user term drops out of every score.
///
/// The baseline pins what a fresh installation answers; a user word that outranked a
/// case's expectation on one machine and not on another would make the file undecidable.
pub(super) struct FreshUser;

impl UserFreqSource for FreshUser {
    fn freq(&self, _key: &str) -> u32 {
        0
    }

    fn record(&self, _key: &str, _weight_hint: u16) {}

    fn is_user_word(&self, _key: &str) -> bool {
        false
    }
}

/// Runs the case set against the mounted dictionary and answers the report.
///
/// This is the library entry the CLI prints: the run itself reads the files, mounts the
/// dictionary once and observes each case once.
///
/// # Errors
///
/// A case file that is missing, unreadable or malformed, a dictionary that cannot be
/// mounted, and an English dictionary that a row needs but that cannot be mounted, each
/// end in an error that names the file and the reason. A case set that parses to
/// nothing is refused rather than reported green: a baseline of zero cases pins
/// nothing.
///
/// # Panics
///
/// Never.
pub fn run_cases(
    cases: &Path,
    dict: &Path,
    english: Option<&Path>,
    clock: Clock,
) -> Result<SmokeReport> {
    let text = read_input(cases)?;
    let set = parse_cases(&text).with_context(|| format!("in {}", cases.display()))?;
    ensure!(
        !set.is_empty(),
        "smoke: {} holds no case; a baseline of zero pins nothing",
        cases.display()
    );
    let needs_english = set.iter().any(|case| case.flags.needs_english());
    let mounted = mount(dict, if needs_english { english } else { None })?;
    Ok(evaluate(&set, &mounted, clock))
}

/// Evaluates a parsed case set against a mounted dictionary.
///
/// # Panics
///
/// Never.
pub(crate) fn evaluate(set: &[SmokeCase], mounted: &Mounted, clock: Clock) -> SmokeReport {
    let mut report = SmokeReport {
        total: set.len(),
        passed: 0,
        failures: Vec::new(),
    };
    for case in set {
        // One civil reading per case: the quick panel's render and a `@today`
        // comparison must see the same instant, or a second boundary between two
        // reads would flake a row that is in fact green. A fixed clock answers the
        // same fields on every read; the system clock is the one this ordering
        // protects.
        let civil = clock.civil();
        let texts = observe(mounted, case, civil);
        // A `@today` form is resolved against that same reading before it is
        // compared, so the satisfaction check stays a plain text comparison.
        let satisfied = match &case.expect.kind {
            ExpectKind::Today(fmt) => {
                let rendered = format_civil(&civil, fmt);
                texts.first().is_some_and(|got| *got == rendered)
            }
            kind => satisfied(kind, &texts),
        };
        if satisfied {
            report.passed += 1;
        } else {
            report.failures.push(SmokeFailure {
                id: case.id.clone(),
                keys: case.keys.clone(),
                expected: case.expect.raw.clone(),
                got: observed_summary(&case.expect.kind, &texts),
                page: first_page(texts.as_slice()),
            });
        }
    }
    report
}

/// Collects the candidate texts one case's observation answers with.
///
/// The dispatch is the flags column's: a quick row reads the panel against the civil
/// reading the caller supplies, an association row drives the session script with the
/// feature in the state the column names, and every other row decodes the keys with
/// whatever switches the row named.
///
/// # Panics
///
/// Never.
fn observe(mounted: &Mounted, case: &SmokeCase, civil: CivilTime) -> Vec<String> {
    if case.flags.quick {
        return v2::quick_texts(&case.keys, civil);
    }
    if case.flags.assoc {
        // The script was validated at parse time, so the drive cannot meet an
        // unparseable column here.
        let Ok(script) = v2::parse_script(&case.keys) else {
            return Vec::new();
        };
        return v2::assoc_texts(mounted, &script, case.flags.assoc_enabled);
    }
    decode_texts(mounted, &case.keys, case.flags.decode)
}

/// Summarizes what the engine answered, for a failure's `got` column.
///
/// The exact-match forms name the top-1, which is the text the check read; the
/// membership forms name the whole first page, because the information a failure
/// needs is where the expected text went.
///
/// # Panics
///
/// Never.
fn observed_summary(kind: &ExpectKind, texts: &[String]) -> String {
    let wants_page = matches!(kind, ExpectKind::Top3(_) | ExpectKind::Empty);
    if !wants_page {
        return texts
            .first()
            .cloned()
            .unwrap_or_else(|| String::from("(no candidate)"));
    }
    first_page(texts)
}

/// Joins the first page's texts the way a failure report spells them.
///
/// # Panics
///
/// Never.
fn first_page(texts: &[String]) -> String {
    if texts.is_empty() {
        return String::from("(no candidate)");
    }
    texts
        .iter()
        .take(PAGE)
        .cloned()
        .collect::<Vec<_>>()
        .join("/")
}

/// Mounts the compiled dictionary -- and the English one, when a path is given -- and
/// the model built over the main container.
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
pub(crate) fn mount(dict: &Path, english: Option<&Path>) -> Result<Mounted> {
    let lexicon = FstLexicon::load(dict)
        .with_context(|| format!("cannot mount the smoke dictionary {}", dict.display()))?;
    let lm = DictLm::new(&lexicon);
    let english_lexicon = match english {
        Some(path) => Some(FstLexicon::load(path).with_context(|| {
            format!(
                "cannot mount the smoke English dictionary {}",
                path.display()
            )
        })?),
        None => None,
    };
    Ok(Mounted {
        decoder: Decoder::default(),
        lexicon,
        lm,
        english: english_lexicon,
    })
}

/// Decodes `keys` with `extra` switches over the defaults and answers the candidate
/// texts, in ranked order.
///
/// The decode goes through the shared workspace form, which is the same path
/// [`Decoder::decode`] takes, so a plain row's answer is bit-identical to a v1 run's.
///
/// # Panics
///
/// Never.
pub(crate) fn decode_texts(mounted: &Mounted, keys: &str, extra: DecodeFlags) -> Vec<String> {
    let user = FreshUser;
    let mut request = DecodeRequest::new(keys);
    // The row's switches are inserted over the defaults, the same union the scenario
    // harness applies, so a row cannot silently drop the default switches.
    request.flags.insert(extra);
    let english = mounted
        .english
        .as_ref()
        .map(|lexicon| lexicon as &dyn Lexicon);
    let mut scratch = DecodeScratch::new();
    mounted.decoder.decode_into(
        &mut scratch,
        &request,
        DecodeSources {
            lexicon: &mounted.lexicon,
            user_freq: &user,
            lm: &mounted.lm,
            english,
            hints: None,
        },
    );
    scratch
        .into_result()
        .candidates
        .into_iter()
        .map(|c| c.text)
        .collect()
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
        ExpectKind::Top3(items) => top3_satisfied(items, texts),
        // Resolved against the clock by the caller; a `Today` reaching here means the
        // caller wanted the plain form, and the honest answer is the comparison's.
        ExpectKind::Today(_) => false,
        ExpectKind::Empty => texts.is_empty(),
    }
}

/// Answers whether the first page's first three candidates hold `items` in order.
///
/// A greedy left-to-right scan: each item is consumed by the first candidate at or
/// after the previous match's position, which is exactly the "in this relative order"
/// reading of the form -- equal-text candidates never both match one item, and a
/// later item cannot be matched by an earlier position.
///
/// # Panics
///
/// Never.
fn top3_satisfied(items: &[String], texts: &[String]) -> bool {
    let window = texts.iter().take(3);
    let mut cursor = 0usize;
    'items: for item in items {
        for (offset, candidate) in window.clone().enumerate().skip(cursor) {
            if candidate == item {
                cursor = offset + 1;
                continue 'items;
            }
        }
        return false;
    }
    true
}

/// Parses the case set's text into cases.
///
/// `#` lines and blank lines are skipped. Every other line is at least three
/// tab-separated columns -- identifier, key sequence, expectation -- with an optional
/// fourth `flags:` column naming the row's switches, and any further column read as a
/// band tag.
///
/// # Errors
///
/// A row with too few columns, an empty identifier or key sequence, a key sequence that
/// is not lower-case pinyin over the apostrophe, an expectation that names no known
/// form, a flags column that names an undrivable switch or an impossible combination, a
/// pattern or `@today` format that does not compile, or an identifier another row
/// already holds -- each refuses the whole set with the line number, because a set that
/// half-parses would report on half the behaviours it claims to pin.
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
        // The column after the expectation is the v2 flags column when it carries the
        // prefix, and the first tag column when it does not, which is what keeps every
        // v1 row reading exactly as it always did.
        let rest: Vec<&str> = columns.collect();
        let (flags, tag_columns) = match rest.split_first() {
            Some((first, tail)) if first.starts_with("flags:") => (v2::parse_flags(first), tail),
            _ => (Ok(RowFlags::default()), rest.as_slice()),
        };
        let flags = flags.with_context(|| format!("smoke: line {number} (case {id})"))?;
        // The cohesion rules hold for the plain rows too: a `@today` row with no
        // flags column is exactly the row that can never answer, so the check is not
        // reserved for rows that name a switch.
        v2::check_row_cohesion(
            &flags,
            keys,
            expect_column.starts_with("@today:"),
            expect_column == "@empty",
        )
        .with_context(|| format!("smoke: line {number} (case {id})"))?;
        let expect = expectation(expect_column)
            .with_context(|| format!("smoke: line {number} (case {id})"))?;
        ensure_keys_for(keys, &flags)
            .with_context(|| format!("smoke: line {number} (case {id})"))?;
        // Every column past the flags column is tag text, and the tags inside one
        // column are whitespace-separated, so `ambiguity  extra` reads as two.
        let tags: Vec<String> = tag_columns
            .iter()
            .flat_map(|column| column.split_whitespace())
            .map(str::to_owned)
            .collect();
        cases.push(SmokeCase {
            id: id.to_owned(),
            keys: keys.to_owned(),
            expect,
            flags,
            tags,
        });
    }
    Ok(cases)
}

/// Validates one row's keys column against what its flags turn on.
///
/// # Errors
///
/// A quick row's trigger and a plain row's keys must be lower-case pinyin spellings; an
/// association row's column holds a script instead and is checked by parsing it here,
/// so a malformed marker is a load-time refusal rather than a run-time empty answer
/// that an `@empty` row would silently pass on.
///
/// # Panics
///
/// Never.
fn ensure_keys_for(keys: &str, flags: &RowFlags) -> Result<()> {
    if flags.assoc {
        return v2::parse_script(keys).map(|_| ());
    }
    ensure_keys(keys)
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
pub(crate) fn ensure_keys(keys: &str) -> Result<()> {
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
/// pattern, an empty expectation of any form is refused outright, and a `@top3:` list
/// with an empty member is refused because an empty member would match nothing and
/// turn the row red for a reason the file did not mean.
///
/// # Panics
///
/// Never.
pub fn expectation(column: &str) -> Result<Expectation> {
    ensure!(!column.is_empty(), "the expectation column is empty");
    let raw = column.to_owned();
    if column == "@empty" {
        return Ok(Expectation {
            kind: ExpectKind::Empty,
            raw,
        });
    }
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
        "@top3" => ExpectKind::Top3(top3_items(value)?),
        "@today" => {
            ensure!(!value.is_empty(), "@today names no format");
            validate_format(value)?;
            ExpectKind::Today(value.to_owned())
        }
        _ => bail!(
            "unknown expectation form {prefix:?}; the forms are @first:, @contains:, @regex:, @top3:, @today: and @empty"
        ),
    };
    Ok(Expectation { kind, raw })
}

/// Splits a `@top3:` value into its ordered items.
///
/// # Errors
///
/// A value holding an empty item -- a leading, trailing or doubled `|` -- is refused,
/// because an empty item can never match a candidate and would keep the row red for a
/// reason the file did not state.
///
/// # Panics
///
/// Never.
fn top3_items(value: &str) -> Result<Vec<String>> {
    let items: Vec<String> = value.split('|').map(str::to_owned).collect();
    ensure!(
        items.iter().all(|item| !item.is_empty()),
        "the @top3 list {value:?} holds an empty item"
    );
    Ok(items)
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
        "the expectation {column:?} starts a form but names none; the forms are @first:, @contains:, @regex:, @top3:, @today: and @empty"
    );
    Ok(())
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
