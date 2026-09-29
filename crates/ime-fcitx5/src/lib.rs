//! rspinyin host integration layer.
//!
//! Registers the plugin with Fcitx5 as an addon, takes over `UserInterface` so the
//! candidate window can be self-drawn, and translates between Fcitx5's C++ object
//! model and the frozen Rust contract in `ime-types`.
//!
//! # Raw-pointer boundary
//!
//! Only `src/ffi/**` in this crate may contain raw-pointer code or `extern "C"`
//! declarations. That module is also the only place that could unwind across the FFI
//! boundary, and it must not: every entry point catches panics and reports failure
//! instead, because unwinding into C++ is undefined behaviour.
//!
//! The `fcitx5-host` feature gates the real C ABI link so the pure-Rust build and its
//! tests run on machines without the Fcitx5 development packages installed.

pub mod addon;
pub mod cursor;
pub mod screen;

// The `unsafe_code` allowance is scoped to this one module declaration rather than
// applied crate-wide, so raw pointers stay confined to the FFI boundary the architecture
// rules and the unsafe audit both name.
#[allow(unsafe_code)]
pub mod ffi;
