//! The frame mirror, re-exported from the diagnostics crate.
//!
//! The codec and the writer used to live here; they moved to `ime_diag::uiframe`
//! so the plugin's publish hook (the sink receive path, ADR-0011) and this harness
//! read and write one format instead of two transcriptions of it. Everything the
//! test platform names — [`FrameSnapshot`], [`UiFrameMirror`], the view types — is
//! the moved module's public surface, unchanged.
//!
//! The re-export is deliberately unfiltered and carries an `allow`: until the
//! `test-mirror` / `capture` subcommands are wired (the E2E verification card), no
//! xtask code names these items, but the paths must exist for that wiring to land
//! against.

#[allow(unused_imports)]
pub use ime_diag::uiframe::*;
