//! The user-interface role: host-panel capture and the candidate-window takeover.
//!
//! Fcitx5 routes input-panel updates to the **active user interface** only. This plugin
//! stops the host from drawing its own candidate window by becoming that user
//! interface, not by disabling ClassicUI: an addon that hard-disables another addon
//! takes the candidates away from every other user of it. This module is the Rust half
//! of the role — the callbacks the C++ `fcitx::UserInterface` subclass forwards
//! (`src/ffi/cpp/ui_glue.cpp`), the state those callbacks leave behind, and the
//! takeover decision the addon lifecycle runs.
//!
//! # Budget
//!
//! Every callback here runs on the Fcitx5 host thread inside the host's own dispatch
//! and has 100us. Nothing on this path decodes, formats, resolves geometry or touches
//! the filesystem: the panel snapshot is copied into a latest-wins slot, the caret
//! rectangle is stored exactly as the host reported it, and the takeover decision
//! reads a handful of atomics. The panel slot keeps its two `String`s, so a
//! steady-state update allocates nothing.
//!
//! # What is deliberately not here
//!
//! The candidate list this plugin draws does **not** come from the host's candidate
//! list. The decoder builds a [`ime_types::UiFrame`] and the frame builder posts it, so
//! the host panel is captured for validation and as the fallback source, and no frame
//! is ever derived from it here. Resolving a caret rectangle into a screen
//! [`ime_types::Anchor`] is the caret ladder's job ([`crate::cursor`]), which runs where
//! an anchor is needed and not inside a host callback.
//!
//! # Takeover
//!
//! [`register_takeover`] is the whole policy: switch only when the self-drawn window
//! exists *and* a platform backend can host it, never when no backend can (that is the
//! ClassicUI fallback tier, where the window would have nowhere to appear), and never
//! a second time when the host kept another user interface active. The value a restore
//! needs is the previous user interface's name, which the outcome carries.
//!
//! Every branch reports a stable `domain/action/reason` code, exported here as
//! [`TAKEOVER_ACTIVE_CODE`], [`TAKEOVER_NOT_REGISTERED_CODE`] and
//! [`TAKEOVER_DECLINED_CODE`] — plus [`NO_BACKEND_CODE`] and [`NO_HOST_CODE`] for the
//! two conditions the takeover shares with the platform layer and the FFI boundary. A
//! caller records the line [`TakeoverOutcome::diagnostic`] returns; the takeover never
//! records anything itself. It does reach the host, though, so it belongs on the Fcitx5
//! host thread and not on a worker: see `takeover`'s module documentation.
//!
//! # Module map
//!
//! Each callback and the state behind it lives in the submodule it belongs to:
//! `availability` answers the host's availability query and follows suspend/resume,
//! `takeover` holds the switch policy and its diagnostics, `panel` captures the host's
//! input panel, and `cursor_rects` stores the caret rectangles the host reports. This
//! file is the module's public surface -- every name the rest of the crate reaches
//! through `crate::ui_impl` is re-exported here -- and nothing outside the module names
//! the submodules directly.

mod availability;
mod cursor_rects;
mod panel;
mod takeover;

#[cfg(test)]
mod tests;

pub use self::availability::{
    is_available, is_host_ui_suspended, on_host_resume, on_host_suspend,
    set_window_backend_available, window_backend_available,
};
pub use self::cursor_rects::{latest_cursor_rect, on_cursor_rect};
pub use self::panel::{PanelMirror, PanelUpdate, on_input_panel_update, panel_mirror};
pub use self::takeover::{
    NO_BACKEND_CODE, NO_HOST_CODE, TAKEOVER_ACTIVE_CODE, TAKEOVER_DECLINED_CODE,
    TAKEOVER_NOT_REGISTERED_CODE, TakeoverOutcome, register_takeover,
};
