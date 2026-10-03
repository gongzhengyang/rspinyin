//! Candidate paging: the page the window shows and the candidate it highlights.
//!
//! Responsibility: own the two numbers the candidate grid is drawn from -- which
//! page is visible and which candidate carries the highlight -- and keep them
//! consistent with the candidate list after every re-decode, page flip, highlight
//! move and page-size change.
//!
//! Boundaries: pure arithmetic over the candidate list it is handed. This module
//! never decodes, never renders, never reads a clock and never touches the input
//! buffer, so a test drives it from hand-built candidates.
//!
//! # Indexing
//!
//! `highlight` is a **zero-based global candidate index**: the position of the
//! candidate in the whole decoded list, not in the page on show. The mouse events
//! of the frozen contract (`UiEvent::Select` / `UiEvent::Hover`) carry the same
//! numbering, which is what makes a click and a number key name the same candidate.
//! [`Paging::local_index`] converts to the position within the page for anything
//! that draws or hit-tests the grid.
//!
//! # The five-page window
//!
//! `ASM-07` bounds the grid at five pages of nine candidates. The reach is capped
//! at [`MAX_PAGES`] pages of whatever the page size is, so a page size of five
//! reaches twenty-five candidates and not forty-five; the surplus stays decoded but
//! is not reachable, and the status strip shows the page count instead of offering a
//! "next page" that leads nowhere.
//!
//! # No wrapping
//!
//! [`Paging::flip`] stops at the first and last page and reports `false` rather than
//! wrapping around. Wrapping would send a user who held a page key from the last page
//! back to the first, and a user who scrolled up from the first page to the last; the
//! window's dismiss-on-scroll-up behaviour (`DismissReason::ScrollUpEmpty`) depends on
//! the boundary being an event rather than a wrap.

use ime_types::{Candidate, PageDir, PageState};

/// Candidates per page when no configuration says otherwise: `ui.max_per_row`'s
/// documented default.
pub const DEFAULT_PAGE_SIZE: u8 = 5;

/// Smallest page size a configuration may ask for (`ui.max_per_row`).
pub const MIN_PAGE_SIZE: u8 = 3;

/// Largest page size a configuration may ask for (`ui.max_per_row`).
pub const MAX_PAGE_SIZE: u8 = 9;

/// Most pages the window pages through (`ASM-07`): five pages of nine candidates.
pub const MAX_PAGES: u8 = 5;

/// Most candidates the window can reach: [`MAX_PAGES`] pages of [`MAX_PAGE_SIZE`].
pub const MAX_REACHABLE_CANDIDATES: u16 = MAX_PAGES as u16 * MAX_PAGE_SIZE as u16;

/// Where the candidate grid is scrolled to and what it highlights.
///
/// The three fields are public because the engine reads them to build a frame and
/// the diagnostics read them to describe a session; every mutation goes through the
/// methods below, which keep the invariant that the highlight is inside the page on
/// show and that the page is inside the reachable window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Paging {
    /// Zero-based index of the page on show.
    pub page: u8,
    /// Candidates per page, from `ui.max_per_row`; clamped into
    /// [`MIN_PAGE_SIZE`]`..=`[`MAX_PAGE_SIZE`].
    pub page_size: u8,
    /// Zero-based global index of the highlighted candidate; see the module
    /// documentation for the numbering.
    pub highlight: u16,
}

impl Default for Paging {
    /// The state of a session that has not decoded anything yet: page zero, the
    /// default page size, the first candidate highlighted.
    fn default() -> Self {
        Self::new()
    }
}

impl Paging {
    /// Creates the paging state of an empty session.
    pub const fn new() -> Self {
        Self {
            page: 0,
            page_size: DEFAULT_PAGE_SIZE,
            highlight: 0,
        }
    }

    /// Creates the paging state of an empty session that shows `page_size`
    /// candidates per page, clamped into the configurable range.
    pub fn with_page_size(page_size: u8) -> Self {
        let mut paging = Self::new();
        paging.set_page_size(page_size);
        paging
    }

    /// Adopts a new page size, clamped into [`MIN_PAGE_SIZE`]`..=`[`MAX_PAGE_SIZE`].
    ///
    /// The caller reconciles afterwards: this only records the size, because the
    /// page and the highlight can only be recomputed against a candidate list.
    pub fn set_page_size(&mut self, page_size: u8) {
        self.page_size = page_size.clamp(MIN_PAGE_SIZE, MAX_PAGE_SIZE);
    }

    /// Returns the page size as a count that is never zero.
    ///
    /// The field is public, so a caller could have written a zero into it; every
    /// division and every page boundary goes through here rather than trusting it,
    /// which keeps a broken field from turning into a division by zero.
    fn size(&self) -> u16 {
        u16::from(self.page_size.max(1))
    }

    /// Returns how many pages the window offers for `total` candidates.
    ///
    /// At least one page for a non-empty list, zero for an empty one, and never more
    /// than [`MAX_PAGES`].
    pub fn page_count(&self, total: u16) -> u8 {
        if total == 0 {
            return 0;
        }
        let pages = total.div_ceil(self.size());
        u8::try_from(pages).unwrap_or(u8::MAX).min(MAX_PAGES)
    }

    /// Returns the global index of the first candidate on the page on show.
    pub fn page_start(&self) -> u16 {
        u16::from(self.page).saturating_mul(self.size())
    }

    /// Returns the global index one past the last candidate on the page on show.
    ///
    /// Never past `total`, so a partly filled last page ends where the list does.
    pub fn page_end(&self, total: u16) -> u16 {
        self.page_start().saturating_add(self.size()).min(total)
    }

    /// Returns the position of the highlighted candidate within the page on show.
    pub fn local_index(&self) -> u16 {
        self.highlight.saturating_sub(self.page_start())
    }

    /// Returns the page-local position of the keyboard highlight, or `None` when the
    /// page on show holds no candidate for it.
    ///
    /// This is [`Paging::local_index`]'s saturating arithmetic, finished into an answer:
    /// a highlight at or past the page's end names no candidate on the page, and the
    /// frame carries `None` for that instead of a position the grid cannot draw. The
    /// view treats `None` as "hide the ring", never as "keep the previous one" -- a ring
    /// that outlives its candidate is the defect this conversion exists to prevent.
    pub fn highlight_position_in_page(&self, total: u16) -> Option<u16> {
        let local = self.local_index();
        let page_len = self.page_end(total).saturating_sub(self.page_start());
        (local < page_len).then_some(local)
    }

    /// Returns the paging part of the frame the window draws.
    ///
    /// `current` is one-based, which is what the window shows; an empty candidate
    /// list reports `current: 0, total: 0`.
    pub fn page_state(&self, total: u16) -> PageState {
        let total_pages = self.page_count(total);
        PageState {
            current: if total_pages == 0 {
                0
            } else {
                self.page.saturating_add(1)
            },
            total: total_pages,
            page_size: self.page_size,
        }
    }

    /// Returns the global index a number key selects, or `None` when the digit names
    /// no candidate on the page on show.
    ///
    /// `digit` is the key as the user sees it: one-based, `1` to `9`. A digit past
    /// the end of a partly filled page, and `0` (which is a page key or a
    /// passthrough, never a selection), answer `None`, and the caller hands the key
    /// back to the host instead of swallowing it.
    pub fn digit_target(&self, digit: u8, total: u16) -> Option<u16> {
        if !(1..=MAX_PAGE_SIZE).contains(&digit) {
            return None;
        }
        let index = self.page_start().checked_add(u16::from(digit) - 1)?;
        (index < total && index < self.page_end(total)).then_some(index)
    }

    /// Turns to the next or previous page, and reports whether the page moved.
    ///
    /// The highlight lands on the first candidate of the new page when paging
    /// forward and on the last when paging back, so it is always on the page the
    /// window shows and `Space` commits something the user can see.
    ///
    /// At the first page a backward flip and at the last page a forward flip leave
    /// the state untouched and answer `false`: the pages do not wrap.
    pub fn flip(&mut self, dir: PageDir, total: u16) -> bool {
        let pages = self.page_count(total);
        if pages == 0 {
            return false;
        }
        let last = pages - 1;
        let target = match dir {
            PageDir::Next => self.page.saturating_add(1).min(last),
            PageDir::Prev => self.page.saturating_sub(1),
        };
        if target == self.page {
            return false;
        }
        self.page = target;
        self.highlight = match dir {
            PageDir::Next => self.page_start(),
            PageDir::Prev => self.page_end(total).saturating_sub(1),
        };
        true
    }

    /// Moves the highlight by `delta` candidates, and reports whether it moved.
    ///
    /// The move stays inside the reachable window: it stops at the last candidate
    /// the five-page cap allows rather than at the end of the decoded list. Crossing
    /// a page boundary turns the page, so the highlight is always visible, and it
    /// lands on the first or last candidate of the new page for the same reason
    /// [`Paging::flip`] does.
    ///
    /// A `delta` of zero, an empty list, and a move that would leave the window all
    /// answer `false` without changing anything.
    pub fn move_highlight(&mut self, delta: i8, total: u16) -> bool {
        if delta == 0 || total == 0 {
            return false;
        }
        let pages = self.page_count(total);
        if pages == 0 {
            return false;
        }
        let last = u16::from(pages)
            .saturating_mul(self.size())
            .saturating_sub(1);
        let current = self.highlight.min(last).min(total - 1);
        let target = if delta < 0 {
            current.saturating_sub(u16::from(delta.unsigned_abs()))
        } else {
            current
                .saturating_add(u16::from(delta as u8))
                .min(last)
                .min(total - 1)
        };
        if target == current {
            return false;
        }
        self.highlight = target;
        self.page = u8::try_from(target / self.size()).unwrap_or(0);
        true
    }

    /// Puts the highlight back on the candidate it was on, against a new list.
    ///
    /// The decoded list changes on every keystroke, and the highlight should stay on
    /// the word the user had chosen rather than jump back to the first candidate, so
    /// the candidate is looked up by its text in the new list. A hit moves the
    /// highlight there and turns to the page that holds it; a miss, and a first
    /// decode with nothing to look for, put the highlight back on the first
    /// candidate of the first page.
    ///
    /// Both the page and the highlight are clamped to the new list afterwards, which
    /// is what makes a page-size change land on a visible candidate: the text is
    /// found at its global position and the page is recomputed from the new size.
    pub fn reconcile(&mut self, prev_text: Option<&str>, candidates: &[Candidate]) {
        let total = u16::try_from(candidates.len()).unwrap_or(u16::MAX);
        let found = prev_text.and_then(|text| candidates.iter().position(|held| held.text == text));
        match found {
            Some(position) => {
                self.highlight = u16::try_from(position).unwrap_or(u16::MAX);
                self.page = u8::try_from(self.highlight / self.size()).unwrap_or(u8::MAX);
            }
            None => self.reset(),
        }
        self.clamp(total);
    }

    /// Puts the grid back on the first candidate of the first page.
    pub fn reset(&mut self) {
        self.page = 0;
        self.highlight = 0;
    }

    /// Clamps the page and the highlight into a list of `total` candidates.
    ///
    /// The page is clamped first, so the highlight is clamped against the page that
    /// is actually shown; that ordering is what keeps a shortened list from leaving
    /// the highlight on a page that no longer exists.
    fn clamp(&mut self, total: u16) {
        let pages = self.page_count(total);
        if pages == 0 {
            self.reset();
            return;
        }
        self.page = self.page.min(pages - 1);
        let start = self.page_start();
        let end = self.page_end(total).saturating_sub(1);
        // `start <= end` holds for every page the count allows, so the clamp cannot
        // panic; the `max` is there to keep that true if the invariant ever breaks.
        self.highlight = self.highlight.clamp(start, end.max(start));
    }
}

#[cfg(test)]
mod tests {
    use ime_types::CandidateSource;

    use super::*;

    /// Builds a candidate list of `count` entries whose texts are `w0`, `w1`, ...
    fn candidates(count: u16) -> Vec<Candidate> {
        (0..count)
            .map(|position| Candidate {
                index: position.saturating_add(1),
                text: format!("w{position}"),
                annotation: None,
                source: CandidateSource::Dict,
                score: 0.0,
                consumed_syllables: 1,
            })
            .collect()
    }

    #[test]
    fn test_new_paging_starts_on_the_first_page_with_the_first_candidate() {
        let paging = Paging::new();
        assert_eq!(paging.page, 0);
        assert_eq!(paging.page_size, DEFAULT_PAGE_SIZE);
        assert_eq!(paging.highlight, 0);
        assert_eq!(paging, Paging::default());
        assert_eq!(paging.local_index(), 0);
    }

    #[test]
    fn test_page_count_rounds_up_and_caps_at_five_pages() {
        let paging = Paging::with_page_size(9);
        assert_eq!(paging.page_count(0), 0);
        assert_eq!(paging.page_count(1), 1);
        assert_eq!(paging.page_count(9), 1);
        assert_eq!(paging.page_count(10), 2);
        assert_eq!(paging.page_count(45), 5);
        assert_eq!(paging.page_count(u16::MAX), MAX_PAGES);

        // A page size of five reaches five pages of five, not nine pages: the cap is
        // on the page count, which is what ASM-07 bounds.
        let narrow = Paging::with_page_size(5);
        assert_eq!(narrow.page_count(45), MAX_PAGES);
        assert_eq!(u16::from(MAX_PAGES) * u16::from(narrow.page_size), 25);
    }

    #[test]
    fn test_page_size_is_clamped_into_the_configurable_range() {
        assert_eq!(Paging::with_page_size(0).page_size, MIN_PAGE_SIZE);
        assert_eq!(Paging::with_page_size(2).page_size, MIN_PAGE_SIZE);
        assert_eq!(Paging::with_page_size(3).page_size, 3);
        assert_eq!(Paging::with_page_size(9).page_size, 9);
        assert_eq!(Paging::with_page_size(200).page_size, MAX_PAGE_SIZE);
    }

    #[test]
    fn test_flip_forward_at_the_last_page_stays_and_reports_false() {
        let mut paging = Paging::with_page_size(9);
        assert!(paging.flip(PageDir::Next, 10));
        assert_eq!(paging.page, 1);
        assert_eq!(paging.highlight, 9);
        // The second page is the last one, and the pages do not wrap.
        assert!(!paging.flip(PageDir::Next, 10));
        assert_eq!(paging.page, 1);
        assert_eq!(paging.highlight, 9);
    }

    #[test]
    fn test_flip_back_at_the_first_page_stays_and_reports_false() {
        let mut paging = Paging::with_page_size(9);
        assert!(!paging.flip(PageDir::Prev, 45));
        assert_eq!(paging.page, 0);
        assert_eq!(paging.highlight, 0);

        assert!(paging.flip(PageDir::Next, 45));
        assert!(paging.flip(PageDir::Prev, 45));
        assert_eq!(paging.page, 0);
        // Back onto the first page lands on its last candidate, which is the last of
        // the page that was left.
        assert_eq!(paging.highlight, 8);
    }

    #[test]
    fn test_flip_forward_lands_on_the_first_candidate_of_the_new_page() {
        let mut paging = Paging::with_page_size(5);
        paging.highlight = 3;
        assert!(paging.flip(PageDir::Next, 45));
        assert_eq!(paging.page, 1);
        assert_eq!(paging.highlight, 5);
        assert_eq!(paging.local_index(), 0);
        assert_eq!(paging.page_start(), 5);
        assert_eq!(paging.page_end(45), 10);
    }

    #[test]
    fn test_flip_on_a_single_candidate_does_nothing() {
        let mut paging = Paging::new();
        assert!(!paging.flip(PageDir::Next, 1));
        assert!(!paging.flip(PageDir::Prev, 1));
        assert_eq!(paging, Paging::new());
        // An empty list has no page to turn to at all.
        assert!(!paging.flip(PageDir::Next, 0));
    }

    #[test]
    fn test_move_highlight_crosses_a_page_boundary_and_turns_the_page() {
        let mut paging = Paging::with_page_size(5);
        // Four steps reach the last candidate of the first page.
        for expected in [1u16, 2, 3, 4] {
            assert!(paging.move_highlight(1, 45));
            assert_eq!(paging.highlight, expected);
            assert_eq!(paging.page, 0);
        }
        // The fifth step crosses onto the next page and lands on its first item.
        assert!(paging.move_highlight(1, 45));
        assert_eq!(paging.highlight, 5);
        assert_eq!(paging.page, 1);
        // And back again, onto the last item of the page before.
        assert!(paging.move_highlight(-1, 45));
        assert_eq!(paging.highlight, 4);
        assert_eq!(paging.page, 0);
    }

    #[test]
    fn test_move_highlight_stops_at_both_ends_without_wrapping() {
        let mut paging = Paging::with_page_size(9);
        assert!(!paging.move_highlight(-1, 45));
        assert_eq!(paging.highlight, 0);
        // With nine per page the reach covers the whole 45-candidate list.
        for _ in 0..200 {
            paging.move_highlight(1, 45);
        }
        assert_eq!(paging.page, MAX_PAGES - 1);
        assert_eq!(paging.highlight, MAX_REACHABLE_CANDIDATES - 1);
        assert!(!paging.move_highlight(1, 45));
    }

    #[test]
    fn test_move_highlight_stops_at_the_last_candidate_the_window_can_reach() {
        // Five candidates per page reach twenty-five of the forty-five decoded
        // candidates; the rest stay decoded but unreachable, which is what ASM-07
        // bounds.
        let mut paging = Paging::with_page_size(5);
        for _ in 0..200 {
            paging.move_highlight(1, 45);
        }
        assert_eq!(paging.page, MAX_PAGES - 1);
        assert_eq!(paging.highlight, u16::from(MAX_PAGES) * 5 - 1);
        assert!(paging.highlight < 44);
    }

    #[test]
    fn test_move_highlight_ignores_a_zero_delta_and_an_empty_list() {
        let mut paging = Paging::with_page_size(9);
        paging.highlight = 2;
        assert!(!paging.move_highlight(0, 45));
        assert_eq!(paging.highlight, 2);
        assert!(!paging.move_highlight(1, 0));
        assert_eq!(paging.highlight, 2);
    }

    #[test]
    fn test_reconcile_keeps_the_highlight_on_the_same_text() {
        let mut paging = Paging::with_page_size(5);
        let before = candidates(45);
        paging.highlight = 7;
        paging.page = 1;
        let text = before[7].text.clone();

        // The same word is still offered, but at a different position: the highlight
        // follows the word, not the position.
        let mut after = candidates(45);
        after.remove(7);
        after.insert(3, before[7].clone());
        paging.reconcile(Some(&text), &after);
        assert_eq!(paging.highlight, 3);
        assert_eq!(paging.page, 0);
        assert_eq!(after[usize::from(paging.highlight)].text, text);
    }

    #[test]
    fn test_reconcile_resets_when_the_text_is_gone() {
        let mut paging = Paging::with_page_size(5);
        paging.highlight = 12;
        paging.page = 2;
        paging.reconcile(Some("gone"), &candidates(45));
        assert_eq!(paging.highlight, 0);
        assert_eq!(paging.page, 0);
        // A first decode has nothing to look for and starts at the first candidate.
        paging.highlight = 9;
        paging.reconcile(None, &candidates(45));
        assert_eq!(paging.highlight, 0);
        assert_eq!(paging.page, 0);
    }

    #[test]
    fn test_reconcile_clamps_a_list_that_shrank_from_45_to_one() {
        let mut paging = Paging::with_page_size(9);
        paging.highlight = 44;
        paging.page = 4;
        paging.reconcile(Some("w44"), &candidates(1));
        assert_eq!(paging.highlight, 0);
        assert_eq!(paging.page, 0);
        assert_eq!(paging.page_count(1), 1);
    }

    #[test]
    fn test_reconcile_after_a_page_size_change_keeps_the_highlight_visible() {
        let list = candidates(45);
        let mut paging = Paging::with_page_size(5);
        paging.highlight = 17;
        paging.page = 3;
        let text = list[17].text.clone();

        paging.set_page_size(9);
        paging.reconcile(Some(&text), &list);
        assert_eq!(paging.highlight, 17);
        // Seventeen is on the second page of nine, not on the fourth page of five.
        assert_eq!(paging.page, 1);
        assert_eq!(paging.local_index(), 8);
        assert!(paging.local_index() < u16::from(paging.page_size));
    }

    #[test]
    fn test_reconcile_clamps_a_text_that_lies_past_the_five_page_window() {
        let list = candidates(45);
        let mut paging = Paging::with_page_size(5);
        let text = list[30].text.clone();
        paging.reconcile(Some(&text), &list);
        // Thirty is past the fifth page of five, so the highlight stops at the last
        // candidate the window can reach.
        assert_eq!(paging.page, MAX_PAGES - 1);
        assert_eq!(paging.highlight, u16::from(MAX_PAGES) * 5 - 1);
    }

    #[test]
    fn test_digit_target_names_a_candidate_on_the_page_on_show() {
        let mut paging = Paging::with_page_size(5);
        assert_eq!(paging.digit_target(1, 45), Some(0));
        assert_eq!(paging.digit_target(5, 45), Some(4));
        // Past the end of the page, and never a selection.
        assert_eq!(paging.digit_target(6, 45), None);
        assert_eq!(paging.digit_target(0, 45), None);
        assert_eq!(paging.digit_target(10, 45), None);
        // A page boundary does not renumber the digits: page 1 shows indices 5..9, so
        // its digits run 1..=5 again and 6 still names nothing. The guard is against the
        // page on show, not against the list.
        assert!(paging.flip(PageDir::Next, 45));
        assert_eq!(paging.digit_target(1, 45), Some(5));
        assert_eq!(paging.digit_target(5, 45), Some(9));
        assert_eq!(paging.digit_target(6, 45), None);

        let mut short = Paging::with_page_size(5);
        assert_eq!(short.digit_target(3, 2), None);
        short.page = 1;
        assert_eq!(short.digit_target(1, 7), Some(5));
        assert_eq!(short.digit_target(3, 7), None);
    }

    #[test]
    fn test_page_state_reports_one_based_pages() {
        let mut paging = Paging::with_page_size(9);
        let first = paging.page_state(45);
        assert_eq!(first.current, 1);
        assert_eq!(first.total, MAX_PAGES);
        assert_eq!(first.page_size, 9);
        assert!(paging.flip(PageDir::Next, 45));
        assert_eq!(paging.page_state(45).current, 2);
        // An empty list has no page at all.
        assert_eq!(paging.page_state(0).current, 0);
        assert_eq!(paging.page_state(0).total, 0);
    }

    #[test]
    fn test_local_index_is_the_position_within_the_page() {
        let mut paging = Paging::with_page_size(5);
        paging.highlight = 7;
        paging.page = 1;
        assert_eq!(paging.local_index(), 2);
        // A highlight before the page start cannot underflow.
        paging.page = 3;
        assert_eq!(paging.local_index(), 0);
    }

    #[test]
    fn test_highlight_position_in_page_names_the_candidate_the_page_shows() {
        let mut paging = Paging::with_page_size(5);
        paging.highlight = 3;
        assert_eq!(
            paging.highlight_position_in_page(45),
            Some(3),
            "the third candidate of the first page sits at position three"
        );
        // A page flip re-bases the highlight: the same global index lands on the page's
        // own numbering, so the frame and the grid always mean the same cell.
        assert!(paging.flip(PageDir::Next, 45));
        assert_eq!(paging.highlight, 5);
        assert_eq!(paging.highlight_position_in_page(45), Some(0));
        paging.highlight = 9;
        assert_eq!(paging.highlight_position_in_page(45), Some(4));
    }

    #[test]
    fn test_highlight_position_in_page_is_none_when_the_page_holds_no_candidate() {
        // An empty list has no page and no highlight to place on it.
        assert_eq!(Paging::new().highlight_position_in_page(0), None);
        // A single candidate is reachable and highlighted.
        assert_eq!(Paging::new().highlight_position_in_page(1), Some(0));

        // A highlight past the page's end names no candidate on it: a partly filled
        // last page ends where the list does, and the frame hides the ring instead of
        // drawing one on a cell that is not there.
        let mut paging = Paging::with_page_size(5);
        paging.page = 2;
        paging.highlight = 12;
        assert_eq!(
            paging.highlight_position_in_page(13),
            Some(2),
            "the last page of thirteen candidates holds three"
        );
        paging.highlight = 13;
        assert_eq!(paging.highlight_position_in_page(13), None);
    }
}
