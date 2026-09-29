//! Turning backend events into the events the host is posted.
//!
//! Responsibility: drain what the platform has queued, test a pointer position against the hit
//! map of the last placement, and post the hover or the selection that follows. This is the
//! only place a `SurfaceEvent` is read.
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
//! container's own space, so the shadow reserve is subtracted once, on the way in, and every
//! comparison below is like for like.

use std::time::Instant;

use ime_types::{ImeError, SelectTrigger, SurfaceEvent, UiEvent};

use crate::channel::UiEventQueue;
use crate::geometry::Geometry;

use super::CandidateSurface;

impl CandidateSurface {
    /// Drains pending input and compositor events, posting what they mean.
    ///
    /// Pointer events are tested against the hit map of the last placement and become a hover
    /// or a selection; a resize or a scale change was already applied to the window by the
    /// platform. Events beyond `limit` stay queued for the next call, so a burst of pointer
    /// motion cannot starve rendering and no input is lost either.
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
        let geometry = self.geometry.as_ref();
        for event in self.pending.drain(..ready) {
            deliver(event, events, geometry, revision);
        }
        Ok(())
    }
}

/// Turns one backend event into what the host needs to know about it.
///
/// Pointer positions arrive relative to the window and are tested against the hit map of the
/// last placement. Everything else either has no meaning for an override-redirect panel -- a
/// close request, a wheel -- or was already applied to the window by the platform, which is
/// what a resize and a scale change are.
fn deliver(event: SurfaceEvent, events: &UiEventQueue, geometry: Option<&Geometry>, revision: u32) {
    match event {
        SurfaceEvent::PointerEnter { x, y } | SurfaceEvent::PointerMotion { x, y } => {
            let index = hit_test(geometry, x, y);
            events.post_hover(UiEvent::Hover { revision, index }, Instant::now());
        }
        SurfaceEvent::PointerLeave => {
            events.post_hover(
                UiEvent::Hover {
                    revision,
                    index: None,
                },
                Instant::now(),
            );
        }
        SurfaceEvent::PointerButton {
            x,
            y,
            button: 1,
            pressed: true,
        } => {
            let Some(index) = hit_test(geometry, x, y) else {
                return;
            };
            // A click that cannot be handed over within the queue's budget is abandoned rather
            // than retried: the queue counts it behind `ui/select/timeout`, and failing this
            // call would take the whole UI thread down.
            let _ = events.post_select(UiEvent::Select {
                revision,
                index,
                trigger: SelectTrigger::Mouse,
            });
        }
        _ => {}
    }
}

/// The candidate under a window-relative pointer position, if there is one.
///
/// The pointer arrives relative to the window and the hit map is expressed in the container's
/// own space, so the shadow reserve is subtracted before the test. A position that cannot be
/// expressed in that space -- the far edge of an `i32` -- is outside every cell.
fn hit_test(geometry: Option<&Geometry>, x: i32, y: i32) -> Option<u16> {
    let geometry = geometry?;
    let x = i64::from(x) - i64::from(geometry.container_offset.0);
    let y = i64::from(y) - i64::from(geometry.container_offset.1);
    geometry
        .hit_map
        .iter()
        .find(|(rect, _)| {
            let left = i64::from(rect.x);
            let top = i64::from(rect.y);
            (left..left + i64::from(rect.w)).contains(&x)
                && (top..top + i64::from(rect.h)).contains(&y)
        })
        .map(|(_, index)| *index)
}
