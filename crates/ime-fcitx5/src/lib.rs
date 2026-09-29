//! rspinyin host integration layer.
//!
//! Registers the plugin with Fcitx5 as an input-method addon and translates between
//! Fcitx5's C++ object model and the frozen Rust contract in `ime-types`.
//!
//! # This crate is the engine addon only
//!
//! The user-interface role lives in `crates/ime-ui-addon`, which builds the second
//! cdylib Fcitx5 loads. The two are separate addons because Fcitx5 picks the active
//! user interface from the addons it discovered with `Category=UI`, and an addon
//! belongs to exactly one category — so one addon cannot be both the engine and the
//! user interface. See `docs/dev/adr/0003-ui-role-separate-addon.md`.
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
pub mod engine;
pub mod privacy_impl;

// The `unsafe_code` allowance is scoped to this one module declaration rather than
// applied crate-wide, so raw pointers stay confined to the FFI boundary the architecture
// rules and the unsafe audit both name.
#[allow(unsafe_code)]
pub mod ffi;
