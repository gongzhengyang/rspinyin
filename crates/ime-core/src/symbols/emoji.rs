//! The emoji domain: the alias-matched entries of the symbol table.
//!
//! Responsibility: read one emoji document -- rows of `character TAB pinyin-alias` --
//! and answer the alias filter a panel lookup asks. The reader is this module's
//! [`row_of`]; the filter is [`filter`], which keeps the entries whose alias starts
//! with what the user typed past the domain's code, in file order, homophones side
//! by side.
//!
//! Boundaries: pure, like the table it feeds. The document's text arrives as a
//! `&str`; the loader that owns the bytes is the host's. The code and the label the
//! domain is known by are constants here rather than document fields, because they
//! are the panel's own naming -- the trigger the feature documents, not data a
//! file carries.

/// The code the emoji domain is keyed by, typed after the `v`.
///
/// The feature's documented trigger: `ve` opens the emoji list, and letters typed
/// past the `e` filter it by alias.
pub(super) const EMOJI_DOMAIN_CODE: &str = "e";

/// The label the emoji domain's candidates carry, and the text of the index page's
/// candidate for it.
pub(super) const EMOJI_DOMAIN_LABEL: &str = "表情";

/// Most entries the emoji domain may hold.
///
/// The domain is one flat list the alias filter is meant to narrow; the shipped
/// document sits well inside the bound, and a document past it is not an emoji
/// list but something else being loaded as one -- refused, with the extra rows
/// counted as skips.
pub const MAX_EMOJI_PER_DOMAIN: usize = 512;

/// Longest alias one emoji entry may carry, in bytes.
///
/// A pinyin alias of a common emoji is a syllable or three; the bound keeps a
/// line of prose pasted into the document from being read as an alias.
pub const MAX_EMOJI_ALIAS_LEN: usize = 16;

/// One emoji row the reader kept: the character and the pinyin alias it answers to.
pub(super) struct EmojiRow {
    /// The character the candidate text is.
    pub(super) symbol: char,
    /// The pinyin alias, lower case, that matches the entry.
    pub(super) alias: Box<str>,
}

/// Reads one line of an emoji document.
///
/// # Returns
///
/// `Ok(None)` for a blank line and for a comment, `Ok(Some(row))` for a usable
/// row, and the fault of the first row that is not: a line that is not two
/// tab-separated fields, a character field that is empty or holds more than one
/// code point, or an alias that is empty, too long, or outside `a`..=`z`.
///
/// # Panics
///
/// Never.
pub(super) fn row_of(line: &str) -> Result<Option<EmojiRow>, String> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return Ok(None);
    }
    let mut fields = line.split('\t');
    let Some(symbol_field) = fields.next() else {
        return Err(String::from("not two tab-separated fields"));
    };
    let Some(alias_field) = fields.next() else {
        return Err(String::from("not two tab-separated fields"));
    };
    if fields.next().is_some() {
        return Err(String::from("not two tab-separated fields"));
    }
    let symbol_field = symbol_field.trim();
    // Exactly one code point: the candidate text is one character, which is what
    // lets the entries reuse the panel's single-character slot unchanged. A
    // sequence with a joiner or a variant selector is a cluster, not a character,
    // and is refused rather than half-served.
    let mut chars = symbol_field.chars();
    let Some(symbol) = chars.next() else {
        return Err(String::from("the character field is empty"));
    };
    if chars.next().is_some() {
        return Err(String::from(
            "the character field holds more than one code point",
        ));
    }
    let alias = alias_field.trim();
    if alias.is_empty()
        || alias.len() > MAX_EMOJI_ALIAS_LEN
        || !alias.bytes().all(|byte| byte.is_ascii_lowercase())
    {
        return Err(String::from("the alias is empty, too long, or outside a-z"));
    }
    Ok(Some(EmojiRow {
        symbol,
        alias: Box::from(alias),
    }))
}

/// Keeps the entries whose alias starts with `query`, in file order.
///
/// `aliases` and `symbols` are the aligned columns of one category, built that
/// way by [`SymbolTable::add_emoji_domain`](super::SymbolTable::add_emoji_domain);
/// the zip pairs them and stops at the shorter, which cannot happen for a table
/// the builder produced. The match is exact-case, like the code match above it:
/// an alias that is not typed exactly is an alias that does not match, and the
/// next keystroke narrows the list instead.
pub(super) fn filter(aliases: &[Box<str>], symbols: &[char], query: &str) -> Vec<char> {
    aliases
        .iter()
        .zip(symbols)
        .filter(|(alias, _)| alias.as_ref().starts_with(query))
        .map(|(_, symbol)| *symbol)
        .collect()
}
