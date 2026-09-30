//! Routing one surface event into the channel that owns its overflow rule.
//!
//! [`InteractionState`] answers *what* a pointer gesture means; this module answers *where*
//! that answer goes. The split is deliberate, because the classification is contractual:
//! `UiEvent::Select` is never dropped, `UiEvent::Hover` is a latest-wins slot behind a 16ms
//! throttle, and `UiEvent::Page` / `UiEvent::Dismiss` are ordered and collapse to the newest.
//! A surface that posted everything to one queue would silently change one of those
//! guarantees, so the choice is taken in exactly one place and every surface goes through it.
//!
//! # Why a router rather than a free function
//!
//! The interaction state has to be owned by whoever sees the whole event stream, and the
//! surface is the only such owner. Keeping the state and the classification in one value is
//! what makes the sequence "translate, post, notice that something drawn changed" impossible
//! to perform in the wrong order: there is one call, and the repaint flag it leaves behind
//! is read by the same caller that made it.
//!
//! # Same source as the keyboard
//!
//! A click names a candidate by its **global** index -- the numbering `Paging::highlight`,
//! `UiEvent::Select` and the number keys all use -- so the candidate a click selects is the
//! one the digit that labels it selects. That is a property of the numbering rather than of
//! a conversion performed here, and it is pinned by a test: the index a hit test produces
//! and the index a number key produces are derived from different inputs and must agree.
//! The trigger field is the one field the two paths are meant to differ in, which is why it
//! is the only one.
//!
//! # What is deliberately absent
//!
//! No timer and no clock of its own: the instant a debounce and a throttle are measured
//! against is the caller's, so both are pure functions of their inputs. Nothing here
//! allocates per event, takes a lock, or touches the host thread; the only wait it can
//! perform is the bounded one [`UiEventQueue::post_select`] performs when the host is not
//! draining, and the surface that called it is the UI thread, never the host's.

#[cfg(test)]
mod tests;

use std::time::Instant;

use ime_types::{SurfaceEvent, UiEvent};

use crate::channel::UiEventQueue;
use crate::geometry::Geometry;

use super::InteractionState;

/// One input to [`PointerRouter::route`].
///
/// A bundle rather than a parameter list: the four values always travel together, and a
/// caller that has them already -- the surface, which reads the revision and the placement
/// off the frame it is drawing -- has nothing to assemble.
#[derive(Clone, Copy, Debug)]
pub struct RouteRequest<'a> {
    /// The backend event to translate.
    pub event: &'a SurfaceEvent,
    /// Revision of the frame the caller is showing. It travels into the produced event so
    /// the engine can drop one that arrives late.
    pub revision: u32,
    /// That frame's placement, or `None` before the first frame. Nothing is produced
    /// without a hit map to test the pointer against.
    pub geometry: Option<&'a Geometry>,
    /// The instant the click debounce and the hover throttle are measured against. Passed
    /// in rather than read here so that both are testable without sleeping.
    pub now: Instant,
}

/// Turns surface events into host events and posts them into the channel each belongs to.
///
/// # Concurrency
///
/// Owned by the UI thread and never shared, like the surface that drives it. It is `Send`
/// and `Sync` because it holds nothing but plain values, but no method is meant to be called
/// from two threads. Every method is non-blocking except for the bounded wait
/// [`UiEventQueue::post_select`] performs, which is the contract's "a click is never
/// dropped" rule rather than a wait this type chose.
#[derive(Debug, Default)]
pub struct PointerRouter {
    /// The translation state: what the pointer is over, what a press started on, and
    /// whether anything drawn changed.
    state: InteractionState,
}

impl PointerRouter {
    /// Creates a router that has seen no pointer.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn new() -> Self {
        Self::default()
    }

    /// Translates one surface event and posts what it means.
    ///
    /// # Parameters
    ///
    /// * `request` -- the event, the revision it belongs to, the placement to test it
    ///   against and the instant to measure its debounce and throttle against.
    /// * `events` -- the queue the host drains.
    ///
    /// # Returns
    ///
    /// The event that reached the host, or `None` when the input meant nothing (a motion
    /// that stayed on the same cell, a press, a compositor event, a click the select channel
    /// refused inside its budget) or when there is no placement to test against yet.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn route(&mut self, request: RouteRequest<'_>, events: &UiEventQueue) -> Option<UiEvent> {
        // Without a placement there is no hit map, so every pointer event is meaningless
        // rather than "outside": the window has not been placed yet.
        let geometry = request.geometry?;
        let produced =
            self.state
                .translate_at(request.event, request.revision, geometry, request.now)?;
        post(produced, events, request.now)
    }

    /// Re-hit-tests a pointer that is not moving against a newly placed frame.
    ///
    /// The caller invokes this when it applies a frame, or when the placement pass produced
    /// a new geometry for the frame it already holds. Without it a stationary pointer would
    /// keep a hover the new cells no longer justify, and a press taken against the old frame
    /// could still select against the new one.
    ///
    /// # Parameters
    ///
    /// * `revision` -- revision of the frame being adopted.
    /// * `geometry` -- that frame's placement, or `None` when nothing has been placed.
    /// * `now` -- the instant to post the hover against.
    /// * `events` -- the queue the host drains.
    ///
    /// # Returns
    ///
    /// The hover that reached the host, or `None` when the hovered candidate survived the
    /// change or nothing has been placed.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn adopt_frame(
        &mut self,
        revision: u32,
        geometry: Option<&Geometry>,
        now: Instant,
        events: &UiEventQueue,
    ) -> Option<UiEvent> {
        let geometry = geometry?;
        let produced = self.state.adopt_frame(revision, geometry)?;
        post(produced, events, now)
    }

    /// Takes the "something drawn has changed" flag.
    ///
    /// The caller calls this after routing a batch of events and redraws when it returns
    /// `true`. A press and a release raise it without producing an event of their own, which
    /// is the only way they can become visible; a hover that moved to another candidate
    /// raises it too.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn take_repaint(&mut self) -> bool {
        self.state.take_repaint()
    }

    /// The candidate the host was last told is hovered.
    ///
    /// This is the view layer's `is-hovered` source, in the **global** numbering. The
    /// adapter converts it to a position within the page on show.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn hovered(&self) -> Option<u16> {
        self.state.hovered()
    }

    /// The candidate a press is currently held on, in the global numbering.
    ///
    /// This is the view layer's `is-pressed` source.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn pressed(&self) -> Option<u16> {
        self.state.pressed()
    }

    /// How many clicks the debounce suppressed.
    ///
    /// This is the probe counter behind `ui/click/debounced`.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn debounced_clicks(&self) -> u64 {
        self.state.debounced_clicks()
    }
}

/// Which channel an event belongs to.
///
/// The classification is written out rather than left to the caller so that there is one
/// answer to "where does this go". It is a private enum because the queues themselves are
/// the contract: what a caller observes is which slot an event comes back out of, and that
/// is what the tests assert on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Channel {
    /// The bounded queue that never drops an entry.
    Select,
    /// The latest-wins slot behind the throttle.
    Hover,
    /// The ordered queue, collapsed to the newest when the host is not draining.
    Ordered,
    /// Not posted here at all.
    Unrouted,
}

/// The channel an event belongs to.
fn channel_of(event: &UiEvent) -> Channel {
    match event {
        UiEvent::Select { .. } => Channel::Select,
        UiEvent::Hover { .. } => Channel::Hover,
        UiEvent::Page { .. } | UiEvent::Dismiss { .. } => Channel::Ordered,
        // A render receipt is a latency probe, and the surface posts it where the frame is
        // presented rather than here.
        UiEvent::Rendered { .. } => Channel::Unrouted,
    }
}

/// Posts one produced event and reports what the host will actually see.
///
/// A click the select channel refused inside its budget is abandoned rather than retried:
/// the counter behind `ui/select/timeout` records it, and failing the caller would take the
/// whole UI thread down for a click the user can repeat.
fn post(event: UiEvent, events: &UiEventQueue, now: Instant) -> Option<UiEvent> {
    match channel_of(&event) {
        Channel::Select => match events.post_select(event.clone()) {
            Ok(()) => Some(event),
            Err(_) => None,
        },
        Channel::Hover => {
            events.post_hover(event.clone(), now);
            Some(event)
        }
        Channel::Ordered => {
            events.post_ordered(event.clone());
            Some(event)
        }
        Channel::Unrouted => None,
    }
}
