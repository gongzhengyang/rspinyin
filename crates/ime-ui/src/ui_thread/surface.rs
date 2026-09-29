//! The seam between the UI thread's event loop and whatever draws the window.
//!
//! The event loop owns the channels, the wakeup and the `poll(2)` call; it knows
//! nothing about Slint, pixels or platform windows. Everything it needs from the
//! other side is [`UiSurface`]: a descriptor to wait on, a place to put a state
//! change, a place to post the events the user produced, a render call, and a
//! way to go away.
//!
//! Keeping the trait this small is what lets the loop be tested without a
//! display server -- the tests drive a recording surface instead -- and it is
//! also the only contract a renderer has to satisfy to be driven by this loop.
//! The surface is built on the UI thread, because that is where the platform
//! object has to be installed, so it arrives as a factory rather than as a value
//! moved across the thread boundary.

use std::os::fd::BorrowedFd;
use std::time::Instant;

use ime_types::{Anchor, HideReason, ImeError, ThemeSpec, UiCommand, UiFrame};

use crate::channel::UiEventQueue;

/// One state change the loop hands to the surface.
///
/// This is [`UiCommand`] with the two channels that never reach a surface
/// removed: `Shutdown` is consumed by the loop itself, and the routing that
/// separates a coalescing channel from an ordered one has already happened by
/// the time a value is applied. Making that explicit means a surface cannot
/// forget to handle a case that can never occur.
#[derive(Clone, Debug, PartialEq)]
pub enum SurfaceUpdate {
    /// A complete candidate-window state; the newest one wins.
    Frame(Box<UiFrame>),
    /// Semantic theme tokens, which the surface resolves into colours.
    Theme(ThemeSpec),
    /// The window should appear, anchored at the cursor.
    Show {
        /// Revision of the frame the anchor belongs to.
        revision: u32,
        /// Where the cursor is, in screen physical pixels.
        anchor: Anchor,
    },
    /// The window should disappear.
    Hide {
        /// Revision of the frame that was current when the host decided.
        revision: u32,
        /// Why the window is being hidden.
        reason: HideReason,
    },
}

impl SurfaceUpdate {
    /// Converts a command that travelled on the ordered channel into an update.
    ///
    /// Returns `None` for the commands that never travel that way, so the loop
    /// can drain the ordered queue without an arm it can never reach.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn from_control(command: UiCommand) -> Option<Self> {
        match command {
            UiCommand::Show { revision, anchor } => Some(Self::Show { revision, anchor }),
            UiCommand::Hide { revision, reason } => Some(Self::Hide { revision, reason }),
            // Frames, themes and shutdown have their own channels; a value that
            // arrives on the ordered queue is a routing bug, and dropping it is
            // safer than applying it out of order.
            UiCommand::Frame(_) | UiCommand::Theme(_) | UiCommand::Shutdown => None,
        }
    }
}

/// Everything the UI thread's event loop drives.
///
/// # Concurrency
///
/// The trait is deliberately **not** `Send`. An earlier version required it, on the
/// reasoning that a surface "must be free to own a display connection that another
/// thread could in principle have created". That reasoning does not hold: the
/// `Box<dyn UiSurface>` is built by the factory, which the UI thread runs, and is
/// consumed by the UI loop on the same thread, so the box never crosses a thread
/// boundary and the bound bought nothing. What it cost was the production
/// implementation -- the Slint-backed surface owns a platform object and a component
/// handle, both reference-counted and therefore never `Send`, so the bound made the
/// real surface impossible to write while the headless test surface satisfied it
/// trivially. The constraint the old doc was reaching for is real, and it is enforced
/// where it belongs: a connection must be created on the thread that will poll it.
///
/// It is likewise not `Sync`, because every method takes `&mut self` and a surface is
/// single-threaded. Implementations must not block beyond the call itself -- a surface
/// that waits on the compositor would stall the wakeup the host thread depends on --
/// and must never take keyboard focus: a candidate window that steals focus is the
/// project's highest-severity defect.
///
/// Methods are called from the UI thread only, never reentrantly, and never from
/// inside another method of the same surface.
pub trait UiSurface {
    /// The descriptor the loop adds to its `poll` set, if the surface has one.
    ///
    /// Returning `None` is correct for a surface with no connection of its own:
    /// the loop then waits only on the wakeup counter, which is exactly what a
    /// headless test surface wants.
    fn event_fd(&self) -> Option<BorrowedFd<'_>>;

    /// Applies one state change.
    ///
    /// # Errors
    ///
    /// Returns an error when the change cannot be applied at all. A surface that
    /// can only partially apply one should record what it could not do and
    /// return `Ok`, because failing the call takes the whole UI thread down.
    fn apply(&mut self, update: SurfaceUpdate) -> Result<(), ImeError>;

    /// Drains pending input and compositor events, posting what they mean.
    ///
    /// `limit` caps how many events one call may consume, so a burst of pointer
    /// motion cannot starve rendering.
    ///
    /// # Errors
    ///
    /// Returns an error when the connection is gone or a backend call fails.
    fn drain_events(&mut self, events: &UiEventQueue, limit: usize) -> Result<(), ImeError>;

    /// Advances animation state and draws if anything is dirty.
    ///
    /// Returns the instant the next frame is due, or `None` when nothing is
    /// animating. `None` is what lets the loop block indefinitely: a surface
    /// that returned `Some(now)` forever would turn the loop into a busy poll
    /// and break the idle-CPU budget.
    ///
    /// # Errors
    ///
    /// Returns an error when the frame cannot be rendered or presented. A frame
    /// that cannot be presented because the compositor still holds the previous
    /// buffer is not an error: the surface skips it and returns `Ok`.
    fn render(&mut self, now: Instant) -> Result<Option<Instant>, ImeError>;

    /// Runs the disappearing animation, unmaps the surface and releases it.
    ///
    /// # Errors
    ///
    /// Returns an error when the surface cannot be released.
    ///
    /// # Blocking
    ///
    /// This call may run an animation, so it is the one method allowed to take
    /// time; it must still return well inside the host's shutdown budget, which
    /// is 200ms by default.
    fn close(&mut self) -> Result<(), ImeError>;
}

#[cfg(test)]
mod tests {
    use ime_types::{Placement, RectI, ScreenId};

    use super::*;

    fn anchor() -> Anchor {
        Anchor {
            cursor: RectI {
                x: 1,
                y: 2,
                w: 3,
                h: 4,
            },
            screen: ScreenId::new(0),
            scale: 1.0,
            placement: Placement::Below,
        }
    }

    #[test]
    fn test_surface_update_from_control_keeps_show_and_hide() {
        let show = UiCommand::Show {
            revision: 4,
            anchor: anchor(),
        };
        assert_eq!(
            SurfaceUpdate::from_control(show),
            Some(SurfaceUpdate::Show {
                revision: 4,
                anchor: anchor()
            })
        );
        let hide = UiCommand::Hide {
            revision: 5,
            reason: HideReason::FocusLost,
        };
        assert_eq!(
            SurfaceUpdate::from_control(hide),
            Some(SurfaceUpdate::Hide {
                revision: 5,
                reason: HideReason::FocusLost
            })
        );
    }

    #[test]
    fn test_surface_update_from_control_rejects_the_other_channels() {
        let frame = UiCommand::Frame(Box::new(UiFrame {
            revision: 1,
            preedit: ime_types::Preedit {
                text: String::new(),
                caret: 0,
                spans: Vec::new(),
            },
            candidates: Vec::new(),
            page: ime_types::PageState {
                current: 1,
                total: 1,
                page_size: 9,
            },
            status: ime_types::StatusStrip::default(),
            anchor: anchor(),
            layout: ime_types::LayoutHint {
                max_per_row: 5,
                show_annotation: true,
                max_width_dp: 720,
            },
        }));
        assert_eq!(SurfaceUpdate::from_control(frame), None);
        let theme = UiCommand::Theme(ThemeSpec {
            scheme: ime_types::ColorScheme::Dark,
            accent: ime_types::Rgba8 {
                r: 1,
                g: 2,
                b: 3,
                a: 255,
            },
            acrylic: false,
            base_alpha: 217,
            corner_radius_dp: 12,
            scale: 1.0,
        });
        assert_eq!(SurfaceUpdate::from_control(theme), None);
        assert_eq!(SurfaceUpdate::from_control(UiCommand::Shutdown), None);
    }
}
