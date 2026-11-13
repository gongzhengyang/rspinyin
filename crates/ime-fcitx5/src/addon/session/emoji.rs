//! The v-mode panel's emoji domain: the extra symbol-domain document the data
//! directory may carry.
//!
//! Responsibility: read `emoji.tsv` beside `symbols.tsv` and add it as a domain,
//! carrying the document's own degradation story so the parent module's symbol
//! read stays about the categories. Absent is the ordinary case for an install
//! that predates the domain; a document that cannot be read is reported the way
//! the dictionary's own failures are; and rows the reader refused are a packaging
//! fault in shipped data, reported while the entries that were read still serve.

use std::path::PathBuf;

use ime_core::symbols::SymbolTable;
use ime_types::ImeError;

use crate::addon::{layout, report_step_failure};

/// The name the emoji domain document is installed under inside the data
/// directory.
///
/// The document the v-mode panel's emoji domain reads; absent is the ordinary
/// case for an install that predates the domain, and `ve` decodes exactly as it
/// did before the domain existed.
const EMOJI_DATA_FILE: &str = "emoji.tsv";

/// The path the emoji domain document is read from, or `None` when the layout is
/// unknown.
///
/// # Panics
///
/// Never.
fn emoji_path() -> Option<PathBuf> {
    layout::layout().map(|paths| paths.data_dir.join(EMOJI_DATA_FILE))
}

/// Adds the emoji domain to `table`, when the data directory carries the document.
///
/// Absent is the ordinary case for an install that predates the domain, so it
/// reports nothing; a document that cannot be read is reported without stopping
/// the sequence, and rows the reader refused are a packaging fault in shipped
/// data, reported while the entries that were read still serve.
///
/// # Panics
///
/// Never.
pub(super) fn install_emoji_domain(table: &mut SymbolTable) {
    let Some(path) = emoji_path() else {
        return;
    };
    let document = match std::fs::read_to_string(&path) {
        Ok(document) => document,
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => return,
        Err(cause) => {
            let error = ImeError::DictUnavailable {
                path: path.clone(),
                cause: cause.into(),
            };
            report_step_failure("emoji-domain", &error);
            return;
        }
    };
    let report = table.add_emoji_domain(&document);
    if report.skipped > 0 {
        let error = ImeError::ConfigInvalid {
            key: String::from("emoji.file"),
            reason: format!(
                "{} row(s) of the installed emoji domain could not be used",
                report.skipped
            ),
        };
        report_step_failure("emoji-domain", &error);
    }
}
