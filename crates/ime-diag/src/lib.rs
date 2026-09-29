//! rspinyin diagnostics layer.
//!
//! Structured logging, crash capture, and performance probes.
//!
//! This crate is the only place that installs a `tracing` subscriber. Business crates
//! emit events through the `tracing` macros and rely on this layer to redact them.
//!
//! # Privacy contract
//!
//! The redaction layer is a defensive second line, not a licence to log user content.
//! Input strings, preedit text, candidate text, and commit text must never be passed
//! to a log event in the first place; application identifiers are logged as hashes and
//! home-directory prefixes are rewritten. Diagnostics substitute structural facts —
//! candidate counts, source distributions, DAG sizes — for the content itself.

pub mod crash;
pub mod log;
pub mod panic;
pub mod perms;
pub mod redact;
