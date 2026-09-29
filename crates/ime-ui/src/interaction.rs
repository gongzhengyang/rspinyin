//! Pointer and wheel input: hit testing, coordinate conversion and event translation.
//!
//! [`InteractionState::translate`] turns one [`SurfaceEvent`] into at most one
//! [`UiEvent`]: a hover when the pointer settles on a different candidate, a click
//! when a press and its release land on the same cell, a page turn or a dismissal
//! when the wheel moves, and nothing at all for everything else. The work is
//! integer arithmetic over the frame's hit map -- no filesystem, no lock, no
//! allocation per event -- because it runs inside the UI thread's `poll` loop and
//! a moving pointer is the highest-rate input that thread ever sees.
//!
//! # What it decides
//!
//! | `SurfaceEvent` | `UiEvent` | Condition |
//! |---|---|---|
//! | `PointerEnter` / `PointerMotion` | `Hover { index }` | the hit candidate changed |
//! | `PointerButton { button: 1, pressed: true }` | none | records the pressed cell, which is what draws the `Active` state |
//! | `PointerButton { button: 1, pressed: false }` | `Select { index, trigger: Mouse }` | the release landed on the cell the press started on, against the same frame, outside the debounce window |
//! | `PointerButton { button: 3 }` | `Dismiss { reason: OutsideClick }` | the right button closes the window |
//! | `Axis { delta > 0 }` | `Page { dir: Next }` | the pointer is on the panel |
//! | `Axis { delta < 0 }` | `Page { dir: Prev }`, or `Dismiss { reason: ScrollUpEmpty }` on the first page | the pointer is on the panel |
//! | `Axis { horizontal: true }` | none | horizontal scrolling is not ours |
//! | `PointerLeave` | `Hover { index: None }` | something was hovered |
//!
//! The wheel sign is normalised by the backends, which agree that a positive
//! vertical delta means "page forward"; nothing here inspects it beyond its
//! direction.
//!
//! # Coordinate spaces
//!
//! A pointer coordinate arrives relative to the **window**, shadow reserve
//! included, while the hit map is expressed relative to the **container**, which
//! excludes it. The subtraction happens exactly once, in `hit_test`, so no caller
//! has to know which space it holds a number in.
//!
//! The geometry is borrowed from the caller on every call rather than cached here.
//! It is recomputed whenever the window is re-placed -- a scale change, a new caret
//! position -- which happens without any change of frame revision, so a cache keyed
//! on the revision would hit test against a stale map after exactly the events that
//! move the cells.
//!
//! # Where the hover throttle lives
//!
//! A hover is posted through the channel's latest-wins slot, which owns the 16ms
//! throttle and the "newest index wins" rule. This module's half of that contract
//! is the rule that a hover is produced only when the hovered candidate *changes*:
//! a stationary pointer produces one event, not one per motion sample. Suppressing
//! a changed hover here as well would lose the pointer's final position -- there is
//! no timer on this thread to deliver it later -- so the throttle belongs where the
//! value can be stored rather than discarded.
//!
//! # What is deliberately absent
//!
//! No timer and no polling: the only clock read is the one [`InteractionState::translate`]
//! performs to timestamp a click, and [`InteractionState::translate_at`] takes that
//! instant as an argument so the debounce is testable without sleeping. Nothing here
//! blocks, allocates or takes a lock, and nothing here touches the host thread.

#[cfg(test)]
mod tests;

use std::time::{Duration, Instant};

use ime_types::{DismissReason, PageDir, RectI, SelectTrigger, SurfaceEvent, UiEvent};

use crate::geometry::Geometry;

/// How long a second click on the same candidate is ignored.
///
/// A double click is one intent. Two commits in a row would be two, and the user
/// would have to delete one.
const CLICK_DEBOUNCE: Duration = Duration::from_millis(100);

/// The primary (left) mouse button, in the numbering every backend reports.
const BUTTON_PRIMARY: u8 = 1;

/// The secondary (right) mouse button.
const BUTTON_SECONDARY: u8 = 3;

/// What a point in window coordinates lands on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HitResult {
    /// A candidate cell. The index is global across pages, so it is the index the
    /// host expects in [`UiEvent::Select`] and [`UiEvent::Hover`].
    Candidate(u16),
    /// The panel, but no cell: the header, the separator, the padding or the gap
    /// between two cells.
    Container,
    /// The transparent shadow reserve, or a point outside the window entirely.
    Outside,
}

/// The candidate a press started on, and the frame it started against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Press {
    /// Global candidate index under the pointer when the button went down.
    index: u16,
    /// Revision of the frame that index belongs to. A release against a different
    /// frame names a different candidate, so it must not select.
    revision: u32,
}

/// The pointer-interaction state machine of one candidate window.
///
/// It holds only what a pointer gesture needs to remember across events: what the
/// host was last told is hovered, where a press started, where the pointer was
/// last seen, and when the last click was accepted. Everything else -- the cells
/// and their rectangles -- is read from the [`Geometry`] the caller passes in.
///
/// # Concurrency
///
/// Owned by the UI thread and never shared, like the surface that drives it. It is
/// `Send` and `Sync` because it holds nothing but plain values, but no method is
/// meant to be called from two threads, and the clock argument is the caller's
/// rather than the state's so that a test can drive it without waiting.
#[derive(Debug, Default)]
pub struct InteractionState {
    /// The candidate the host was last told is hovered, or `None` when the pointer
    /// is off the grid. Kept so that a motion which changes nothing produces no
    /// event.
    hovered: Option<u16>,
    /// The press in progress, if any.
    pressed: Option<Press>,
    /// The last pointer position seen, in window physical pixels. Kept so that a
    /// frame change can re-hit-test a pointer that is not moving.
    pointer: Option<(i32, i32)>,
    /// The last accepted click: when it happened and which candidate it selected.
    /// The debounce window is measured from here, and only an accepted click moves
    /// it, so a user drumming on one cell is not silenced indefinitely.
    last_select: Option<(Instant, u16)>,
    /// Clicks the debounce suppressed; the probe counter behind `ui/click/debounced`.
    debounced_clicks: u64,
    /// Whether the drawn state -- `is-hovered`, `is-pressed` -- changed since the
    /// caller last took the flag. A press changes what is drawn without producing a
    /// [`UiEvent`], so the flag is how the caller learns it has something to repaint.
    repaint: bool,
}

impl InteractionState {
    /// Creates a state that has seen no pointer.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn new() -> Self {
        Self::default()
    }

    /// Translates one surface event, timestamping it with the current instant.
    ///
    /// This is the entry point a production caller uses. A caller that already holds
    /// an instant -- the surface needs one anyway to post a hover -- should use
    /// [`InteractionState::translate_at`] instead, which is the same function
    /// without the clock read.
    ///
    /// # Returns
    ///
    /// The event the input means, or `None` when it means nothing: a motion that
    /// stays on the same cell, a press (which is view state rather than an event),
    /// a horizontal wheel, or any compositor event.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn translate(
        &mut self,
        event: &SurfaceEvent,
        revision: u32,
        geometry: &Geometry,
    ) -> Option<UiEvent> {
        self.translate_at(event, revision, geometry, Instant::now())
    }

    /// Translates one surface event against the given instant.
    ///
    /// `now` is passed in rather than read from the clock so that the click debounce
    /// is a pure function of its inputs and can be tested without sleeping, which is
    /// the same reason the event channel takes its hover instant as an argument.
    ///
    /// # Parameters
    ///
    /// * `event` -- one pointer, wheel or compositor event from the backend.
    /// * `revision` -- revision of the frame the caller is showing; it is carried
    ///   into the event so the engine can drop one that arrives late.
    /// * `geometry` -- the placement pass's output for that frame, whose `hit_map`
    ///   is in container coordinates and whose `container_offset` is the shadow
    ///   reserve the pointer coordinate has to be moved by.
    /// * `now` -- the instant to measure the debounce window against.
    ///
    /// # Returns
    ///
    /// The event the input means, or `None` when it means nothing. At most one event
    /// is produced per call, and never a [`UiEvent::Hover`] for a cell the host has
    /// already been told about.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn translate_at(
        &mut self,
        event: &SurfaceEvent,
        revision: u32,
        geometry: &Geometry,
        now: Instant,
    ) -> Option<UiEvent> {
        match *event {
            SurfaceEvent::PointerEnter { x, y } | SurfaceEvent::PointerMotion { x, y } => {
                self.motion(x, y, geometry, revision)
            }
            SurfaceEvent::PointerLeave => self.leave(geometry, revision),
            // A press draws the `Active` state and produces no event of its own; what
            // the host hears about is the release that completes the gesture.
            SurfaceEvent::PointerButton {
                x,
                y,
                button: BUTTON_PRIMARY,
                pressed: true,
            } => {
                self.press(x, y, geometry, revision);
                None
            }
            SurfaceEvent::PointerButton {
                x,
                y,
                button: BUTTON_PRIMARY,
                pressed: false,
            } => self.release(x, y, geometry, revision, now),
            // The right button closes the window. It acts on the press rather than on
            // the release: the window may be gone by the time the button comes back
            // up, and a dismissal that waited for it would be felt as a delay. The
            // release is then ignored, so one right click is one dismissal.
            SurfaceEvent::PointerButton {
                button: BUTTON_SECONDARY,
                pressed: true,
                ..
            } => {
                self.forget_press();
                Some(UiEvent::Dismiss {
                    revision,
                    reason: DismissReason::OutsideClick,
                })
            }
            SurfaceEvent::Axis {
                x,
                y,
                delta,
                horizontal,
            } => self.axis(x, y, delta, horizontal, geometry, revision),
            // Other buttons, and the releases of the ones handled above, say nothing.
            // The wheel buttons never reach here: every backend reports them as
            // `Axis`, which is what keeps the two scroll conventions in one place.
            SurfaceEvent::PointerButton { .. } => None,
            // Compositor events belong to the renderer and the platform layer; this
            // module only ever answers for the pointer.
            SurfaceEvent::Resize { .. }
            | SurfaceEvent::Scale { .. }
            | SurfaceEvent::CloseRequested => None,
        }
    }

    /// Adopts a newly applied frame and re-hit-tests a stationary pointer against it.
    ///
    /// The caller invokes this when it applies a frame, or when the placement pass
    /// produced a new [`Geometry`] for the frame it already holds. Without it, a
    /// pointer that is not moving would keep a hover that the new cells no longer
    /// justify, and a press taken against the old frame could still select against
    /// the new one.
    ///
    /// # Parameters
    ///
    /// * `revision` -- revision of the frame being adopted.
    /// * `geometry` -- that frame's geometry.
    ///
    /// # Returns
    ///
    /// A [`UiEvent::Hover`] when the hovered index changed as a result -- the pointer
    /// is no longer over a cell, or it is over a cell that now carries a different
    /// global index -- and `None` when the hover survived unchanged.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn adopt_frame(&mut self, revision: u32, geometry: &Geometry) -> Option<UiEvent> {
        // A press cannot outlive the frame it started against: its index would name a
        // different candidate in the new one.
        self.forget_press();
        self.hover_event(geometry, revision)
    }

    /// The candidate the host was last told is hovered.
    ///
    /// This is the view layer's `is-hovered` source: the adapter compares it against
    /// each cell's index when it builds the grid.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn hovered(&self) -> Option<u16> {
        self.hovered
    }

    /// The candidate a press is currently held on, if any.
    ///
    /// This is the view layer's `is-pressed` source. It is set the moment the button
    /// goes down, before any event is produced, which is what makes the `Active`
    /// state immediate.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn pressed(&self) -> Option<u16> {
        self.pressed.map(|press| press.index)
    }

    /// Takes the "something drawn has changed" flag.
    ///
    /// The caller calls this after translating a batch of events and repaints when it
    /// returns `true`. Only the transitions that produce no event -- a press, a
    /// release, a cancelled gesture -- raise it, because a produced event is already
    /// the caller's cue.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn take_repaint(&mut self) -> bool {
        std::mem::take(&mut self.repaint)
    }

    /// How many clicks the debounce suppressed.
    ///
    /// This is the probe counter behind `ui/click/debounced`.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn debounced_clicks(&self) -> u64 {
        self.debounced_clicks
    }

    /// What a point in window coordinates lands on.
    ///
    /// The shadow reserve is subtracted here and nowhere else. The scan is linear
    /// because the map holds at most a page of cells, which is a few dozen
    /// comparisons against the five microsecond budget of one translation.
    ///
    /// # Panics
    ///
    /// This function does not panic. Coordinates are widened to `i64` before the
    /// subtraction, so a pointer far outside the window cannot wrap.
    fn hit_test(&self, geometry: &Geometry, x: i32, y: i32) -> HitResult {
        let px = i64::from(x) - i64::from(geometry.container_offset.0);
        let py = i64::from(y) - i64::from(geometry.container_offset.1);
        for (rect, index) in &geometry.hit_map {
            if inside_rect(rect, px, py) {
                return HitResult::Candidate(*index);
            }
        }
        let (width, height) = geometry.container_size;
        if (0..i64::from(width)).contains(&px) && (0..i64::from(height)).contains(&py) {
            HitResult::Container
        } else {
            HitResult::Outside
        }
    }

    /// Records a pointer position and reports a hover if the cell under it changed.
    fn motion(&mut self, x: i32, y: i32, geometry: &Geometry, revision: u32) -> Option<UiEvent> {
        self.pointer = Some((x, y));
        self.hover_event(geometry, revision)
    }

    /// Reports the hover for the last known pointer position, if it changed.
    fn hover_event(&mut self, geometry: &Geometry, revision: u32) -> Option<UiEvent> {
        let index = match self.pointer {
            Some((x, y)) => match self.hit_test(geometry, x, y) {
                HitResult::Candidate(index) => Some(index),
                // The panel's own blank areas and the shadow reserve both mean "no
                // candidate", which is what the host needs in order to clear a
                // highlight it was given earlier.
                HitResult::Container | HitResult::Outside => None,
            },
            None => None,
        };
        if self.hovered == index {
            return None;
        }
        self.hovered = index;
        self.repaint = true;
        Some(UiEvent::Hover { revision, index })
    }

    /// The pointer left the surface: the gesture ends and the hover clears.
    fn leave(&mut self, geometry: &Geometry, revision: u32) -> Option<UiEvent> {
        self.pointer = None;
        // A release we may never be sent -- the pointer is somewhere else now --
        // would otherwise leave a cell stuck in the pressed state, and a gesture
        // that leaves the window and comes back is a drag rather than a click.
        self.forget_press();
        self.hover_event(geometry, revision)
    }

    /// Starts a press gesture.
    fn press(&mut self, x: i32, y: i32, geometry: &Geometry, revision: u32) {
        self.pointer = Some((x, y));
        self.pressed = match self.hit_test(geometry, x, y) {
            HitResult::Candidate(index) => Some(Press { index, revision }),
            HitResult::Container | HitResult::Outside => None,
        };
        if self.pressed.is_some() {
            self.repaint = true;
        }
    }

    /// Ends a press gesture, selecting only when it is the click the user meant.
    fn release(
        &mut self,
        x: i32,
        y: i32,
        geometry: &Geometry,
        revision: u32,
        now: Instant,
    ) -> Option<UiEvent> {
        self.pointer = Some((x, y));
        // `?` rather than `let ... else { return None }`: the function answers `None` for
        // "this was not a click", and a release with no press is exactly that.
        let started = self.pressed?;
        self.forget_press();
        // A release somewhere else is a drag, not a click. The frame has to match
        // too: the same index in a newer frame is a different candidate.
        if started.revision != revision {
            return None;
        }
        let landed = match self.hit_test(geometry, x, y) {
            HitResult::Candidate(index) => index,
            HitResult::Container | HitResult::Outside => return None,
        };
        if landed != started.index {
            return None;
        }
        if self.is_repeat(landed, now) {
            return None;
        }
        self.last_select = Some((now, landed));
        Some(UiEvent::Select {
            revision,
            index: landed,
            trigger: SelectTrigger::Mouse,
        })
    }

    /// Whether a click on `index` at `now` is a repeat of one already accepted.
    fn is_repeat(&mut self, index: u16, now: Instant) -> bool {
        let repeat = self.last_select.is_some_and(|(at, selected)| {
            selected == index && now.saturating_duration_since(at) < CLICK_DEBOUNCE
        });
        if repeat {
            self.debounced_clicks = self.debounced_clicks.saturating_add(1);
        }
        repeat
    }

    /// Clears a press in progress, reporting whether anything drawn changed.
    fn forget_press(&mut self) {
        if self.pressed.take().is_some() {
            self.repaint = true;
        }
    }

    /// Turns a wheel step into a page turn, or into a dismissal at the first page.
    fn axis(
        &mut self,
        x: i32,
        y: i32,
        delta: i32,
        horizontal: bool,
        geometry: &Geometry,
        revision: u32,
    ) -> Option<UiEvent> {
        self.pointer = Some((x, y));
        if horizontal || delta == 0 {
            return None;
        }
        // The wheel is ours only while the pointer is on the panel. A step in the
        // transparent reserve, which is part of the window but not part of the
        // panel, is left for the host to act on.
        if matches!(self.hit_test(geometry, x, y), HitResult::Outside) {
            return None;
        }
        if delta > 0 {
            return Some(UiEvent::Page {
                revision,
                dir: PageDir::Next,
            });
        }
        if self.is_first_page(geometry) {
            // Scrolling up from the first page closes the window, which is what the
            // gesture means when there is nothing above to show.
            self.forget_press();
            return Some(UiEvent::Dismiss {
                revision,
                reason: DismissReason::ScrollUpEmpty,
            });
        }
        Some(UiEvent::Page {
            revision,
            dir: PageDir::Prev,
        })
    }

    /// Whether the frame being shown is the first page of the candidate list.
    ///
    /// The hit map pairs each cell with the global index the host expects, and the
    /// placement pass folds the page's offset into it, so its first entry is the
    /// first index on the page currently shown: zero exactly on the first page. An
    /// empty map means there is nothing on this page and therefore nothing above it.
    fn is_first_page(&self, geometry: &Geometry) -> bool {
        geometry
            .hit_map
            .first()
            .is_none_or(|(_, index)| *index == 0)
    }
}

/// Whether a point lies inside a rectangle.
///
/// The bounds are half-open, the same rule the output enumeration uses: a point on
/// the top or left edge is inside, one on the bottom or right edge is not, so two
/// cells that share an edge resolve to exactly one of them.
fn inside_rect(rect: &RectI, x: i64, y: i64) -> bool {
    let left = i64::from(rect.x);
    let top = i64::from(rect.y);
    (left..left + i64::from(rect.w)).contains(&x) && (top..top + i64::from(rect.h)).contains(&y)
}
