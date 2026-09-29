//! Locator self-healing: finding again what a locator names, from the specification.
//!
//! Responsibility: when a locator no longer finds what it names -- a metrics constant was
//! renamed, or a point the specification's own arithmetic produces has moved -- find it again
//! from the specification and report what had to change. Nothing here reads a file, opens a
//! display or asserts a pixel: both sources arrive as values the caller read, which is what lets
//! the whole module be tested with no display server present.
//!
//! # Why a locator can go missing at all
//!
//! The candidate window has no DOM, no widget tree and no accessibility tree: the plugin draws
//! it itself, pixel by pixel. A case therefore locates what it aims at by three kinds of real
//! identifier, and only two of them can go stale:
//!
//! | Locator | What it names | How it goes stale |
//! |---|---|---|
//! | a metrics constant | one declaration of the `.slint` metrics block | the declaration was renamed |
//! | a point of the hit map | one cell of the candidate grid, in container pixels | a constant its arithmetic is built on was renamed |
//! | a field of a frame snapshot | one field of the frozen frame contract | not at all |
//!
//! The frame side needs no healing and gets none. A frame's field is a Rust field: renaming one
//! stops the harness compiling rather than stopping a case at run time, and the snapshot file
//! carries a format version that refuses a shape it does not know. A locator that "recovered"
//! there would be covering a compile error, so the two channels stay apart.
//!
//! # The one thing healing must never do
//!
//! Healing covers a **name** change and nothing else. A value that changed is not a rename: it
//! means the specification or the implementation was edited, and the harness has to fail and say
//! so rather than follow the new number. [`resolve`] therefore hands back a constant whose value
//! disagrees with the specification **unhealed**, and the metrics channel's cross-check reports
//! it as a mismatch. If healing covered a value change, the one class of regression the
//! specification exists to catch would be the one class it could not.
//!
//! # How a missing name is re-derived
//!
//! The specification fixes the value; the metrics block declares it under a name. When the name
//! a locator used is gone, the constant is found again by two exact tests, and never by
//! similarity:
//!
//! 1. the candidate's value is the value the specification's row states, in the unit the row
//!    states it in -- so a constant whose value changed as well is not a candidate, which is what
//!    keeps a rename from masking a metric change;
//! 2. the words of the old name appear, in order and adjacent, inside the candidate's -- the
//!    rename the project's own vocabulary produces, `grid-gap` to `candidate-grid-gap`, and never
//!    a name that merely looks similar.
//!
//! Exactly one candidate must survive both tests. None, or more than one, is a refusal rather than
//! a guess: [`HealError::Unresolvable`] when the name cannot be re-derived at all, and
//! [`HealError::Drifted`] when one declaration took the name over but carries another value, which
//! is the source having moved rather than the locator having lost its constant.
//!
//! # What a recovery leaves behind
//!
//! Every recovery produces a [`HealRecord`] carrying what the case named, what the source
//! declares now and the specification rows the derivation is anchored on, and [`HealLog`] hands
//! those records to the evidence archive as the entries `assertions.json` holds and renders the
//! summary a batch's `index.md` carries. A recovery that left no record would hide exactly the
//! drift the record exists to make reviewable, so nothing here heals silently.
//!
//! # What a pass may never change
//!
//! A recovery re-derives a locator; it never touches what the locator is aimed at. This module
//! therefore reads its two sources and writes nothing at all -- no file, no patch, no source --
//! and the only value a heal ever carries is one the specification itself states, because a
//! candidate that disagrees with its row is not a candidate. A heal can thus never lower a bar:
//! the most it can do is find again, under another name, the constant the specification already
//! fixed. A value that moved is refused, and the refusal says which side moved -- see
//! [`HealError`] -- so a case that fails can be attributed to the locator or to the product
//! rather than merely reported.
//!
//! The line that keeps a pass from reaching further than a test script is drawn by the guard and
//! not by a convention: `crate::testd::guard` holds the prefixes a recovery may write to, the
//! files whose content is frozen for the whole run, and the audit that judges a pass by what it
//! did to the tree, and it fails closed when it cannot judge at all. Nothing in this module can
//! widen that, because nothing here writes.
//!
//! # Modules
//!
//! [`binding`] locates a constant of the metrics block and re-derives a name that moved,
//! [`words`] holds the exact test a rename has to pass, [`hit`] locates a point of the hit map
//! from the constants the specification fixes, [`report`] holds the record every recovery leaves
//! behind, and [`error`] the refusals.

// The module is exercised by the tests beside it and is not yet reachable from `xtask`'s
// subcommand tree, which lives in `xtask/src/main.rs` and in `xtask/src/testd/mod.rs` -- two
// files this module does not own. Until that wiring lands, every item here is reported as dead
// code in a non-test build, and the attribute goes away with those lines.
//
// `unused_imports` is covered by the same reasoning: the surface of this module is named by
// nothing in the crate yet.
#![allow(dead_code, unused_imports)]

mod binding;
mod error;
mod hit;
mod report;
mod words;

#[cfg(test)]
mod tests;

pub use self::binding::{ConstRef, ResolvedConst, SpecAnchor, resolve};
pub use self::error::HealError;
pub use self::hit::{HitQuery, HitRef, ResolvedHit, resolve_hit};
pub use self::report::{AnchorView, HEAL_MARKER, HealCause, HealLog, HealRecord};
