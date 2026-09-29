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
