//! The symbol table: the categories a `v`-prefixed input offers as candidates.
//!
//! Responsibility: hold the symbol categories the v-mode serves -- digits under `v1`,
//! arrows under `vx`, pinyin tones under `vym` -- and answer, for the code typed after
//! the `v`, the category it names. The session assembles the category's symbols as
//! ordinary candidates; this module only holds the table and answers the lookup.
//!
//! Boundaries: everything here is pure. The text of the table arrives as a `&str` and
//! the loader that owns the bytes lives in the host layer, exactly as the phrase
//! table's does; no file, no clock, no environment and no global state is read. The
//! table decides nothing about a composition: it is asked about a code and answers a
//! category or nothing, and a code no category names is what makes the session fall
//! through to the ordinary decode.
//!
//! # The document
//!
//! One category per line, three tab-separated columns; a blank line and a line whose
//! first non-blank character is `#` carry no entry:
//!
//! ```text
//! # code <TAB> the label the candidates carry <TAB> the symbols, one after another
//! 1<TAB>数字<TAB>０１２３４５６７８９
//! x<TAB>箭头<TAB>←↑→↓
//! ```
//!
//! The code is folded to lower case and must be 1..=[`MAX_SYMBOL_CODE_LEN`] bytes of
//! `a`..=`z` or a digit; the label must be non-empty and at most
//! [`MAX_SYMBOL_LABEL_LEN`] bytes; the symbol list must be non-empty and hold at most
//! [`MAX_SYMBOLS_PER_CATEGORY`] characters. A code defined twice keeps its last
//! definition, which is what lets a shipped table be overridden by one read after it.
//!
//! # The lookup
//!
//! The longest category prefix that the code starts with wins -- `v1x` is the digit
//! category with `x` ignored, which is the documented answer to a code longer than
//! any prefix: the panel serves what the user asked for first and lets the rest go.
//! A code that no prefix matches, and the bare `v` before any code exists, answer
//! `None`, and the session stays in the ordinary decode.
//!
//! # Concurrency
//!
//! A built table is never mutated, so it is `Send + Sync` and any number of sessions
//! may read one at once. Rebuilding it is a load-time job: a reload parses the
//! document into a fresh table and replaces the pointer the sessions hold.

use ime_types::ImeError;

/// Longest code a category is keyed by, in bytes.
///
/// A code is typed, not read: one or two keystrokes after the `v` is what a panel
/// prefix costs. The bound keeps a file that is not a symbol table from being loaded
/// as one.
pub const MAX_SYMBOL_CODE_LEN: usize = 4;

/// Longest label a category carries, in bytes.
///
/// The label is the annotation the candidates show: two to four hanzi name a category
/// in full.
pub const MAX_SYMBOL_LABEL_LEN: usize = 24;

/// Most symbols one category may hold.
///
/// The digit and sequence categories reach into the forties and stay paged by the
/// ordinary candidate window; a category past a hundred would out-page them for no
/// reading a panel serves.
pub const MAX_SYMBOLS_PER_CATEGORY: usize = 96;

/// The key a malformed symbol document is reported against.
///
/// The document is a shipped data file, so a row that cannot be used is a packaging
/// fault rather than a user's; the key names the file kind rather than a path.
const SYMBOLS_FILE_KEY: &str = "symbols.file";

/// Every symbol category, in file order, which is the menu order for the bare `v`.
#[derive(Clone, Debug, Default)]
pub struct SymbolTable {
    /// The categories, kept in the order the document listed them.
    categories: Vec<Category>,
}

/// One category of the table.
#[derive(Clone, Debug)]
struct Category {
    /// The code typed after the `v`, folded to lower case.
    code: Box<str>,
    /// The label the category's candidates carry.
    label: Box<str>,
    /// The symbols, in file order, which is the display order.
    symbols: Vec<char>,
}

/// What reading a symbol document produced.
///
/// The counts a loader reports: how much of the file became a table, and what was
/// left behind. A file that is refused outright has no report -- the refusal names
/// the line it stopped on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SymbolReport {
    /// Categories the resulting table holds.
    pub loaded: usize,
    /// Rows that could not be used and were skipped.
    pub skipped: usize,
    /// Rows that replaced an earlier definition of the same code.
    pub overridden: usize,
}

impl SymbolTable {
    /// Builds a table from the text of a symbol document, refusing a row it cannot
    /// use.
    ///
    /// This is the form a build step or a test wants: the answer names the line that
    /// is wrong instead of quietly leaving it out.
    ///
    /// # Errors
    ///
    /// [`ImeError::ConfigInvalid`] naming [`SYMBOLS_FILE_KEY`] for the first row that
    /// cannot be used -- a line that is not three tab-separated fields, a code outside
    /// the code alphabet or past [`MAX_SYMBOL_CODE_LEN`], an empty label or one past
    /// [`MAX_SYMBOL_LABEL_LEN`], or an empty symbol list or one past
    /// [`MAX_SYMBOLS_PER_CATEGORY`]. The reason carries the line number and never the
    /// line's text.
    ///
    /// # Panics
    ///
    /// Never: every row is read with a checked split and a checked slice.
    ///
    /// # Examples
    ///
    /// ```
    /// use ime_core::symbols::SymbolTable;
    ///
    /// let table = SymbolTable::parse("1\t数字\t０１２３\n").expect("rows are well formed");
    /// assert_eq!(table.len(), 1);
    /// ```
    pub fn parse(text: &str) -> Result<Self, ImeError> {
        let mut rows: Vec<RawRow> = Vec::new();
        for (index, line) in text.lines().enumerate() {
            match row_of(line) {
                Ok(None) => continue,
                Ok(Some(row)) => rows.push(row),
                Err(reason) => {
                    return Err(ImeError::ConfigInvalid {
                        key: String::from(SYMBOLS_FILE_KEY),
                        reason: format!("line {}: {reason}", index.saturating_add(1)),
                    });
                }
            }
        }
        let (table, _) = assemble(rows);
        Ok(table)
    }

    /// Builds a table from the text of a symbol document, skipping rows it cannot
    /// use.
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
    /// use ime_core::symbols::SymbolTable;
    ///
    /// let (table, report) = SymbolTable::load("1\t数字\t０１２\nbroken\n");
    /// assert_eq!(report.loaded, 1);
    /// assert_eq!(report.skipped, 1);
    /// ```
    pub fn load(text: &str) -> (Self, SymbolReport) {
        let mut rows: Vec<RawRow> = Vec::new();
        let mut report = SymbolReport::default();
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

    /// Builds an empty table, which names no category.
    ///
    /// The state a build is in when the data file is absent: the v-mode degrades to
    /// it rather than to an error, and an input that starts with `v` decodes exactly
    /// as it did before the panel existed.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Answers the category the code typed after the `v` names.
    ///
    /// The longest prefix that `code` starts with wins, so a code longer than the
    /// prefix that serves it is still served. The answer carries the category's
    /// position in the table, its label and its symbols in display order.
    ///
    /// # Errors
    ///
    /// None. A code no category names is an answer rather than an error -- the
    /// session falls through to the ordinary decode.
    ///
    /// # Panics
    ///
    /// Never.
    ///
    /// # Examples
    ///
    /// ```
    /// use ime_core::symbols::SymbolTable;
    ///
    /// let table = SymbolTable::parse("1\t数字\t０１２\n").expect("rows are well formed");
    /// let hit = table.longest_prefix("123").expect("the code names the digits");
    /// assert_eq!(hit.label, "数字");
    /// assert_eq!(hit.symbols, &['０', '１', '２']);
    /// assert!(table.longest_prefix("zhishi").is_none());
    /// ```
    pub fn longest_prefix(&self, code: &str) -> Option<CategoryHit<'_>> {
        let mut best: Option<(&Category, usize)> = None;
        for category in &self.categories {
            if !code.starts_with(&*category.code) {
                continue;
            }
            match best {
                // Longest prefix wins; a tie keeps the earlier row, which is the
                // order the document named them in.
                Some((_, length)) if length >= category.code.len() => {}
                _ => best = Some((category, category.code.len())),
            }
        }
        let (category, _) = best?;
        Some(CategoryHit {
            label: &category.label,
            symbols: &category.symbols,
        })
    }

    /// Returns how many categories the table holds.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn len(&self) -> usize {
        self.categories.len()
    }

    /// Returns whether the table holds no category at all.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn is_empty(&self) -> bool {
        self.categories.is_empty()
    }
}

/// One category a lookup answered.
///
/// The label and the symbols borrow the table: nothing is copied here, and the
/// session owns the copies it makes when it builds candidates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CategoryHit<'table> {
    /// The label the category's candidates carry.
    pub label: &'table str,
    /// The symbols, in display order.
    pub symbols: &'table [char],
}

/// One row of a symbol document, as the reader left it.
struct RawRow {
    /// The code typed after the `v`, folded to lower case.
    code: Box<str>,
    /// The label the category's candidates carry.
    label: Box<str>,
    /// The symbols, in file order.
    symbols: Vec<char>,
}

/// Reads one line of a symbol document.
///
/// # Returns
///
/// `Ok(None)` for a blank line and for a comment, `Ok(Some(row))` for a usable row,
/// and the fault of the first row that is not.
fn row_of(line: &str) -> Result<Option<RawRow>, String> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return Ok(None);
    }
    let mut fields = line.split('\t');
    let Some(code) = fields.next() else {
        return Err(String::from("not three tab-separated fields"));
    };
    let Some(label) = fields.next() else {
        return Err(String::from("not three tab-separated fields"));
    };
    let Some(symbols) = fields.next() else {
        return Err(String::from("not three tab-separated fields"));
    };
    if fields.next().is_some() {
        return Err(String::from("not three tab-separated fields"));
    }
    let code = code.trim().to_ascii_lowercase();
    if code.is_empty()
        || code.len() > MAX_SYMBOL_CODE_LEN
        || !code
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    {
        return Err(String::from(
            "the code is empty, too long, or outside a-z and 0-9",
        ));
    }
    if label.is_empty() || label.len() > MAX_SYMBOL_LABEL_LEN {
        return Err(String::from("the label is empty or too long"));
    }
    let symbols: Vec<char> = symbols.chars().collect();
    if symbols.is_empty() || symbols.len() > MAX_SYMBOLS_PER_CATEGORY {
        return Err(format!(
            "the category holds {} symbols; the bound is {MAX_SYMBOLS_PER_CATEGORY}",
            symbols.len()
        ));
    }
    Ok(Some(RawRow {
        code: code.into_boxed_str(),
        label: Box::from(label),
        symbols,
    }))
}

/// Resolves repeated codes and builds the table.
///
/// The file order is the menu order, so the rows are kept in the order the document
/// named them; a later definition of one code replaces the earlier one, which is
/// what lets a later document override a shipped one. Returns the table and how many
/// rows a later definition replaced.
fn assemble(rows: Vec<RawRow>) -> (SymbolTable, usize) {
    let mut categories: Vec<Category> = Vec::with_capacity(rows.len());
    let mut overridden = 0usize;
    for row in rows {
        match categories.iter_mut().find(|held| held.code == row.code) {
            Some(held) => {
                *held = Category {
                    code: row.code,
                    label: row.label,
                    symbols: row.symbols,
                };
                overridden = overridden.saturating_add(1);
            }
            None => categories.push(Category {
                code: row.code,
                label: row.label,
                symbols: row.symbols,
            }),
        }
    }
    (SymbolTable { categories }, overridden)
}

#[cfg(test)]
mod tests;
