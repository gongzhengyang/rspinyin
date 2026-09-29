//! The raw-source layer of the compiler: reading the TSV formats under
//! `data/raw/` into words, readings and corrections.
//!
//! Responsibility: everything that knows what a source file looks like -- the L1
//! character table, the word list, the L3c correction table -- plus the syllable
//! folding every reading goes through before it can become a key.
//!
//! Boundaries: this layer never touches the container format and never writes
//! anything. It reports what it could not use instead of failing, because a raw
//! source is a moving target: a row that names a character the L1 table does not
//! cover must be counted and skipped, not turned into a build failure. The one
//! exception is the correction table, whose rows are asserted to be legal
//! syllables, since a row that cannot be read would silently stop matching. A
//! source file that is not there at all is the second exception, and it is
//! reported through [`read_source`] as the stable `dict/source/missing`
//! diagnostic: the raw sources are fetched rather than committed, so "absent" is
//! an ordinary state of a checkout and has to name its own remedy.
//!
//! The module is split by responsibility: this root holds the parsed-word types and
//! the entry points, [`reading`] folds and segments readings, [`words`] parses the
//! word list, [`polyphone`] reads the correction table, and [`synth`] generates the
//! probe corpus.

mod polyphone;
mod reading;
mod synth;
mod words;

use std::fs;
use std::io;
use std::path::Path;

use anyhow::{Result, anyhow};

pub(crate) use self::polyphone::{apply_polyphone, load_polyphone};
pub(crate) use self::reading::load_l1;
pub(crate) use self::synth::synth_words;
pub(crate) use self::words::load_words;

/// The diagnostic a raw source that has not been fetched produces.
///
/// The raw sources are deliberately not committed: `data/sources.toml` pins them by
/// SHA256 and `scripts/check-dict-sources.sh` treats that file as the single allowlist,
/// which is what makes the fetch reproducible without keeping an upstream copy in the
/// repository. A checkout that has never run the fetch script therefore holds none of
/// them, and the compiler has to say that rather than surface a bare
/// `No such file or directory`.
///
/// The message carries the remedy next to the code: its reader is a developer or a
/// packager who can run the command. An end user never sees it, because a release ships
/// the compiled `base.dict` and never runs this compiler at all.
pub(crate) const SOURCE_MISSING: &str = "dict/source/missing: a dictionary source has not been fetched. \
     Run `bash data/fetch.sh` to download the sources registered in data/sources.toml, \
     or install a prebuilt `base.dict` from a release archive.";

/// Reads one raw source, reporting a file that is not there as [`SOURCE_MISSING`].
///
/// Every reader of a raw source goes through here so that "the source has not been
/// fetched" has exactly one rendering: a stable code plus the command that fixes it.
/// Any other failure keeps the underlying error, because a permission problem or an
/// invalid encoding is not a fetch that has not run, and pointing the caller at the
/// network would hide the real cause behind a remedy that cannot help.
///
/// # Errors
///
/// Returns the [`SOURCE_MISSING`] diagnostic when `path` does not exist, and the
/// underlying I/O error annotated with the path for every other failure.
pub(crate) fn read_source(path: &Path) -> Result<String> {
    fs::read_to_string(path).map_err(|error| source_error(path, error))
}

/// Renders one failure to read a raw source.
///
/// A file that is not there is the ordinary state of a checkout that has not run the
/// fetch script, so it gets the code and the remedy. Anything else is passed through with
/// the path attached, because it needs a different fix.
fn source_error(path: &Path, error: io::Error) -> anyhow::Error {
    if error.kind() == io::ErrorKind::NotFound {
        return anyhow!("{SOURCE_MISSING} Missing file: {}", path.display());
    }
    anyhow::Error::new(error).context(format!("cannot read {}", path.display()))
}

#[cfg(test)]
mod tests;

/// One parsed word, with the reading information the compiler needs.
#[derive(Debug)]
pub(crate) struct Word {
    /// Word text.
    pub(crate) text: String,
    /// Canonical key: the L3a reading, or the reading the row states.
    pub(crate) key: String,
    /// Every reading of every character, in source order, filled only for words
    /// inside the expansion band; empty means the word is not expanded.
    pub(crate) readings: Vec<Vec<String>>,
    /// Ranking weight.
    pub(crate) weight: u32,
    /// Bit set of `ime_dict::format::FLAG_*`.
    pub(crate) flags: u8,
    /// Whether some character of the word carries more than one reading, which is
    /// what makes the baseline reading capable of being wrong.
    pub(crate) polyphone: bool,
}

/// Counters for rows the compiler refused to use, by reason.
#[derive(Debug, Default)]
pub(crate) struct Skips {
    /// Rows whose word is not made of Han characters.
    pub(crate) non_han: u64,
    /// Rows whose word is longer than the container can store.
    pub(crate) too_long: u64,
    /// Rows whose weight column is not a number.
    pub(crate) bad_weight: u64,
    /// Rows whose flags column names no known flag.
    pub(crate) bad_flags: u64,
    /// Rows whose reading is not a legal syllable sequence.
    pub(crate) bad_reading: u64,
    /// Rows naming a character the L1 source does not cover.
    pub(crate) unknown_char: u64,
    /// Rows repeating a word already seen; the heavier row is kept.
    pub(crate) duplicate: u64,
    /// A few examples, so a skipped row can be reproduced without a debugger.
    pub(crate) examples: Vec<String>,
}

impl Skips {
    /// Records one skipped row.
    fn note(&mut self, reason: &str, line: u64, detail: &str) {
        match reason {
            "non-han" => self.non_han += 1,
            "too-long" => self.too_long += 1,
            "bad-weight" => self.bad_weight += 1,
            "bad-flags" => self.bad_flags += 1,
            "bad-reading" => self.bad_reading += 1,
            "unknown-char" => self.unknown_char += 1,
            "duplicate" => self.duplicate += 1,
            _ => {}
        }
        if self.examples.len() < 8 {
            self.examples
                .push(format!("{reason}: line {line}: {detail}"));
        }
    }

    /// Total number of skipped rows.
    pub(crate) fn total(&self) -> u64 {
        self.non_han
            + self.too_long
            + self.bad_weight
            + self.bad_flags
            + self.bad_reading
            + self.unknown_char
            + self.duplicate
    }
}
