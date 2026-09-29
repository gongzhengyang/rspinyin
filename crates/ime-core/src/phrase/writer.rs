//! The write half of a phrase document: an entry becomes a row, a row becomes a document.
//!
//! Responsibility: [`phrase_row`] renders one entry as the line a document carries,
//! [`append_phrase`] adds that row to a document, and [`replace_phrase`] rewrites the row
//! of one key where it stands. The reader lives beside this module and owns what a
//! well-formed row is; this one owns producing them.
//!
//! Boundaries: pure, like the reader (0.4 rule 4). A document arrives as a `&str` and
//! leaves as a `String`; no file is opened, no directory is resolved and no clock is
//! read. The host layer owns the file -- it resolves the path, keeps the document private
//! and performs the one write -- which is what lets a saved phrase be appended without
//! reading the document back first.
//!
//! The writer checks what the reader checks, so a document it produces is one the reader
//! accepts and reads back as the entry that was written. Where the two differ it is a
//! property of the *line* rather than of the entry: a body holding a tab or a line break
//! is refused, because no single row can carry it, and the key is folded but not trimmed,
//! because a caller that builds a row is not a hand-edited document and a space in a key
//! is a defect it should see.

use ime_types::ImeError;

use super::{
    MAX_PHRASE_KEY_LEN, MAX_PHRASE_TEXT_LEN, PHRASES_FILE_KEY, RowFault, is_key_byte, row_of,
};

/// Renders one entry as the row a phrase document carries.
///
/// The writer's smallest piece, and the one a host that saves a phrase uses: the row is
/// rendered and appended to the document, and the document is never read to do it.
///
/// # Parameters
///
/// - `key`: the shortcut. It is folded to lower case exactly as the reader folds a row's
///   key, so what this returns is a row the reader reads the way the caller meant it.
/// - `text`: the text the key commits.
///
/// # Returns
///
/// The row: the key, a tab, the body and a line ending.
///
/// # Errors
///
/// [`ImeError::ConfigInvalid`] naming `phrases.file` when the entry is not one the reader
/// would accept -- a key outside the `a-z` and `'` alphabet or past
/// [`MAX_PHRASE_KEY_LEN`], an empty body or one past [`MAX_PHRASE_TEXT_LEN`], and a body
/// holding a tab or a line break, which no single row can carry. The reason names the
/// rule that was broken and never the entry, which is the user's own text.
///
/// # Panics
///
/// Never: the key and the body are read with a byte-wise check and a checked split.
///
/// # Examples
///
/// ```
/// use ime_core::phrase::phrase_row;
///
/// let row = phrase_row("RQ", "2026-09-29");
/// assert_eq!(row.ok().as_deref(), Some("rq\t2026-09-29\n"));
/// assert!(phrase_row("r q", "2026-09-29").is_err());
/// ```
pub fn phrase_row(key: &str, text: &str) -> Result<String, ImeError> {
    Ok(render_row(&checked_entry(key, text)?, text))
}

/// Appends one entry to a phrase document and returns the document.
///
/// Appended rather than inserted, which is what the reader's merge rule makes correct: a
/// key defined twice keeps its last definition, so a document that still holds an older
/// row of the same key above the new one reads exactly as the caller intends.
///
/// # Parameters
///
/// - `document`: the contents of a phrase document. A document whose last line carries no
///   line ending is given one, because appending straight onto that line would splice two
///   rows into one that neither the reader nor the caller would recognise.
/// - `key`, `text`: the entry, validated as [`phrase_row`] validates it.
///
/// # Returns
///
/// The document with the row appended.
///
/// # Errors
///
/// As [`phrase_row`]. The entry is checked before the document is touched, so a refused
/// entry leaves the caller's document exactly as it was.
///
/// # Panics
///
/// Never.
///
/// # Examples
///
/// ```
/// use ime_core::phrase::{PhraseTable, append_phrase};
///
/// let document = append_phrase("# my shortcuts\n", "rq", "2026-09-29");
/// assert_eq!(
///     document.ok().as_deref(),
///     Some("# my shortcuts\nrq\t2026-09-29\n")
/// );
///
/// // A key the document already holds is overridden, because the later row wins.
/// let document = append_phrase("rq\told\n", "rq", "new").unwrap_or_default();
/// let (table, _) = PhraseTable::load(&document, 100);
/// let hit = table.longest_match("rq", 0);
/// assert_eq!(hit.map(|hit| table.text(hit)), Some("new"));
/// ```
pub fn append_phrase(document: &str, key: &str, text: &str) -> Result<String, ImeError> {
    let row = phrase_row(key, text)?;
    let mut document = String::from(document);
    if !document.is_empty() && !document.ends_with('\n') {
        document.push('\n');
    }
    document.push_str(&row);
    Ok(document)
}

/// Rewrites a phrase document with one entry replaced, and returns the document.
///
/// The rewrite a caller wants when it holds the document: the first row of that key
/// becomes the new entry where it stood, so the comments and the order a user gave their
/// document survive, and a later row of the same key goes, because leaving it would let
/// it shadow the entry that just replaced it. A key the document does not hold is
/// appended, which is exactly [`append_phrase`].
///
/// A line the reader cannot use is left exactly as it is. The writer rewrites the entry
/// it was asked about and repairs nothing else: a row it cannot parse may be a mistake
/// the user is in the middle of fixing, and dropping it would lose text they typed.
///
/// # Parameters
///
/// - `document`: the contents of a phrase document.
/// - `key`, `text`: the entry that replaces whatever the document holds for `key`,
///   validated as [`phrase_row`] validates it.
///
/// # Returns
///
/// The rewritten document.
///
/// # Errors
///
/// As [`phrase_row`].
///
/// # Panics
///
/// Never.
///
/// # Examples
///
/// ```
/// use ime_core::phrase::replace_phrase;
///
/// let rewritten = replace_phrase("# a comment\nrq\told\n", "rq", "new");
/// assert_eq!(rewritten.ok().as_deref(), Some("# a comment\nrq\tnew\n"));
/// ```
pub fn replace_phrase(document: &str, key: &str, text: &str) -> Result<String, ImeError> {
    let key = checked_entry(key, text)?;
    let row = render_row(&key, text);
    let mut rewritten = String::with_capacity(document.len().saturating_add(row.len()));
    let mut is_replaced = false;
    for piece in document.split_inclusive('\n') {
        if row_key(piece).as_deref() != Some(key.as_str()) {
            rewritten.push_str(piece);
            continue;
        }
        // The first row of the key becomes the new entry where it stood; a later row of
        // the same key goes, because the reader keeps the last definition and a document
        // that held both would say two things about one key.
        if !is_replaced {
            rewritten.push_str(&row);
            is_replaced = true;
        }
    }
    if is_replaced {
        return Ok(rewritten);
    }
    append_phrase(document, &key, text)
}

/// Checks one entry against the rules the reader applies, and folds its key.
///
/// The rules are the reader's, so that a row this module writes is a row the reader
/// reads: the key is folded to lower case and must then be 1..=[`MAX_PHRASE_KEY_LEN`]
/// bytes of `a`..=`z` or the `'` that pins a syllable boundary, and the body must be
/// non-empty and at most [`MAX_PHRASE_TEXT_LEN`] bytes. The writer is stricter in the two
/// places where what is being written is a *line* rather than an entry: the key is not
/// trimmed, and a body holding a tab or a line break is refused, because such a row would
/// not come back as one entry.
///
/// # Returns
///
/// The folded key.
///
/// # Errors
///
/// [`ImeError::ConfigInvalid`] naming `phrases.file`, as [`phrase_row`] documents.
fn checked_entry(key: &str, text: &str) -> Result<String, ImeError> {
    let key = key.to_ascii_lowercase();
    let fault = if key.is_empty() || !key.bytes().all(is_key_byte) {
        Some(RowFault::KeyAlphabet)
    } else if key.len() > MAX_PHRASE_KEY_LEN {
        Some(RowFault::KeyTooLong)
    } else if text.is_empty() {
        Some(RowFault::EmptyText)
    } else if text.len() > MAX_PHRASE_TEXT_LEN {
        Some(RowFault::TextTooLong)
    } else if text
        .bytes()
        .any(|byte| matches!(byte, b'\t' | b'\n' | b'\r'))
    {
        Some(RowFault::NotOneLine)
    } else {
        None
    };
    match fault {
        Some(fault) => Err(ImeError::ConfigInvalid {
            key: String::from(PHRASES_FILE_KEY),
            reason: fault.write_reason(),
        }),
        None => Ok(key),
    }
}

/// Renders one row from an entry that has already been checked.
fn render_row(key: &str, text: &str) -> String {
    let mut row = String::with_capacity(key.len().saturating_add(text.len()).saturating_add(2));
    row.push_str(key);
    row.push('\t');
    row.push_str(text);
    row.push('\n');
    row
}

/// The key of the row one line of a document carries, or `None` when it carries none.
///
/// The reader's own rule, asked of the reader's own parser: a line the reader cannot use
/// answers `None` here as well, so the two can never disagree about which row a key
/// belongs to. The line ending is stripped first, because the reader reads lines without
/// one and a body that kept its newline would be a byte too long for the same row.
fn row_key(piece: &str) -> Option<String> {
    let line = piece.strip_suffix('\n').unwrap_or(piece);
    let line = line.strip_suffix('\r').unwrap_or(line);
    match row_of(line) {
        Ok(Some(row)) => Some(row.key.into()),
        _ => None,
    }
}
