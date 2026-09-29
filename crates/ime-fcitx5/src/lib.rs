//! rspinyin host integration layer.
//!
//! Registers the plugin with Fcitx5 as an addon, takes over `UserInterface` so the
//! candidate window can be self-drawn, and translates between Fcitx5's C++ object
//! model and the frozen Rust contract in `ime-types`.
//!
//! # Unsafe boundary
//!
//! Only `src/ffi/**` in this crate may contain `unsafe` or `extern "C"`. That module is
//! also the only place allowed to unwind across the FFI boundary, and it must not:
//! every entry point catches panics and reports failure instead, because unwinding
//! into C++ is undefined behaviour.
//!
//! The `fcitx5-host` feature gates the real C ABI link so the pure-Rust build and its
//! tests run on machines without the Fcitx5 development packages installed.
