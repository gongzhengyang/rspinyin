//! The checklist the visual audit is answered against.
//!
//! Responsibility: hold every item of the audit, in the specification's own order, and nothing
//! else. The items live in two modules because the specification's own visual baseline is two
//! kinds of statement -- the sizes and materials that make up the window's geometry, and the
//! colours, motion, states and degradations that make up what it looks like over time -- and an
//! item's dimension is the table it came from.
//!
//! # What the checklist is not
//!
//! It is not a second copy of the specification's numbers: an item quotes the row it audits so a
//! reviewer can read both, and the citation is the row's own label rather than a line number, so
//! the quote and the document cannot drift apart silently. It is also not an assertion: nothing
//! here reads a PNG or a metric, and the module is deliberately free of any way to do so -- what
//! decides an item is a reviewer's answer, and what judges the answer is
//! [`super::audit`].
//!
//! # Why the items are assembled rather than declared in one table
//!
//! A single table of eighty items in one file would be a file nobody reads and a merge conflict
//! every time the specification gains a row. Split by the document's own sections, a change to
//! 3.2 touches the colour list and nothing else, and the coverage tests can assert the checklist
//! against the parsed document one table at a time.

mod appearance;
mod geometry;

use super::CheckItem;

/// Every item of the visual audit, in the order the prompt presents them.
///
/// The order is the document's, so a reviewer can work down the prompt and up
/// `docs/dev/features.md` at the same time.
///
/// # Panics
///
/// Never.
pub fn check_items() -> Vec<CheckItem> {
    let mut items = geometry::items();
    items.extend(appearance::items());
    items
}
