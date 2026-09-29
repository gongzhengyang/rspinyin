//! rspinyin view layer.
//!
//! Custom Slint `Platform`, software rasterizer, candidate-window geometry, and
//! spring-based animation.
//!
//! # Licensing constraint
//!
//! **This crate's public API must not export any Slint type.** Slint types may appear
//! only inside private modules, so the candidate window is never a Slint surface that
//! third parties could program against. This is a condition of the Slint royalty-free
//! licence, not merely a style preference, and it is enforced in CI by the
//! Slint-leak audit script.

pub mod adapter;
pub mod channel;
pub mod geometry;
pub mod interaction;
pub mod layout;
pub mod platform;
pub mod renderer;
pub mod slint_platform;
pub mod spring;
pub mod surface;
pub mod theme;
pub mod ui_thread;

/// The Slint-generated bindings for the candidate window.
///
/// Private, and that is the whole point: everything `include_modules!` expands to stays
/// unreachable from outside this crate, so no `slint::` type can appear in the public API
/// (ADR-0000 `OB-4`, enforced by `scripts/check-slint-leak.sh`).
///
/// The lints are relaxed because this module is generated: `build.rs` compiles
/// `ui/candidate.slint` and the expansion is not written to satisfy hand-written style
/// rules. It carries its own `@generated` marker, which is also what exempts it from the
/// line limit.
///
/// `clippy::all` is not enough on its own. This workspace raises three `restriction`-group
/// lints to errors (`unwrap_used`, `expect_used`, `panic`, see the root `Cargo.toml`), and
/// `restriction` is disjoint from `all`, so the generated `unwrap()` calls on Slint's
/// `Option`-returning property accessors would fail the gate. They are named explicitly
/// rather than covered by a blanket `clippy::restriction`, which would also silence the
/// lints that still make sense here.
#[allow(
    unsafe_code,
    clippy::all,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]
mod ui_generated {
    slint::include_modules!();
}
