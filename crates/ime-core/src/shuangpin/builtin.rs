//! The built-in layouts: the five published schemes this build ships, compiled
//! once from the layout documents embedded beside this file.
//!
//! Responsibility: hold the five data files (`data/*.toml`), compile them through
//! the same [`parse_layout`](super::layout::parse_layout) /
//! [`compile_layout`](super::layout::compile_layout) pair a user's layout goes
//! through, and hand the compiled tables out as `&'static` values the decode path
//! reads.
//!
//! # Why a one-time lazy compile
//!
//! The tables used to be `const` literals; they are now data, and parsing is not
//! a `const` operation. The compile therefore runs once, on the first
//! [`tables`](fn.tables.html) call, into a `OnceLock` every later call reads.
//! After that first call the decode hot path is the same table walk it always
//! was -- no allocation, no re-parse, no file -- and the load cost lands once per
//! process rather than once per keystroke.
//!
//! # Degradation contract
//!
//! A built-in document that its own validator refuses is a shipping defect (the
//! migration tests compile all five and would fail first). It is nonetheless
//! handled rather than panicked on: the scheme degrades to an
//! [`SchemeTable::empty`] table, which maps nothing, and the rewrite carries the
//! keystrokes through -- the user can still type, which is the one behaviour a
//! defect must never take away.

use std::sync::OnceLock;

use ime_types::SchemeId;

use super::layout::{compile_layout, parse_layout};
use super::SchemeTable;

/// The Xiaohe layout document.
const XIAOHE: &str = include_str!("data/xiaohe.toml");
/// The Ziranma layout document.
const ZIRANMA: &str = include_str!("data/ziranma.toml");
/// The Microsoft layout document.
const MICROSOFT: &str = include_str!("data/microsoft.toml");
/// The Sogou layout document.
const SOGOU: &str = include_str!("data/sogou.toml");
/// The Ziguang layout document.
const ZIGUANG: &str = include_str!("data/ziguang.toml");

/// The embedded documents and the scheme each one is published under, in
/// scheme-number order.
const EMBEDDED: [(SchemeId, &str); 5] = [
    (SchemeId::XIAOHE, XIAOHE),
    (SchemeId::ZIRANMA, ZIRANMA),
    (SchemeId::MICROSOFT, MICROSOFT),
    (SchemeId::SOGOU, SOGOU),
    (SchemeId::ZIGUANG, ZIGUANG),
];

/// The compiled built-in tables, filled on the first call and read afterwards.
static COMPILED: OnceLock<Vec<(SchemeId, SchemeTable)>> = OnceLock::new();

/// The table handed out for a scheme number the embedded set does not hold.
///
/// Unreachable for the numbers the scheme modules ask for, which are exactly the
/// ones [`EMBEDDED`] carries; it exists so that the accessor stays total without
/// a panic.
static UNMATCHED: OnceLock<SchemeTable> = OnceLock::new();

/// Returns the compiled built-in tables, in scheme-number order.
///
/// The first call parses and validates the five embedded documents; every later
/// call is a slice read.
///
/// # Panics
///
/// Never: a document its own validator refuses degrades to an empty table rather
/// than failing the load.
pub(super) fn tables() -> &'static [(SchemeId, SchemeTable)] {
    COMPILED.get_or_init(|| {
        EMBEDDED
            .iter()
            .map(|(id, text)| (*id, compile_one(*id, text)))
            .collect()
    })
}

/// Returns the built-in table a scheme number is published with.
///
/// # Parameters
///
/// - `id`: one of the five scheme numbers the embedded set carries.
///
/// # Returns
///
/// The table. A number the embedded set does not hold -- which the callers in
/// this module cannot produce -- answers an empty table instead of a panic, so
/// that the accessor is total.
///
/// # Panics
///
/// Never.
pub(super) fn table(id: SchemeId) -> &'static SchemeTable {
    tables()
        .iter()
        .find(|(known, _)| *known == id)
        .map(|(_, table)| table)
        .unwrap_or_else(|| {
            UNMATCHED.get_or_init(|| SchemeTable::empty(id))
        })
}

/// Parses, validates and compiles one embedded document.
///
/// A refusal is a shipping defect and degrades to an empty table; the warning is
/// the audit trail, and the migration tests compile all five documents and fail
/// long before a user could see this.
fn compile_one(id: SchemeId, text: &str) -> SchemeTable {
    match parse_layout(text).and_then(|document| compile_layout(&document, id)) {
        Ok(table) => table,
        Err(errors) => {
            tracing::warn!(
                scheme = id.value(),
                errors = errors.len(),
                "built-in layout rejected by its own validator; the scheme degrades to an empty table"
            );
            SchemeTable::empty(id)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every embedded document must load through the same validator a user's
    /// layout goes through. This is the test that fails, naming the document and
    /// the diagnostic, when a data edit breaks a published layout -- the runtime
    /// degrade path behind it is only ever reachable after this test has passed.
    #[test]
    fn test_builtin_documents_compile_through_the_shared_validator() {
        for (id, text) in EMBEDDED {
            match parse_layout(text) {
                Err(errors) => panic!("parse of {id:?} failed: {errors:?}"),
                Ok(document) => match compile_layout(&document, id) {
                    Err(errors) => panic!("compile of {id:?} failed: {errors:?}"),
                    Ok(_) => {}
                },
            }
        }
    }
}
