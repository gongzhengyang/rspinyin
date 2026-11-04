//! The symbol table: the categories a `v`-prefixed input offers as candidates.
//!
//! Responsibility: hold the symbol categories the v-mode serves -- digits under `v1`,
//! arrows under `vx`, pinyin tones under `vym`, the emoji domain under `ve` -- and
//! answer, for the code typed after the `v`, the category it names. The session
//! assembles the category's symbols as ordinary candidates; this module only holds
//! the table and answers the lookup, plus the two views beside it: [`SymbolTable::serve`],
//! which applies the emoji alias filter to a category's list, and [`SymbolTable::domains`],
//! the rows the panel's `vhelp` index page is built from.
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
//! # The emoji domain
//!
//! One category of the table may be the emoji domain, installed by
//! [`SymbolTable::add_emoji_domain`] from a document of its own: one entry per line,
//! two tab-separated columns -- one single-code-point character and the pinyin alias
//! it answers to. An alias is folded nowhere and matched as typed: what the user
//! typed past the domain's code filters the list, and the entries whose alias starts
//! with it are served in file order, homophones side by side -- which is why the
//! document is allowed to repeat an alias.
//!
//! # Concurrency
//!
//! A built table is never mutated, so it is `Send + Sync` and any number of sessions
//! may read one at once. Rebuilding it is a load-time job: a reload parses the
//! document into a fresh table and replaces the pointer the sessions hold.

use ime_types::ImeError;

mod emoji;

pub use emoji::MAX_EMOJI_ALIAS_LEN;
pub use emoji::MAX_EMOJI_PER_DOMAIN;

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
    /// The pinyin alias each symbol answers to, aligned with `symbols` by position,
    /// when the category is one the alias filter serves -- the emoji domain. `None`
    /// for a category of the plain document: its list is served whole, and a suffix
    /// typed past its prefix is ignored exactly as the table documents.
    aliases: Option<Vec<Box<str>>>,
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
    /// [`ImeError::ConfigInvalid`] naming `SYMBOLS_FILE_KEY` for the first row that
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
        report.loaded = table.len();
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
    /// label and its symbols in display order; [`SymbolTable::serve`] is the form
    /// the panel itself reads, which also applies the emoji alias filter.
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
        longest_prefix_category(self, code).map(|category| CategoryHit {
            label: &category.label,
            symbols: &category.symbols,
        })
    }

    /// Answers what the panel serves for the code typed after the `v`.
    ///
    /// The category is the one [`SymbolTable::longest_prefix`] names, and the answer
    /// starts the same way: the label, and the symbols in display order. What this
    /// form adds is the emoji alias: a category that carries aliases serves the
    /// suffix typed past its prefix as a filter, and the answer holds the entries
    /// whose alias starts with it, in file order -- homophones side by side, which
    /// is what the emoji document's repeated aliases are for. A category without
    /// aliases, and an exact prefix with no suffix, serves the whole list.
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
    /// let mut table = SymbolTable::empty();
    /// let report = table.add_emoji_domain("😀\txiao\n😁\txiao\n😊\tlian\n");
    /// assert_eq!(report.loaded, 1);
    /// let served = table.serve("exiao").expect("the code names the domain");
    /// assert_eq!(served.symbols.as_slice(), &['😀', '😁'], "homophones stay together");
    /// ```
    pub fn serve(&self, code: &str) -> Option<Served<'_>> {
        let category = longest_prefix_category(self, code)?;
        // The slice is in bounds: the walk above only answers a category whose
        // code `code` starts with, and the code alphabet is ASCII, so the prefix
        // length is a character boundary of `code`.
        let suffix = &code[category.code.len()..];
        let symbols = match (&category.aliases, suffix.is_empty()) {
            (Some(aliases), false) => {
                ServedSymbols::Filtered(emoji::filter(aliases, &category.symbols, suffix))
            }
            _ => ServedSymbols::All(&category.symbols),
        };
        Some(Served {
            label: &category.label,
            symbols,
        })
    }

    /// Returns every domain as the code and the label the index page names it by.
    ///
    /// The order is the table's: the document's file order, with the emoji domain
    /// last when it was added after the document was read. The index page serves
    /// one candidate per domain in exactly this order, which is what makes a
    /// selection's position name the domain it selects.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn domains(&self) -> impl Iterator<Item = SymbolDomain<'_>> {
        self.categories.iter().map(|category| SymbolDomain {
            code: &category.code,
            label: &category.label,
        })
    }

    /// Parses an emoji document and installs it as the table's emoji domain.
    ///
    /// The document is rows of one single-code-point character, a tab, and the
    /// lower-case pinyin alias the entry answers to; blank lines and `#` comments
    /// carry nothing. The domain answers to `e` typed after the `v`, and a category
    /// the document may already hold under that code is replaced -- the same
    /// last-definition-wins rule a later line of the plain document follows.
    ///
    /// This is the skipping reader, like [`SymbolTable::load`]: a row that cannot
    /// be used is counted and the walk continues, because a domain the user can
    /// live without must not be able to stop the panel from serving the rest.
    /// A document that yields nothing usable installs no domain at all.
    ///
    /// # Errors
    ///
    /// None: every row that cannot be used is counted rather than propagated.
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
    /// let mut table = SymbolTable::empty();
    /// let report = table.add_emoji_domain("😀\txiao\nbroken\n");
    /// assert_eq!(report.loaded, 1);
    /// assert_eq!(report.skipped, 1);
    /// assert_eq!(table.serve("e").expect("served").symbols.as_slice(), &['😀']);
    /// ```
    pub fn add_emoji_domain(&mut self, text: &str) -> SymbolReport {
        let mut report = SymbolReport::default();
        let mut symbols: Vec<char> = Vec::new();
        let mut aliases: Vec<Box<str>> = Vec::new();
        for line in text.lines() {
            if symbols.len() >= MAX_EMOJI_PER_DOMAIN {
                // Past the bound the rest of the document is refused rather than
                // served: the bound is what keeps a file that is not an emoji
                // document from being loaded as one, so an over-long document is
                // skips, not a truncated domain.
                report.skipped = report.skipped.saturating_add(1);
                continue;
            }
            match emoji::row_of(line) {
                Ok(Some(row)) => {
                    symbols.push(row.symbol);
                    aliases.push(row.alias);
                }
                Ok(None) => continue,
                Err(_) => report.skipped = report.skipped.saturating_add(1),
            }
        }
        if symbols.is_empty() {
            // Nothing usable: no domain is installed, and the table answers the
            // code exactly as it did before the call.
            return report;
        }
        let category = Category {
            code: Box::from(emoji::EMOJI_DOMAIN_CODE),
            label: Box::from(emoji::EMOJI_DOMAIN_LABEL),
            symbols,
            aliases: Some(aliases),
        };
        match self
            .categories
            .iter_mut()
            .find(|held| held.code == category.code)
        {
            Some(held) => {
                *held = category;
                report.overridden = report.overridden.saturating_add(1);
            }
            None => self.categories.push(category),
        }
        report.loaded = report.loaded.saturating_add(1);
        report
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

/// What [`SymbolTable::serve`] answered: the label and the symbols to serve.
///
/// The label borrows the table, as [`CategoryHit`] does; the symbols may be a
/// borrowed slice or a filtered list the answer owns, because the entries an
/// alias keeps are scattered through the table rather than one slice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Served<'table> {
    /// The label the category's candidates carry.
    pub label: &'table str,
    /// The symbols, in display order.
    pub symbols: ServedSymbols<'table>,
}

/// The symbol list one [`Served`] answer carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServedSymbols<'table> {
    /// The category's whole list, borrowed from the table -- the common answer,
    /// and the only one a table without the emoji domain ever gives.
    All(&'table [char]),
    /// The entries an emoji alias filter kept, in file order. Owned, because the
    /// kept entries are not contiguous in the table.
    Filtered(Vec<char>),
}

impl ServedSymbols<'_> {
    /// The list as a slice, whichever form it was served in.
    ///
    /// The panel reads the answer through this, so the candidate walk is one
    /// loop over symbols rather than one per form.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn as_slice(&self) -> &[char] {
        match self {
            Self::All(symbols) => symbols,
            Self::Filtered(symbols) => symbols,
        }
    }
}

/// One row of the index page: a domain's code and the label the page names it by.
///
/// Both borrow the table, the way a [`CategoryHit`] does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SymbolDomain<'table> {
    /// The code typed after the `v` that opens the domain.
    pub code: &'table str,
    /// The label the domain's candidates carry, and the text of the index page's
    /// candidate for it.
    pub label: &'table str,
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

/// The category the longest matching prefix names: the walk
/// [`SymbolTable::longest_prefix`] and [`SymbolTable::serve`] share.
///
/// `None` when no category's code prefixes `code`.
fn longest_prefix_category<'a>(table: &'a SymbolTable, code: &str) -> Option<&'a Category> {
    let mut best: Option<(&Category, usize)> = None;
    for category in &table.categories {
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
    best.map(|(category, _)| category)
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
                    // A plain document row carries no aliases: the override rule
                    // replaces the whole category, so a document that redefines
                    // the emoji domain's code takes the domain off.
                    aliases: None,
                };
                overridden = overridden.saturating_add(1);
            }
            None => categories.push(Category {
                code: row.code,
                label: row.label,
                symbols: row.symbols,
                aliases: None,
            }),
        }
    }
    (SymbolTable { categories }, overridden)
}

#[cfg(test)]
mod tests;
