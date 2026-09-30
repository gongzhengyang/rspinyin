//! Turning backend events into the events the host is posted.
//!
//! Responsibility: drain what the platform has queued and hand every event to the pointer
//! router, which decides what it means and which channel it travels on. This is the only
//! place a `SurfaceEvent` is read, and the only place the router is driven, so the mouse path
//! and the keyboard path share one selection entry rather than each growing their own.
//!
//! Boundaries: nothing here decides what a candidate is worth, and nothing here draws or
//! places anything. A position outside every cell clears the hover rather than failing, and a
//! click that cannot be handed over inside the queue's budget is abandoned: failing this call
//! would take the whole UI thread down, and a lost click is recoverable while a dead candidate
//! window is not.
//!
//! # Coordinate spaces
//!
//! A pointer position arrives relative to the window and the hit map is expressed in the
//! container's own space, so the shadow reserve is subtracted once, inside the interaction
//! layer, and nothing here has to know which space it holds a number in.
//!
//! # Why the drawn state is refreshed here
//!
//! The five states of the design table -- the focus ring, the hover, the press -- are not
//! part of `UiFrame`: the engine's paging state holds the highlight and the frame carries
//! none of it. The pointer state is therefore pushed into the adapter whenever the router
//! says something drawn changed, which is the same call that asks for the repaint. A hover
//! that reached the model but not the window would be a highlight the user cannot see.

use std::time::Instant;

use ime_types::ImeError;

use crate::adapter::{PointerState, local_position};
use crate::channel::UiEventQueue;
use crate::interaction::RouteRequest;

use super::CandidateSurface;

impl CandidateSurface {
    /// Drains pending input and compositor events, posting what they mean.
    ///
    /// Each event is translated by the pointer router and posted into the channel its
    /// overflow rule belongs to; a resize or a scale change was already applied to the
    /// window by the platform. Events beyond `limit` stay queued for the next call, so a
    /// burst of pointer motion cannot starve rendering and no input is lost either.
    ///
    /// # Parameters
    ///
    /// * `events` -- the queue the loop posts the engine's events to.
    /// * `limit` -- how many events this call may consume.
    ///
    /// # Errors
    ///
    /// Returns [`ImeError::CompositorUnsupported`] when the display connection is gone, which
    /// means no further frame can be presented.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn drain_events(&mut self, events: &UiEventQueue, limit: usize) -> Result<(), ImeError> {
        self.platform
            .poll_events(&mut self.pending)
            .map_err(ImeError::from)?;
        let ready = self.pending.len().min(limit);
        let revision = self.frame.as_ref().map_or(0, |frame| frame.revision);
        // One clock read per batch rather than one per event: the click debounce and the
        // hover throttle are both measured against the instant the batch arrived.
        let now = Instant::now();
        // The batch is moved out so that the events can be drained while the router and the
        // placement they are tested against are borrowed from the surface; the allocation
        // travels with it, so nothing is allocated per event. Whatever the limit left over
        // is handed back for the next call.
        let mut batch = core::mem::take(&mut self.pending);
        // Adopting before the batch is what makes the events that arrived after a placement
        // land on the cells that placement produced.
        if core::mem::take(&mut self.adoption_pending) {
            let _ = self
                .router
                .adopt_frame(revision, self.geometry.as_ref(), now, events);
        }
        for event in batch.drain(..ready) {
            // The hover or the selection that came out is the host's, and the channel it was
            // posted to owns what happens to it next.
            let _ = self.router.route(
                RouteRequest {
                    event: &event,
                    revision,
                    geometry: self.geometry.as_ref(),
                    now,
                },
                events,
            );
        }
        self.pending = batch;
        // The router raises this for the transitions that produce no event of their own -- a
        // press, a release, a gesture that was cancelled -- which are exactly the ones the
        // window would otherwise never draw.
        if self.router.take_repaint() {
            self.sync_pointer();
        }
        Ok(())
    }

    /// Writes the pointer state the router holds into the grid the component draws.
    ///
    /// The router numbers candidates globally, across pages, because that is the numbering
    /// the host is told about; the grid compares them against a cell of the page on show. The
    /// conversion is the adapter's own, so the cell the pointer state names is the cell the
    /// hit map named.
    ///
    /// The highlight is deliberately not converted: it is the one field that is already a
    /// position within the page, because it belongs to the engine's paging state and arrives
    /// through the adapter rather than through this path. Mapping it again would move the
    /// focus ring to a cell the engine never named.
    fn sync_pointer(&mut self) {
        let Some(frame) = self.frame.as_deref() else {
            return;
        };
        let page = frame.page;
        let pointer = PointerState {
            highlighted: self.adapter.state().pointer.highlighted,
            hovered: self
                .router
                .hovered()
                .and_then(|index| local_position(index, &page)),
            pressed: self
                .router
                .pressed()
                .and_then(|index| local_position(index, &page)),
        };
        // The adapter asks for the repaint itself when the grid changed, so the return value
        // is the same information the router's flag already carried.
        self.adapter.apply_pointer(pointer);
    }
}
