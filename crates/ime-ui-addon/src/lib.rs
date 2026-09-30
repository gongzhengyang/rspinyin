//! rspinyin user-interface addon.
//!
//! The second of the two cdylibs ADR-0003 requires. Fcitx5 picks the active user
//! interface from the addons it discovered with `Category=UI`, and an addon belongs to
//! exactly one category, so the input-method addon cannot also be the user interface.
//! This crate is the `UI` half: it reads the input panel the host routes to it, resolves
//! the caret into a screen anchor, and draws the candidate window itself.
//!
//! # Relationship to the engine addon
//!
//! `librspinyin.so` and `librspinyin_ui.so` are `dlopen`'d independently and share no
//! static state and no IPC. Everything they have in common travels through Fcitx5's own
//! interfaces: the engine writes preedit and candidates into the `InputContext`, and
//! this side reads them back from `InputContext::inputPanel()` in the
//! `UserInterface::update` callback. That is how Fcitx5's own candidate window works,
//! and it is why neither library needs to know the other exists.
//!
//! # Raw-pointer boundary
//!
//! Only `src/ffi/**` in this crate may contain raw-pointer code or `extern "C"`
//! declarations, and it is the only place that could unwind across the FFI boundary.
//! Every entry point catches panics and reports failure instead, because unwinding into
//! C++ is undefined behaviour.
//!
//! # Rendering
//!
//! The view layer is [`ime_ui`]: the software rasteriser, the Slint platform object and
//! the X11 / Wayland surface backends. This crate owns the *host* half of the window —
//! when it appears, where it is anchored, and when the host has suspended it.
//!
//! The `fcitx5-host` feature gates the real C ABI link so the pure-Rust build and its
//! tests run on machines without the Fcitx5 development packages installed.

pub mod addon;
pub mod cursor;
pub mod platform;
pub mod screen;
pub mod ui_impl;

// The `unsafe_code` allowance is scoped to this one module declaration rather than
// applied crate-wide, so raw pointers stay confined to the FFI boundary the architecture
// rules and the unsafe audit both name.
#[allow(unsafe_code)]
pub mod ffi;
