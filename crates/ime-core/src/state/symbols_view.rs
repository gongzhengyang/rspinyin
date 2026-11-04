//! The index page: the panel's self-describing view of its own domains.
//!
//! Responsibility: the two pure pieces of the `vhelp` view -- the input that opens
//! the page, and the candidates it serves, one per domain of the symbol table with
//! the trigger key as the annotation. The transitions call into here from the arms
//! that serve and select the page; nothing in this module touches session state,
//! and the writer fills the workspace's candidate slots the way a decode's refresh
//! does, so no list is copied and no decode runs.
//!
//! Boundaries: the index code is a fixed built-in, not configuration. It is
//! reserved against the exact string typed after the `v`: a category whose code
//! prefixes it stays reachable (`vh` serves the shipped sequence row), and a
//! category named exactly `help` would be shadowed -- the page has to win the
//! exact code, because the shipped table carries an `h` category and `h` is a
//! prefix of `help`, so the category-first rule would leave the page unreachable.

use ime_types::{Candidate, CandidateSource, DecodeResult};

use crate::symbols::SymbolTable;

use super::machine::{Ctx, Effect, Session};

/// The code the index page answers to, typed after the panel's `v`.
pub(in crate::state) const INDEX_CODE: &str = "help";

/// The input on screen while the index page is up: the panel's prefix plus the
/// code.
const INDEX_INPUT: &str = "vhelp";

/// Whether the input on screen is the index page's.
///
/// The exact string only: `vhel` is a prefix category's business and `vhelpx` is
/// the one it extends, so the page opens on the code typed exactly, as
/// [`super::Session::symbol_table_serves`] reserves it.
pub(in crate::state) fn is_open(raw: &str) -> bool {
    raw == INDEX_INPUT
}

/// Writes the index page's candidates: one per domain, in table order.
///
/// The candidate text is the domain's label -- what the user reads -- and the
/// annotation is the trigger key, `v` plus the code, which is what the page
/// promises: domain name, then the key that opens it. The slots of a longer
/// previous list are reused the way a decode reuses them, and the list is
/// truncated to the domain count, so a page over a smaller table starts empty.
/// Selecting a candidate is the domain jump the transitions own; the candidate's
/// position in this order is what names the domain it selects.
///
/// # Panics
///
/// Never: every index is written through a checked conversion.
pub(in crate::state) fn write_candidates(
    out: &mut DecodeResult,
    table: &SymbolTable,
    consumed: u16,
) {
    for (position, domain) in table.domains().enumerate() {
        // The display number is the position, 1-based, and the annotation the
        // trigger key the page advertises: the `v` plus the code the domain is
        // opened by.
        let index = u16::try_from(position)
            .unwrap_or(u16::MAX)
            .saturating_add(1);
        let key = format!("v{}", domain.code);
        match out.candidates.get_mut(position) {
            Some(slot) => {
                slot.index = index;
                slot.text.clear();
                slot.text.push_str(domain.label);
                slot.annotation = Some(key);
                slot.source = CandidateSource::Symbol;
                slot.score = 0.0;
                slot.consumed_syllables = consumed;
            }
            None => out.candidates.push(Candidate {
                index,
                text: String::from(domain.label),
                annotation: Some(key),
                source: CandidateSource::Symbol,
                score: 0.0,
                consumed_syllables: consumed,
            }),
        }
    }
    let count = table.len();
    out.candidates.truncate(count);
}

impl Session {
    /// Jumps from the index page into the highlighted domain.
    ///
    /// The selection the page exists for: the panel's input becomes the selected
    /// domain's trigger key -- the `v` plus the code the annotation promised --
    /// and the refresh that follows serves that domain's symbols through the
    /// ordinary panel path. Paging, the highlight and the number keys work from
    /// there unchanged, which is the whole reason the page rides the candidate
    /// grid instead of a UI of its own. Nothing is committed and no frequency is
    /// learned: a domain is a place the panel goes, not text the user typed.
    ///
    /// The candidate's position names the domain: the index page is written one
    /// candidate per domain in table order, so the highlight's index is the
    /// domain's position, and the highlight that follows the jump is reconciled
    /// against the new list like any refresh's.
    ///
    /// This method lives beside the page's writer rather than in the transitions
    /// file, which is the split the line limit asks of one type wearing two hats:
    /// the transitions keep the routing arms, this module keeps the panel's own
    /// view machinery.
    ///
    /// # Panics
    ///
    /// Never: the trigger key's characters are of the alphabet the input buffer
    /// accepts, and a push the buffer refused anyway is diagnosed, not ignored.
    pub(super) fn jump_to_domain(&mut self, ctx: &mut Ctx<'_>) {
        let Some(table) = ctx.env.symbols else {
            return;
        };
        let Some(domain) = table.domains().nth(usize::from(self.paging.highlight)) else {
            return;
        };
        // The input is replaced whole: whatever of `vhelp` is still on screen is
        // the page's, and the trigger key starts from the bare `v`. The pushes
        // cannot fail on a built table -- the code was validated into the
        // alphabet at parse time -- so a refusal here is a defect and is
        // reported as one.
        self.buf.clear();
        for ch in "v".chars().chain(domain.code.chars()) {
            if let Err(err) = self.buf.push_char(ch) {
                ctx.push(Effect::Diagnose(err));
                return;
            }
        }
        self.refresh(ctx.env);
        self.emit_update(ctx);
    }
}
