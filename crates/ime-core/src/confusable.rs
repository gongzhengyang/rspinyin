//! The confusable-word table: the wrong forms a hint steers the reader away from.
//!
//! Responsibility: hold the pairs of a wrong spelling and the correct one -- `按奈` for
//! `按捺`, `迫不急待` for `迫不及待` -- and answer, for one candidate's exact text, what
//! the correct form is. The annotation channel turns the answer into the `字:→正字`
//! hint a candidate carries; this module only holds the table and answers the lookup.
//!
//! Boundaries: everything here is pure. The text of the table arrives as a `&str` and
//! the loader that owns the bytes lives in the host layer, exactly as the phrase
//! table's does; no file, no clock, no environment and no global state is read. The
//! table decides nothing about a candidate: it is asked about a finished text and
//! answers a borrowed slice or nothing.
//!
//! # Why the rows are whole wrong words
//!
//! A hint is only honest when the wrong form is wrong wherever it appears. The
//! classic character-level confusions -- 的/地/得, 在/再, 帐/账 -- are positional: `帐`
//! is the right spelling in `帐篷` and wrong in `账目`, so a hint keyed on the bare
//! character would misfire on every second candidate. The rows this table holds are
//! therefore whole words whose wrong form is wrong in every context, which is what
//! makes a lookup on the candidate's whole text safe without a context model.
//!
//! # The document
//!
//! One entry per line, two tab-separated columns; a blank line and a line whose first
//! non-blank character is `#` carry no entry:
//!
//! ```text
//! # the wrong form <TAB> the correct form
//! 按奈<TAB>按捺
//! 迫不急待<TAB>迫不及待
//! ```
//!
//! Both columns must be non-empty and at most [`MAX_CONFUSABLE_LEN`] bytes, and a
//! wrong form may be defined once: the last definition of a repeated key wins, which
//! is what lets a shipped table be overridden by one read after it.
//!
//! # The table
//!
//! The same shape the phrase table uses: entries sorted by key with their answers
//! concatenated into one pool, so a lookup is a binary search and a hit is a byte
//! range rather than a copy. A built table is never mutated, so it is `Send + Sync`
//! and any number of decodes may read one at once; a reload builds a fresh table and
//! swaps the pointer.

use ime_types::ImeError;

/// Longest wrong or correct form, in bytes.
///
/// Eight hanzi at three bytes each covers every idiom-length wrong form a table
/// carries, and the bound keeps a file that is not a confusable table from being
/// loaded as one.
pub const MAX_CONFUSABLE_LEN: usize = 24;

/// The annotation prefix a confusable hint carries, ahead of the correct form.
///
/// User-facing text is Chinese by the project's language convention, and the arrow
/// says in one glyph what the hint means: this form, the one after it instead.
pub const CONFUSABLE_ANNOTATION_PREFIX: &str = "字:→";

/// The key a malformed confusable document is reported against.
///
/// The document is a shipped data file, so a row that cannot be used is a packaging
/// fault rather than a user's; the key names the file kind rather than a path.
const CONFUSABLE_FILE_KEY: &str = "confusable.file";

/// Every confusable pair, sorted by the wrong form and ready to match.
///
/// Built once per load and read by every decode, so it is immutable in use.
#[derive(Clone, Debug, Default)]
pub struct ConfusableTable {
    /// Entries in ascending key order, which is what makes a lookup a binary search.
    entries: Vec<Entry>,
    /// Every correct form, concatenated in entry order. An answer is a slice of this
    /// pool rather than an owned string, so a table of a thousand rows is one
    /// allocation for the text.
    pool: String,
}

/// One entry of the table.
#[derive(Clone, Debug)]
struct Entry {
    /// The wrong form, exactly as the document spells it.
    key: Box<str>,
    /// Start of the correct form in [`ConfusableTable::pool`].
    start: u32,
    /// End of the correct form in [`ConfusableTable::pool`].
    end: u32,
}

/// What reading a confusable document produced.
///
/// The counts a loader reports: how much of the file became a table, and what was
/// left behind. A file that is refused outright has no report -- the refusal names
/// the line it stopped on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ConfusableReport {
    /// Entries the resulting table holds.
    pub loaded: usize,
    /// Rows that could not be used and were skipped.
    pub skipped: usize,
    /// Rows that replaced an earlier definition of the same wrong form.
    pub overridden: usize,
}

impl ConfusableTable {
    /// Builds a table from the text of a confusable document, refusing a row it
    /// cannot use.
    ///
    /// This is the form a build step or a test wants: the answer names the line that
    /// is wrong instead of quietly leaving it out.
    ///
    /// # Errors
    ///
    /// [`ImeError::ConfigInvalid`] naming [`CONFUSABLE_FILE_KEY`] for the first row
    /// that cannot be used -- a line that is not two tab-separated fields, or a
    /// column that is empty or past [`MAX_CONFUSABLE_LEN`]. The reason carries the
    /// line number and never the line's text.
    ///
    /// # Panics
    ///
    /// Never: every row is read with a checked split and a checked slice.
    ///
    /// # Examples
    ///
    /// ```
    /// use ime_core::confusable::ConfusableTable;
    ///
    /// let table = ConfusableTable::parse("按奈\t按捺\n").expect("rows are well formed");
    /// assert_eq!(table.lookup("按奈"), Some("按捺"));
    /// ```
    pub fn parse(text: &str) -> Result<Self, ImeError> {
        let mut rows: Vec<Row> = Vec::new();
        for (index, line) in text.lines().enumerate() {
            match row_of(line) {
                Ok(None) => continue,
                Ok(Some(row)) => rows.push(row),
                Err(reason) => {
                    return Err(ImeError::ConfigInvalid {
                        key: String::from(CONFUSABLE_FILE_KEY),
                        reason: format!("line {}: {reason}", index.saturating_add(1)),
                    });
                }
            }
        }
        let (table, _) = assemble(rows);
        Ok(table)
    }

    /// Builds a table from the text of a confusable document, skipping rows it
    /// cannot use.
    ///
    /// This is the degradation path the host loads with: a table the user can live
    /// without must not be able to stop the input method from starting, so a row
    /// that cannot be used is counted and the walk continues.
    ///
    /// # Errors
    ///
    /// None: every row that cannot be used is counted rather than propagated.
    ///
    /// # Panics
    ///
    /// Never: every row is read with a checked split and a checked slice.
    ///
    /// # Examples
    ///
    /// ```
    /// use ime_core::confusable::ConfusableTable;
    ///
    /// let (table, report) = ConfusableTable::load("按奈\t按捺\nbroken\n");
    /// assert_eq!(report.loaded, 1);
    /// assert_eq!(report.skipped, 1);
    /// ```
    pub fn load(text: &str) -> (Self, ConfusableReport) {
        let mut rows: Vec<Row> = Vec::new();
        let mut report = ConfusableReport::default();
        for line in text.lines() {
            match row_of(line) {
                Ok(Some(row)) => rows.push(row),
                Ok(None) => continue,
                Err(_) => report.skipped = report.skipped.saturating_add(1),
            }
        }
        let (table, overridden) = assemble(rows);
        report.overridden = overridden;
        report.loaded = usize::try_from(table.len()).unwrap_or(usize::MAX);
        (table, report)
    }

    /// Builds an empty table, which matches nothing.
    ///
    /// The state a build is in when the data file is absent: the hint channel degrades
    /// to it rather than to an error, so a build without the file behaves exactly like
    /// one whose table matches nothing.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Looks up the correct form for one candidate's exact text.
    ///
    /// The match is on the whole text: a candidate the table's wrong form is not
    /// spelled by in full answers `None`, which is what keeps a hint from firing on
    /// a substring of a longer word.
    ///
    /// # Errors
    ///
    /// None. A form the table does not hold is an answer rather than an error -- the
    /// hint channel copies the candidate through unchanged.
    ///
    /// # Panics
    ///
    /// Never.
    ///
    /// # Examples
    ///
    /// ```
    /// use ime_core::confusable::ConfusableTable;
    ///
    /// let table = ConfusableTable::parse("一如继往\t一如既往\n").expect("rows are well formed");
    /// assert_eq!(table.lookup("一如继往"), Some("一如既往"));
    /// assert_eq!(table.lookup("一如既往"), None);
    /// ```
    pub fn lookup(&self, text: &str) -> Option<&str> {
        let index = self
            .entries
            .binary_search_by(|entry| (*entry.key).cmp(text))
            .ok()?;
        let entry = self.entries.get(index)?;
        let (Ok(start), Ok(end)) = (usize::try_from(entry.start), usize::try_from(entry.end))
        else {
            return None;
        };
        self.pool.get(start..end)
    }

    /// Returns how many entries the table holds.
    ///
    /// Reported by the diagnostics surface, so a build can say whether its file was
    /// read at all.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn len(&self) -> u64 {
        u64::try_from(self.entries.len()).unwrap_or(u64::MAX)
    }

    /// Returns whether the table holds no entry at all.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// One row of a confusable document, as the reader left it.
struct Row {
    /// The wrong form.
    key: Box<str>,
    /// The correct form.
    text: Box<str>,
}

/// Reads one line of a confusable document.
///
/// # Returns
///
/// `Ok(None)` for a blank line and for a comment, `Ok(Some(row))` for a usable row,
/// and the fault of the first row that is not.
fn row_of(line: &str) -> Result<Option<Row>, String> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return Ok(None);
    }
    let mut fields = line.split('\t');
    let Some(key) = fields.next() else {
        return Err(String::from("not two tab-separated fields"));
    };
    let Some(text) = fields.next() else {
        return Err(String::from("not two tab-separated fields"));
    };
    if fields.next().is_some() {
        return Err(String::from("not two tab-separated fields"));
    }
    if key.is_empty() || text.is_empty() {
        return Err(String::from("a column is empty"));
    }
    if key.len() > MAX_CONFUSABLE_LEN {
        return Err(format!(
            "the wrong form is longer than {MAX_CONFUSABLE_LEN} bytes"
        ));
    }
    if text.len() > MAX_CONFUSABLE_LEN {
        return Err(format!(
            "the correct form is longer than {MAX_CONFUSABLE_LEN} bytes"
        ));
    }
    Ok(Some(Row {
        key: Box::from(key),
        text: Box::from(text),
    }))
}

/// Sorts the rows, resolves repeated keys and builds the table.
///
/// The sort is stable, so the definitions of one key stay in the order the document
/// gave them and the last of them is the one that survives -- the same rule the
/// phrase table applies, and what lets a later document override a shipped one.
/// Returns the table and how many rows a later definition replaced.
fn assemble(mut rows: Vec<Row>) -> (ConfusableTable, usize) {
    rows.sort_by(|left, right| (*left.key).cmp(&*right.key));
    let mut unique: Vec<Row> = Vec::with_capacity(rows.len());
    let mut overridden = 0usize;
    for row in rows {
        match unique.last_mut() {
            Some(previous) if previous.key == row.key => {
                *previous = row;
                overridden = overridden.saturating_add(1);
            }
            _ => unique.push(row),
        }
    }
    let mut pool = String::new();
    let mut entries = Vec::with_capacity(unique.len());
    for row in unique {
        // The pool is bounded by the row count times `MAX_CONFUSABLE_LEN`, so the
        // offsets of a table that was read at all fit a `u32`; the saturation is what
        // a document past that bound would need, and it cannot be built.
        let start = u32::try_from(pool.len()).unwrap_or(u32::MAX);
        pool.push_str(&row.text);
        let end = u32::try_from(pool.len()).unwrap_or(u32::MAX);
        entries.push(Entry {
            key: row.key,
            start,
            end,
        });
    }
    (ConfusableTable { entries, pool }, overridden)
}

#[cfg(test)]
mod tests;
