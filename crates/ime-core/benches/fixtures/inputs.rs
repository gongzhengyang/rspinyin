//! The inputs the decode benchmark cases run on, and the dictionary keys they spell.
//!
//! Responsibility: hold the case table -- the five inputs whose cost the decode
//! budget is stated for -- and turn each of them into the `'`-separated keys a
//! decoder looks up while decoding it, plus the pinyin keys of the held-out
//! evaluation set, so that the generated corpus carries a word wherever a decode
//! reaches.
//!
//! The table is also checked against the inputs it names before a run measures
//! anything: a case whose input does not spell the number of syllables its name
//! claims would measure a shape the budget was not stated for, which reads as a
//! comfortable margin rather than as a broken fixture.
//!
//! Boundaries: this module knows nothing about criterion and nothing about how the
//! corpus is generated. It reads the case table and the held-out fixture and hands
//! out strings.

use ime_core::segment::{HINT_INLINE_BOUNDARIES, MAX_RAW_LEN, SyllableDag};
use smallvec::SmallVec;

/// Longest window of syllables the corpus guarantees a word for.
///
/// Four covers the words a decoder actually finds -- the two- and three-syllable
/// words and the four-character idioms -- without making the corpus carry a word
/// for every span nobody ever looks up.
pub const MAX_SEED_SYLLABLES: usize = 4;

/// The held-out evaluation set, embedded so that the benchmark reads no file.
///
/// The set is the one the language-model work maintains under `tests/fixtures/`;
/// the benchmark reuses it rather than inventing a second corpus, and the path is
/// resolved against this file so it does not depend on the working directory.
const HOLDOUT: &str = include_str!("../../tests/fixtures/lm_holdout.tsv");

/// One benchmark case: the name criterion publishes it under and the input it decodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Case {
    /// Last segment of the criterion id: the case is `decode/<name>`.
    pub name: &'static str,
    /// The raw input the case decodes.
    pub raw: &'static str,
    /// Syllables [`Case::raw`] spells, which is the size the case is stated for.
    ///
    /// Declared rather than counted from the input at run time, so that the check in
    /// [`seed_keys`] compares the cut against a number the case states: an input that
    /// changed under its case would then be a failure instead of a size that silently
    /// re-declared itself.
    pub syllables: usize,
}

/// The cases of the `decode` group.
///
/// The first four are the syllable counts the decode budget is stated for
/// (`BUDGET-LAT-02`: raw of at most twelve syllables), and the last is the hard
/// limit the input buffer accepts, which is the worst input the engine can be
/// handed at all.
///
/// The four inputs are the ones the design names, extended where the design's
/// label and its input disagreed: `jintiantianqizhenbucuo` spells seven syllables
/// by its longest-match cut and `womenmingtianqugongyuanwanba` spells nine, so the
/// eight-syllable case carries one more word and the twelve-syllable case three. A
/// case named `12syl` that decodes nine syllables would understate the budget it
/// exists to guard.
///
/// Every case's declared size is checked against its own cut before the benchmark
/// measures anything; see [`check_size`] for why the check is not a test.
pub const CASES: &[Case] = &[
    Case {
        name: "2syl",
        raw: "nihao",
        syllables: 2,
    },
    Case {
        name: "4syl",
        raw: "zhongguoxiangqi",
        syllables: 4,
    },
    Case {
        name: "8syl",
        raw: "jintiantianqizhenbucuohao",
        syllables: 8,
    },
    Case {
        name: "12syl",
        raw: "womenmingtianqugongyuanwanbazhongguoxue",
        syllables: 12,
    },
    Case {
        name: "64byte",
        raw: MAX_LENGTH_INPUT,
        syllables: MAX_LENGTH_SYLLABLES,
    },
];

/// The longest input the buffer accepts: twelve `nihao` and one `niha`.
///
/// Spelled out rather than built at run time so that the length is settled when the
/// benchmark is compiled; the assertion below fails the build if it drifts.
pub const MAX_LENGTH_INPUT: &str =
    "nihaonihaonihaonihaonihaonihaonihaonihaonihaonihaonihaonihaoniha";

/// Syllables [`MAX_LENGTH_INPUT`] spells: twelve `nihao` and one `niha`, two each.
///
/// The case is named for its byte length rather than for a syllable count, so this
/// number is not written in its name; it is declared all the same, because the size
/// the case is measured at has to come from the case table and not from whatever the
/// input happens to cut into.
pub const MAX_LENGTH_SYLLABLES: usize = 26;

// The length is the whole point of the case, so it is checked by the compiler
// rather than by the run: a const block that panics stops the build.
const _: () = assert!(MAX_LENGTH_INPUT.len() == MAX_RAW_LEN);

/// The keys the corpus must carry a word for, one per benchmark input span.
///
/// Every input is cut by the segmentation layer's own default hint and every
/// contiguous window of up to [`MAX_SEED_SYLLABLES`] syllables of that cut becomes
/// a key. Deriving them rather than listing them keeps the seeds from drifting away
/// from the case table: an input that changes changes its keys with it.
///
/// Every cut is also checked against the size its case declares ([`check_size`]), so
/// the first thing a benchmark run does is refuse a case table whose inputs are not
/// the shapes the table says they are.
///
/// # Errors
/// Returns a description of the first input that cannot be cut into syllables, and
/// of the first whose cut is not the size its case claims. Neither can happen for
/// the table above, and both are reported rather than assumed because an input that
/// quietly contributed no keys, or a different number of syllables than its name
/// claims, would leave the benchmark measuring a lattice the corpus does not
/// populate or a shape the budget was not stated for -- which reads as a fast decode
/// rather than as a broken fixture.
pub fn seed_keys() -> Result<Vec<String>, String> {
    let mut keys = Vec::new();
    for case in CASES {
        let syllables = cut(case.raw)
            .map_err(|reason| format!("case {} ({}): {reason}", case.name, case.raw))?;
        check_size(case, &syllables)?;
        for start in 0..syllables.len() {
            for length in 1..=MAX_SEED_SYLLABLES {
                let Some(window) = syllables.get(start..start + length) else {
                    break;
                };
                keys.push(window.join("'"));
            }
        }
    }
    keys.sort();
    keys.dedup();
    Ok(keys)
}

/// Checks that a case's input is the shape its name and its declared size say it is.
///
/// The check runs here rather than in a `#[cfg(test)]` module beside this file because
/// a benchmark target is built with `harness = false`: libtest never drives it, so a
/// test written there would never execute and would be invisible to
/// `cargo clippy --all-targets` as well. Every benchmark run performs the check
/// instead, before it measures anything, and a case table that disagrees with its
/// inputs stops the run rather than producing a number for a shape the budget was not
/// stated for.
///
/// # Errors
/// Returns a description of the first case whose cut is not the syllable count it
/// declares, and of the first whose name claims a count its declaration disagrees with.
fn check_size(case: &Case, syllables: &[String]) -> Result<(), String> {
    let spelled = syllables.len();
    if spelled != case.syllables {
        return Err(format!(
            "case {} declares {} syllables and {} spells {spelled}",
            case.name, case.syllables, case.raw
        ));
    }
    match claimed_syllables(case.name) {
        Some(claimed) if claimed != case.syllables => Err(format!(
            "case {} is named for {claimed} syllables and declares {}",
            case.name, case.syllables
        )),
        _ => Ok(()),
    }
}

/// The syllable count a case's name claims, or `None` when the name claims a size in
/// another unit.
///
/// The name is the half of a case that reaches the budget document -- the criterion id
/// and the threshold binding are both built from it -- so a name and a declaration that
/// disagree would leave the budget asserting a size nothing measures. The byte-capped
/// case is named for its length instead, which the assertion beside
/// [`MAX_LENGTH_INPUT`] pins at compile time.
fn claimed_syllables(name: &str) -> Option<usize> {
    name.strip_suffix("syl")?.parse::<usize>().ok()
}

/// The pinyin keys of the held-out evaluation set, in file order.
///
/// The set is decoded as one pass by the `decode/holdout` case, so the corpus has
/// to carry a word for every one of its keys; otherwise the case would measure how
/// fast the decoder gives up rather than how fast it decodes.
///
/// The file is `pinyin<TAB>expected first choice` with `#` comments and blank lines
/// skipped.
pub fn holdout_keys() -> Vec<String> {
    HOLDOUT
        .lines()
        .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
        .filter_map(|line| line.split('\t').next())
        .filter(|key| !key.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The syllables `raw` is cut into, by the segmentation layer's default hint.
fn cut(raw: &str) -> Result<Vec<String>, String> {
    let mut dag = SyllableDag::new();
    dag.build(raw)
        .map_err(|error| format!("cannot be cut into syllables: {error}"))?;
    let mut boundaries = SmallVec::<[u16; HINT_INLINE_BOUNDARIES]>::new();
    if !dag.best_segmentation_hint(&mut boundaries) {
        return Err(String::from("has no segmentation at all"));
    }
    let normalized = dag.normalized();
    let mut syllables = Vec::with_capacity(boundaries.len());
    for pair in boundaries.windows(2) {
        let (start, end) = (usize::from(pair[0]), usize::from(pair[1]));
        let Some(text) = normalized.get(start..end) else {
            return Err(format!(
                "boundary {start}..{end} is not a byte range of the input"
            ));
        };
        // A forced boundary marker belongs to no syllable, so it is dropped from the
        // slice the hint's boundaries delimit.
        syllables.push(text.replace('\'', ""));
    }
    Ok(syllables)
}
