//! The frame context a session carries between steps.
//!
//! Responsibility: where the window goes and what its status strip says, held rather than
//! passed to `step`, because the anchor has to survive between steps -- a frame emitted on
//! a keystroke the host said nothing about still has to land in the right place.
//!
//! Boundaries: pure data. The host writes it through the session's setters; nothing here
//! reads the host.

use ime_types::{Anchor, Placement, RectI, ScreenId, StatusStrip};

/// The parts of a frame the session cannot derive from its own state.
///
/// The cursor rectangle and the mode bits belong to the host, not to the session, so
/// the engine copies them here before stepping and the session writes them into
/// every frame it emits. They are held rather than passed to [`step`] because the
/// anchor has to survive between steps: a frame emitted on a keystroke the host did
/// not tell us about must still place the window where the caret is.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameContext {
    /// Where the window should appear, as far as the host has reported it.
    pub anchor: Anchor,
    /// Mode label, full-width and punctuation flags, and the read-only marker.
    pub status: StatusStrip,
}

impl Default for FrameContext {
    /// The context of a session that has not been told anything: an empty cursor
    /// rectangle on the first screen, and a blank status strip.
    fn default() -> Self {
        Self {
            anchor: Anchor {
                cursor: RectI {
                    x: 0,
                    y: 0,
                    w: 0,
                    h: 0,
                },
                screen: ScreenId::new(0),
                scale: 1.0,
                placement: Placement::Auto,
            },
            status: StatusStrip::default(),
        }
    }
}
