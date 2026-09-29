//! The frozen half of the contract: the ABI version, the registration handshake and the
//! `#[repr(C)]` payload mirrors the callbacks exchange.
//!
//! Responsibility: state what the two sides agree on, and nothing about what either side
//! does with it. The version constant is the workspace's single definition of which ABI
//! is current, the handshake is the check that both sides are running it, and the mirrors
//! are the values that cross the boundary.
//!
//! Boundaries: no entry point, no panic guard and no raw-pointer read live here. The
//! callback table itself — the type and the one instance the host caches — is in the
//! module root, and every mirror's field order is the ABI: the root's documentation
//! carries the append-only rule.

use std::sync::atomic::{AtomicBool, Ordering};

use ime_types::{ImeError, check_abi};

use super::RspinyinVtable;

/// ABI version of the Rust / C++ vtable contract.
///
/// Re-exported from the frozen contract crate so the workspace keeps a single
/// definition; the assertion below is what keeps this module honest if that
/// definition ever drifts from the version the C++ glue is compiled with.
pub use ime_types::RSPINYIN_ABI_VERSION;

// Compile-time guard on the frozen value. The C++ glue carries the same constant,
// so a change here without a matching change there is an ABI break, not a refactor.
//
// Version 2 is the two-addon split: the user-interface slots that used to live in the
// engine's table became `crates/ime-ui-addon`'s own table.
const _: () = assert!(RSPINYIN_ABI_VERSION == 2);

/// A key press or release as Fcitx5 delivers it to the input method engine.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FcitxKeyEvent {
    /// XKB keysym, passed through unchanged from `fcitx::Key::sym()`.
    pub sym: u32,
    /// Modifier bit mask, passed through unchanged from
    /// `fcitx::Key::states().toInteger()`.
    pub state: u32,
    /// `true` for a key release.
    pub is_release: bool,
    /// Frontend timestamp in milliseconds, 0 when the frontend reports none.
    pub time_ms: u32,
}

/// Registration state of the vtable handshake.
///
/// Both sides validate the version they receive: the C++ glue refuses to cache a
/// table whose `abi_version` differs from the version it was compiled with, and this
/// side refuses to hand the table over in the first place. Either rejection leaves
/// the addon in pure-engine mode instead of letting it run against a layout it does
/// not know.
#[derive(Debug)]
pub struct RspinyinHandshake {
    accepted: AtomicBool,
}

impl RspinyinHandshake {
    /// Creates a handshake that has not accepted a table yet.
    pub const fn new() -> Self {
        Self {
            accepted: AtomicBool::new(false),
        }
    }

    /// Validates `vt` and, when it passes, marks the handshake accepted.
    ///
    /// # Errors
    ///
    /// Returns `ImeError::Fcitx5VersionMismatch` when the table's `abi_version`
    /// differs from [`RSPINYIN_ABI_VERSION`]. The handshake stays rejected, so the
    /// callbacks that depend on it keep reporting the safe default.
    ///
    /// # Examples
    ///
    /// ```
    /// use rspinyin::ffi::{RSPINYIN_VTABLE, RspinyinHandshake};
    ///
    /// let handshake = RspinyinHandshake::new();
    /// assert!(handshake.accept(&RSPINYIN_VTABLE).is_ok());
    /// assert!(handshake.is_accepted());
    /// ```
    pub fn accept(&self, vt: &RspinyinVtable) -> Result<(), ImeError> {
        check_abi(vt.abi_version)?;
        self.accepted.store(true, Ordering::Release);
        Ok(())
    }

    /// Whether a table with a matching ABI version has been accepted.
    pub fn is_accepted(&self) -> bool {
        self.accepted.load(Ordering::Acquire)
    }
}

impl Default for RspinyinHandshake {
    fn default() -> Self {
        Self::new()
    }
}
