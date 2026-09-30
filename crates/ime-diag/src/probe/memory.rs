//! The process's resident memory: what the memory budgets are stated in.
//!
//! Responsibility: read the two kernel interfaces the memory budgets name -- `VmRSS`
//! from `/proc/self/status`, and `Anonymous` plus `Private_Dirty` from
//! `/proc/self/smaps_rollup` -- and carry them, together with the baselines a growth is
//! measured from, in the form a snapshot file holds.
//!
//! # Why two interfaces
//!
//! `VmRSS` answers "how much of this process is in physical memory", which is what the
//! plugin's own budget is stated in. It cannot answer "how much of that is the plugin's
//! rather than the file it mapped": a dictionary read straight out of its mapping is
//! resident without being private, and a budget that counted those pages as private
//! would be measuring the kernel's page cache rather than the plugin. The anonymous and
//! private-dirty totals are what separate the two, which is why the dictionary's budget
//! is stated in them.
//!
//! # A reading is not a growth
//!
//! A reading is three numbers taken at one moment; every memory budget in the design is
//! a *growth*, and a growth is the difference between two readings. [`MemorySnapshot`]
//! therefore carries the reading a snapshot was taken at and the baselines the growths
//! are measured from, and computes the differences. It states no ceiling: which growth
//! a budget is stated for, and what the ceiling is, belong to the document that holds
//! the thresholds.
//!
//! # A reading that failed is not a reading of zero
//!
//! Every field of a [`MemorySnapshot`] is an `Option`, and a platform with no `/proc`, a
//! sandbox that hides it, or a baseline nobody marked leaves its field empty. An empty
//! field is reported as an unmeasured budget rather than as a zero, because a zero
//! passes every ceiling there is.
//!
//! # Reading the kernel's files
//!
//! Both files are read whole and parsed line by line. The parse is strict in the same
//! way the snapshot parser is: a field the reader needs and cannot find, or one whose
//! value is not a whole number of kibibytes, is an error rather than a zero. The read
//! happens when a snapshot is written -- once per report, off the key path -- and never
//! on the decode path.

use std::fs;
use std::io;

/// The kernel file `VmRSS` is read from.
const STATUS_FILE: &str = "/proc/self/status";

/// The kernel file the anonymous and private-dirty totals are read from.
const SMAPS_ROLLUP_FILE: &str = "/proc/self/smaps_rollup";

/// The prefix that marks a record of the memory section in a snapshot file.
///
/// Stated once, here, because two readers depend on it agreeing: the snapshot reader
/// skips a record carrying it, and this module's own reader accepts only records
/// carrying it. A prefix that drifted would have one reader refuse the other's file.
pub const MEMORY_PREFIX: &str = "memory.";

/// One reading of the process's resident memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryReading {
    /// Resident set size, in kibibytes.
    pub rss_kib: u64,
    /// Anonymous resident pages, in kibibytes.
    pub anonymous_kib: u64,
    /// Private dirty pages, in kibibytes.
    pub private_dirty_kib: u64,
}

impl MemoryReading {
    /// Reads this process's memory.
    ///
    /// # Errors
    ///
    /// Returns the underlying [`io::Error`] when either kernel file cannot be read -- a
    /// platform without `/proc`, a sandbox that hides it, a process whose `/proc/self`
    /// entry has gone away -- and [`io::ErrorKind::InvalidData`] when one of them does
    /// not carry the line this reader needs, or carries it with a value that is not a
    /// whole number. It never answers with a guessed reading: a number nothing measured
    /// must not become a number a budget passes against.
    pub fn now() -> io::Result<Self> {
        let status = read_file(STATUS_FILE)?;
        let rollup = read_file(SMAPS_ROLLUP_FILE)?;
        let (anonymous_kib, private_dirty_kib) = Self::parse_smaps_rollup(&rollup)?;
        let rss_kib = Self::parse_status(&status)?;
        Ok(Self {
            rss_kib,
            anonymous_kib,
            private_dirty_kib,
        })
    }

    /// The pages this process owns rather than shares with a file it mapped.
    ///
    /// The sum of the anonymous and private-dirty totals, saturated rather than wrapped:
    /// the two are the pair the dictionary's budget is stated in, and their sum is a
    /// size, so a value that wrapped would read as a process that owns almost nothing.
    ///
    /// It is a sum and not a partition -- an anonymous private page is in both totals --
    /// so it is an upper bound on what the process owns. That is the number the
    /// threshold names, which is why it is the number a report compares.
    pub fn dirty_kib(&self) -> u64 {
        self.anonymous_kib.saturating_add(self.private_dirty_kib)
    }

    /// Reads `VmRSS` out of a `/proc/self/status` dump, in kibibytes.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::InvalidData`] when the dump carries no `VmRSS` line, and
    /// when the line's value is not a whole number.
    pub fn parse_status(text: &str) -> io::Result<u64> {
        parse_kib_field(text, "VmRSS", STATUS_FILE)
    }

    /// Reads `Anonymous` and `Private_Dirty` out of a `smaps_rollup` dump, in kibibytes.
    ///
    /// # Errors
    ///
    /// As [`MemoryReading::parse_status`], naming the field that was missing or
    /// unreadable.
    pub fn parse_smaps_rollup(text: &str) -> io::Result<(u64, u64)> {
        let anonymous_kib = parse_kib_field(text, "Anonymous", SMAPS_ROLLUP_FILE)?;
        let private_dirty_kib = parse_kib_field(text, "Private_Dirty", SMAPS_ROLLUP_FILE)?;
        Ok((anonymous_kib, private_dirty_kib))
    }
}

/// The process's memory, as a snapshot file carries it.
///
/// Every field is an `Option`, and an empty one means "nothing measured this" rather
/// than "this measured zero"; see the module documentation for why that distinction is
/// load-bearing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemorySnapshot {
    /// Resident set size when the snapshot was taken, in kibibytes.
    pub rss_kib: Option<u64>,
    /// Anonymous plus private-dirty pages at the same moment, in kibibytes.
    pub dirty_kib: Option<u64>,
    /// Resident set size when the probes were created.
    pub baseline_rss_kib: Option<u64>,
    /// Anonymous plus private-dirty pages when the probes were created, in kibibytes.
    pub baseline_dirty_kib: Option<u64>,
    /// Resident set size when the UI rendering layer marked its baseline.
    pub ui_baseline_rss_kib: Option<u64>,
    /// Anonymous plus private-dirty pages when the dictionary had been mapped.
    pub dictionary_baseline_dirty_kib: Option<u64>,
}

impl MemorySnapshot {
    /// Whether `key` names a record of the memory section.
    ///
    /// The prefix is what the snapshot reader uses to tell its own records from this
    /// section's, so both readers ask this question rather than repeating the prefix.
    pub fn is_key(key: &str) -> bool {
        key.starts_with(MEMORY_PREFIX)
    }

    /// The plugin process's resident growth since the probes were created, in kibibytes.
    ///
    /// `None` when either the reading or the baseline is missing, which is what a run
    /// that never took the reading reports. A reading below its baseline answers zero:
    /// the budgets are ceilings, a process that shrank is inside every one of them, and
    /// a negative growth is not a number the comparison needs.
    pub fn plugin_growth_kib(&self) -> Option<u64> {
        growth(self.rss_kib, self.baseline_rss_kib)
    }

    /// The UI rendering layer's resident growth since it marked its baseline.
    ///
    /// `None` when the layer never marked a baseline, so a run in which the UI layer was
    /// never brought up reports no growth rather than the process's whole one.
    pub fn ui_growth_kib(&self) -> Option<u64> {
        growth(self.rss_kib, self.ui_baseline_rss_kib)
    }

    /// The dictionary mapping's private growth since it was mapped, in kibibytes.
    ///
    /// Measured in the anonymous and private-dirty totals rather than in the resident
    /// set, because the budget this answers is the one stated for pages the mapping owns
    /// rather than for pages it shares with the file.
    pub fn dictionary_growth_kib(&self) -> Option<u64> {
        growth(self.dirty_kib, self.dictionary_baseline_dirty_kib)
    }

    /// Renders the section as the records a snapshot file holds.
    ///
    /// A field that was not measured is left out rather than written as a zero, which is
    /// what makes an unmeasured growth survive the file: the reader that parses it back
    /// sees an absent record, not a measured zero.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        for (field, read) in FIELDS {
            if let Some(value) = read(self) {
                out.push_str(&format!("{MEMORY_PREFIX}{field}={value}\n"));
            }
        }
        out
    }

    /// Reads the memory section out of a snapshot file's text.
    ///
    /// Records that are not the memory section's are skipped, so the whole file can be
    /// handed to this reader and to the snapshot's own one.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::InvalidData`] when a memory record is not a `key=value`
    /// line, names a field the section does not carry, is written twice, or carries a
    /// value that is not a whole number. Every message names the line, so a hand-edited
    /// file says where it went wrong.
    pub fn parse(text: &str) -> io::Result<Self> {
        let mut snapshot = Self::default();
        let mut seen: Vec<&str> = Vec::new();
        for (index, line) in text.lines().enumerate() {
            let line = line.trim();
            if !Self::is_key(line) {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                return Err(malformed(format!(
                    "line {}: `{line}` is not a `key=value` record",
                    index + 1
                )));
            };
            let key = key.trim();
            if seen.contains(&key) {
                let line = index + 1;
                return Err(malformed(format!("line {line}: `{key}` is written twice")));
            }
            seen.push(key);
            let value = value.trim();
            let number = value
                .parse::<u64>()
                .map_err(|_| malformed(format!("`{key}`: `{value}` is not a whole number")))?;
            snapshot.set_field(key, number)?;
        }
        Ok(snapshot)
    }

    /// Writes one field, refusing a name the section does not carry.
    fn set_field(&mut self, field: &str, value: u64) -> io::Result<()> {
        let slot = match field {
            "memory.rss_kib" => &mut self.rss_kib,
            "memory.dirty_kib" => &mut self.dirty_kib,
            "memory.baseline_rss_kib" => &mut self.baseline_rss_kib,
            "memory.baseline_dirty_kib" => &mut self.baseline_dirty_kib,
            "memory.ui_baseline_rss_kib" => &mut self.ui_baseline_rss_kib,
            "memory.dictionary_baseline_dirty_kib" => &mut self.dictionary_baseline_dirty_kib,
            other => {
                let known: Vec<String> = FIELDS
                    .iter()
                    .map(|(name, _)| format!("{MEMORY_PREFIX}{name}"))
                    .collect();
                return Err(malformed(format!(
                    "`{other}` is not a field of the memory section; expected one of {}",
                    known.join(", ")
                )));
            }
        };
        *slot = Some(value);
        Ok(())
    }
}

/// Reads one field out of the section.
type FieldReader = fn(&MemorySnapshot) -> Option<u64>;

/// The fields of the memory section, in the order the file writes them, each with the
/// reader that extracts it.
///
/// One table rather than a pair of `match` arms that have to be kept in step: the writer
/// walks it, and the parser refuses a field name that is not in it, so a field cannot be
/// written by one half of the format and unknown to the other.
const FIELDS: [(&str, FieldReader); 6] = [
    ("rss_kib", |memory| memory.rss_kib),
    ("dirty_kib", |memory| memory.dirty_kib),
    ("baseline_rss_kib", |memory| memory.baseline_rss_kib),
    ("baseline_dirty_kib", |memory| memory.baseline_dirty_kib),
    ("ui_baseline_rss_kib", |memory| memory.ui_baseline_rss_kib),
    (
        "dictionary_baseline_dirty_kib",
        |memory| memory.dictionary_baseline_dirty_kib,
    ),
];


/// The growth from `baseline` to `reading`, or `None` when either was not measured.
fn growth(reading: Option<u64>, baseline: Option<u64>) -> Option<u64> {
    Some(reading?.saturating_sub(baseline?))
}

/// Reads one of the kernel's files, naming it in the failure.
fn read_file(path: &str) -> io::Result<String> {
    fs::read_to_string(path).map_err(|error| unreadable(path, error))
}

/// Reads one `Key: <number> kB` line out of a `/proc` dump.
///
/// The scan splits on the first colon and compares the whole name, so `RssAnon` is not
/// `Anonymous` and `Pss_Anon` is not either; a line whose value is not a number is
/// reported rather than skipped, because a field that is there and unreadable is a
/// reader that has drifted from the kernel rather than a kernel that has no such field.
fn parse_kib_field(text: &str, key: &str, path: &str) -> io::Result<u64> {
    for line in text.lines() {
        let Some((name, rest)) = line.split_once(':') else {
            continue;
        };
        if name.trim() != key {
            continue;
        }
        let value = rest.split_whitespace().next().unwrap_or_default();
        return value
            .parse::<u64>()
            .map_err(|_| malformed(format!("{path}: `{key}` is not a number: `{rest}`")));
    }
    Err(malformed(format!("{path}: `{key}` is not in the file")))
}

/// A file that could not be read.
fn unreadable(path: &str, source: io::Error) -> io::Error {
    io::Error::new(source.kind(), format!("{path}: {source}"))
}

/// A dump that could not be parsed.
fn malformed(detail: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, detail.into())
}
