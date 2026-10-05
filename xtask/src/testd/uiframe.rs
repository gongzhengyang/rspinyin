//! The frame snapshot channel, re-exported from the diagnostics crate.
//!
//! The codec and the writer live in `ime_diag::uiframe`, so the plugin's publish hook and
//! this harness read and write one format instead of two transcriptions of it. Everything
//! the test platform names -- [`FrameSnapshot`], [`UiFrameMirror`], the view types -- is
//! the moved module's public surface, unchanged.
//!
//! The channel's consumer is `xtask test-mirror` (`crate::testd::mirror_cli`), which reads
//! through this re-export, so the re-export carries no dead-code allowance of its own any
//! more.

pub use ime_diag::uiframe::*;
