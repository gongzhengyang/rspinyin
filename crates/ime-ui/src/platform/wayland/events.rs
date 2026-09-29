//! The wire vocabulary: protocol events reduced to plain data, and their translation onto the
//! contract's [`SurfaceEvent`].
//!
//! Responsibility: name the events this backend acts on in a form that carries no protocol
//! object, and convert the ones the contract speaks. Boundaries: the connection binding
//! decodes the wire into [`WireEvent`]; nothing here owns a connection, so every conversion
//! below is covered by an ordinary test with no compositor running.
//!
//! # Units
//!
//! Wayland measures a pointer position, a configure size and an input region in surface-local
//! units -- the same units as the surface's logical size, which is the buffer size divided by
//! the buffer scale. The contract measures in physical pixels. [`pointer_event`] is where the
//! pointer half of that conversion happens; [`SurfaceRect`](super::SurfaceRect) is where the
//! rectangle half does.
//!
//! # What is not modelled
//!
//! Events only the binding acts on are absent: `ack_configure`, `wl_display.delete_id`, seat
//! capabilities, key maps. The binding handles them where it decodes them, and nothing above
//! it needs to know they happened.

use ime_types::SurfaceEvent;

/// Which axis a scroll event moved along.
///
/// `wl_pointer.axis` carries a value in this vocabulary; an axis this build does not know is
/// [`PointerAxis::Other`] and is dropped rather than reported as a scroll nobody made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerAxis {
    /// `axis = 0`.
    Vertical,
    /// `axis = 1`.
    Horizontal,
    /// Anything else.
    Other,
}

/// One protocol event this backend acts on.
///
/// Coordinates are surface-local units, exactly as the wire carries them: converting them is
/// [`pointer_event`]'s job, and doing it here as well would apply the scale twice.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WireEvent {
    /// `wl_pointer.enter`: the pointer moved onto our surface.
    PointerEnter {
        /// Pointer position along x, in surface-local units.
        x: f64,
        /// Pointer position along y, in surface-local units.
        y: f64,
    },
    /// `wl_pointer.leave`: the pointer left our surface.
    PointerLeave,
    /// `wl_pointer.motion`.
    PointerMotion {
        /// Pointer position along x, in surface-local units.
        x: f64,
        /// Pointer position along y, in surface-local units.
        y: f64,
    },
    /// `wl_pointer.button`: a button was pressed or released.
    PointerButton {
        /// Pointer position along x, in surface-local units.
        x: f64,
        /// Pointer position along y, in surface-local units.
        y: f64,
        /// The Linux input event code, e.g. `0x110` for the left button.
        button: u32,
        /// Whether the button went down.
        pressed: bool,
    },
    /// `wl_pointer.axis`, with the discrete or high-resolution count when one was sent.
    PointerAxis {
        /// Pointer position along x, in surface-local units. The protocol's axis event
        /// carries no position, so the binding fills in the last one it saw.
        x: f64,
        /// Pointer position along y, in surface-local units.
        y: f64,
        /// Which axis moved.
        axis: PointerAxis,
        /// The continuous value the compositor reported.
        value: f64,
        /// The step count, from `axis_discrete` or from `axis_value120` divided by 120; zero
        /// when the compositor sent neither.
        discrete: i32,
    },
    /// `wl_surface.enter`: the surface moved onto an output, whose device pixel ratio follows.
    SurfaceEnter {
        /// The output's device pixel ratio.
        scale: f32,
    },
    /// `zwlr_layer_surface_v1.configure`, in surface-local units.
    LayerConfigure {
        /// The width the compositor will accept; zero means "choose one yourself".
        width: u32,
        /// The height the compositor will accept; zero means "choose one yourself".
        height: u32,
    },
    /// `zwlr_layer_surface_v1.closed`: the compositor will not show the surface again.
    LayerClosed,
    /// `xdg_popup.configure`, in surface-local units relative to the parent's window geometry.
    PopupConfigure {
        /// Left edge relative to the parent's window geometry.
        x: i32,
        /// Top edge relative to the parent's window geometry.
        y: i32,
        /// Width the compositor settled on.
        width: u32,
        /// Height the compositor settled on.
        height: u32,
    },
    /// `xdg_popup.popup_done`: the compositor dismissed the popup.
    PopupDone,
    /// `wl_buffer.release`: the compositor is finished with a buffer, which may be drawn into
    /// again.
    BufferReleased {
        /// The slot the released buffer was drawn into.
        slot: u8,
    },
    /// `wl_callback.done`: the frame callback requested earlier has fired.
    FrameDone {
        /// The token that was passed when the callback was requested.
        data: u64,
    },
    /// Our surface was given the keyboard.
    ///
    /// This must never arrive. The binding reports it from `wl_keyboard.enter` and must report
    /// it only for a surface of ours; the ladder turns it into a failed tier at once.
    KeyboardEnter,
    /// `wl_display.error`: the connection is unusable.
    DisplayError,
}

/// Translates a pointer event onto the contract vocabulary.
///
/// Returns `None` for any other event, and for a pointer event this backend has no contract
/// equivalent for -- a button with no counterpart in the numbering the contract uses is
/// dropped rather than mapped onto a number that would mean something else.
pub(crate) fn pointer_event(event: &WireEvent, scale: f32) -> Option<SurfaceEvent> {
    match *event {
        WireEvent::PointerEnter { x, y } => Some(SurfaceEvent::PointerEnter {
            x: px(x, scale),
            y: px(y, scale),
        }),
        WireEvent::PointerLeave => Some(SurfaceEvent::PointerLeave),
        WireEvent::PointerMotion { x, y } => Some(SurfaceEvent::PointerMotion {
            x: px(x, scale),
            y: px(y, scale),
        }),
        WireEvent::PointerButton {
            x,
            y,
            button,
            pressed,
        } => {
            let button = button_number(button)?;
            Some(SurfaceEvent::PointerButton {
                x: px(x, scale),
                y: px(y, scale),
                button,
                pressed,
            })
        }
        WireEvent::PointerAxis {
            x,
            y,
            axis,
            value,
            discrete,
        } => axis_event(axis, value, discrete, px(x, scale), px(y, scale)),
        _ => None,
    }
}

/// Translates one scroll event onto the contract vocabulary.
///
/// The sign is the protocol's and the contract's at once: a positive vertical value is a
/// downward scroll, and the X11 backend reports the same direction as `+1`, so the candidate
/// list moves the same way whichever backend is running.
pub(crate) fn axis_event(
    axis: PointerAxis,
    value: f64,
    discrete: i32,
    x: i32,
    y: i32,
) -> Option<SurfaceEvent> {
    let horizontal = match axis {
        PointerAxis::Vertical => false,
        PointerAxis::Horizontal => true,
        PointerAxis::Other => return None,
    };
    let delta = steps(discrete, value);
    if delta == 0 {
        return None;
    }
    Some(SurfaceEvent::Axis {
        x,
        y,
        delta,
        horizontal,
    })
}

/// How far one wire event moves the candidate list.
///
/// One event is one step. A wheel reports a discrete count, a touchpad reports a continuous
/// value and a high-resolution wheel reports 120ths of a step; all three mean "the user
/// scrolled", and the granularity the contract asks for is a single step either way. Clamping
/// to one keeps a fast flick from jumping the list, which is the same granularity the X11
/// backend's button 4 and 5 presses have.
fn steps(discrete: i32, value: f64) -> i32 {
    if discrete != 0 {
        return discrete.signum();
    }
    if value > 0.0 {
        1
    } else if value < 0.0 {
        -1
    } else {
        0
    }
}

/// Maps a Linux input event code onto the button number the contract's vocabulary uses.
///
/// The X11 backend reports X11's button numbers -- 1 left, 2 middle, 3 right -- and a caller
/// that matches on `button` must see the same value whichever backend is running, so the same
/// numbering is used here. Wayland reports the kernel's `BTN_*` codes instead; a code with no
/// counterpart yields `None`, because the candidate window acts on the left button (select)
/// and the right one (cancel), and guessing at the rest would be worse than dropping them.
pub(crate) fn button_number(code: u32) -> Option<u8> {
    match code {
        0x110 => Some(1), // BTN_LEFT
        0x112 => Some(2), // BTN_MIDDLE
        0x111 => Some(3), // BTN_RIGHT
        0x113 => Some(8), // BTN_SIDE
        0x114 => Some(9), // BTN_EXTRA
        _ => None,
    }
}

/// Converts a surface-local coordinate into a physical pixel.
///
/// A non-finite value is reported as the origin rather than propagated: it cannot be a
/// position, and a `NaN` cast to an integer is undefined behaviour on some targets.
fn px(value: f64, scale: f32) -> i32 {
    let scale = f64::from(crate::platform::normalize_scale(scale));
    let scaled = (value * scale).round();
    if !scaled.is_finite() {
        return 0;
    }
    scaled.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pointer_positions_are_reported_in_physical_pixels() {
        let event = WireEvent::PointerMotion { x: 12.0, y: 4.5 };
        assert_eq!(
            pointer_event(&event, 2.0),
            Some(SurfaceEvent::PointerMotion { x: 24, y: 9 })
        );
        assert_eq!(
            pointer_event(&event, 1.0),
            Some(SurfaceEvent::PointerMotion { x: 12, y: 5 })
        );
        let enter = WireEvent::PointerEnter { x: 0.0, y: 0.0 };
        assert_eq!(
            pointer_event(&enter, 1.5),
            Some(SurfaceEvent::PointerEnter { x: 0, y: 0 })
        );
        assert_eq!(
            pointer_event(&WireEvent::PointerLeave, 1.0),
            Some(SurfaceEvent::PointerLeave)
        );
    }

    #[test]
    fn test_pointer_positions_survive_an_unusable_scale() {
        let event = WireEvent::PointerMotion {
            x: f64::NAN,
            y: f64::INFINITY,
        };
        assert_eq!(
            pointer_event(&event, f32::NAN),
            Some(SurfaceEvent::PointerMotion { x: 0, y: 0 }),
            "a position that is not a number is reported as the origin"
        );
    }

    #[test]
    fn test_pointer_buttons_use_the_contract_numbering() {
        let press = WireEvent::PointerButton {
            x: 1.0,
            y: 2.0,
            button: 0x110,
            pressed: true,
        };
        assert_eq!(
            pointer_event(&press, 1.0),
            Some(SurfaceEvent::PointerButton {
                x: 1,
                y: 2,
                button: 1,
                pressed: true
            })
        );
        let release = WireEvent::PointerButton {
            x: 1.0,
            y: 2.0,
            button: 0x111,
            pressed: false,
        };
        assert_eq!(
            pointer_event(&release, 1.0),
            Some(SurfaceEvent::PointerButton {
                x: 1,
                y: 2,
                button: 3,
                pressed: false
            })
        );
        let middle = WireEvent::PointerButton {
            x: 0.0,
            y: 0.0,
            button: 0x112,
            pressed: true,
        };
        assert_eq!(
            pointer_event(&middle, 1.0),
            Some(SurfaceEvent::PointerButton {
                x: 0,
                y: 0,
                button: 2,
                pressed: true
            })
        );
    }

    #[test]
    fn test_pointer_buttons_without_a_contract_number_are_dropped() {
        let unknown = WireEvent::PointerButton {
            x: 0.0,
            y: 0.0,
            button: 0x150,
            pressed: true,
        };
        assert_eq!(pointer_event(&unknown, 1.0), None);
        assert_eq!(button_number(0x110), Some(1));
        assert_eq!(button_number(0x150), None);
    }

    #[test]
    fn test_scroll_direction_matches_the_x11_backend() {
        let down = WireEvent::PointerAxis {
            x: 3.0,
            y: 4.0,
            axis: PointerAxis::Vertical,
            value: 15.0,
            discrete: 0,
        };
        assert_eq!(
            pointer_event(&down, 1.0),
            Some(SurfaceEvent::Axis {
                x: 3,
                y: 4,
                delta: 1,
                horizontal: false
            }),
            "a positive vertical value pages forward, as X11's button 5 does"
        );
        let up = WireEvent::PointerAxis {
            x: 3.0,
            y: 4.0,
            axis: PointerAxis::Vertical,
            value: -15.0,
            discrete: 0,
        };
        assert_eq!(
            pointer_event(&up, 1.0),
            Some(SurfaceEvent::Axis {
                x: 3,
                y: 4,
                delta: -1,
                horizontal: false
            })
        );
        let right = WireEvent::PointerAxis {
            x: 0.0,
            y: 0.0,
            axis: PointerAxis::Horizontal,
            value: 1.0,
            discrete: 1,
        };
        assert_eq!(
            pointer_event(&right, 1.0),
            Some(SurfaceEvent::Axis {
                x: 0,
                y: 0,
                delta: 1,
                horizontal: true
            })
        );
    }

    #[test]
    fn test_scroll_is_one_step_per_event_whatever_the_device() {
        // A high-resolution wheel reports a fraction of a step; a touchpad reports a small
        // continuous value; a mouse wheel reports whole steps. All of them move the list once.
        assert_eq!(steps(0, 0.5), 1);
        assert_eq!(steps(0, 8.0), 1);
        assert_eq!(steps(3, 8.0), 1);
        assert_eq!(steps(-3, -8.0), -1);
        assert_eq!(steps(0, 0.0), 0, "a zero-value axis event is not a scroll");
    }

    #[test]
    fn test_unknown_axes_and_empty_events_produce_nothing() {
        let other = WireEvent::PointerAxis {
            x: 0.0,
            y: 0.0,
            axis: PointerAxis::Other,
            value: 10.0,
            discrete: 1,
        };
        assert_eq!(pointer_event(&other, 1.0), None);
        assert_eq!(pointer_event(&WireEvent::FrameDone { data: 7 }, 1.0), None);
        assert_eq!(
            pointer_event(&WireEvent::KeyboardEnter, 1.0),
            None,
            "a keyboard event is not pointer input"
        );
        assert_eq!(
            axis_event(PointerAxis::Vertical, 0.0, 0, 0, 0),
            None,
            "an axis event with no movement is not a scroll"
        );
    }
}
