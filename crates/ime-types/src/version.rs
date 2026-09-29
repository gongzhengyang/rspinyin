//! Frozen version numbers and the ABI handshake.
//!
//! Three independent versions exist and must not be confused:
//!
//! - `RSPINYIN_ABI_VERSION` versions the Rust / C++ vtable contract. It is
//!   checked before anything is registered, and a mismatch makes the plugin
//!   refuse to load rather than run against a vtable layout it does not know.
//! - `DICT_FORMAT_VERSION` versions the compiled dictionary container.
//! - `CONFIG_SCHEMA_VERSION` versions `config.toml`.
//!
//! This module is part of the frozen contract: changing it requires an ADR under
//! `docs/dev/adr/`.

use crate::error::ImeError;

/// ABI version of the Rust / C++ vtable contract.
///
/// Bumped whenever a vtable's layout changes: a C struct has no other
/// compatibility mechanism, so this version is what stops an old host from
/// calling into a newer plugin.
///
/// Version 2 is the two-addon split (ADR-0003, ADR-0004). The engine's table lost
/// the two user-interface slots, `on_input_panel_update` and `on_cursor_rect`,
/// which became the first two entries of the user-interface addon's own table in
/// `crates/ime-ui-addon`. Both tables carry this same constant: it identifies the
/// revision of the project's Rust/C++ contract, and the two libraries are built
/// with their own glue so they can never disagree at run time.
pub const RSPINYIN_ABI_VERSION: u32 = 2;

/// Format version of the compiled dictionary container.
///
/// A file carrying a greater version is rejected by the reader, which then
/// disables candidate display while leaving pass-through input working.
pub const DICT_FORMAT_VERSION: u16 = 1;

/// Schema version of `config.toml`.
///
/// Moved 1 -> 2 by ADR-0005. Version 2 adds the `[scheme]`, `[phrases]` and
/// `[script]` sections and the `[data]` keys; a version 1 document is migrated
/// forward on load rather than rejected.
pub const CONFIG_SCHEMA_VERSION: u16 = 2;

/// Verifies that the host offers the ABI this build was compiled against.
///
/// Called before the plugin registers anything with the host. A mismatch is
/// reported through `ImeError::Fcitx5VersionMismatch` so that the frozen error
/// list gains no extra code: `host` carries the ABI version the host announced
/// and `required` the one this build requires.
///
/// # Errors
///
/// Returns `ImeError::Fcitx5VersionMismatch` when `host` differs from
/// `RSPINYIN_ABI_VERSION`.
///
/// # Examples
///
/// ```
/// use ime_types::version::{RSPINYIN_ABI_VERSION, check_abi};
///
/// assert!(check_abi(RSPINYIN_ABI_VERSION).is_ok());
/// assert!(check_abi(RSPINYIN_ABI_VERSION + 1).is_err());
/// ```
pub fn check_abi(host: u32) -> Result<(), ImeError> {
    if host == RSPINYIN_ABI_VERSION {
        Ok(())
    } else {
        Err(ImeError::Fcitx5VersionMismatch {
            host: host.to_string(),
            required: RSPINYIN_ABI_VERSION.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_abi_matching_host_succeeds() {
        assert!(check_abi(RSPINYIN_ABI_VERSION).is_ok());
    }

    #[test]
    fn test_check_abi_newer_host_returns_version_mismatch() {
        let host = RSPINYIN_ABI_VERSION.wrapping_add(1);
        let result = check_abi(host);
        assert!(result.is_err());
        if let Err(err) = result {
            assert!(matches!(err, ImeError::Fcitx5VersionMismatch { .. }));
            let prefix = "platform/fcitx5/version-mismatch";
            let expected = format!("{prefix}: host={host} required={RSPINYIN_ABI_VERSION}");
            assert_eq!(err.to_string(), expected);
        }
    }

    #[test]
    fn test_check_abi_zero_host_returns_version_mismatch() {
        let result = check_abi(0);
        assert!(result.is_err());
        if let Err(err) = result {
            assert!(err.to_string().contains("host=0"));
        }
    }

    #[test]
    fn test_version_constants_match_frozen_values() {
        // These are the numbers a build is allowed to assume. Changing one is a contract
        // change: it needs an ADR and this assertion updated in the same commit, which is
        // the whole point of pinning them here rather than letting them drift.
        //
        // `RSPINYIN_ABI_VERSION` moved 1 -> 2 in ADR-0004, when the plugin split into two
        // cdylibs and the two UI-only slots left the engine's vtable, and
        // `CONFIG_SCHEMA_VERSION` 1 -> 2 in ADR-0005, which added the `[scheme]`,
        // `[phrases]` and `[script]` sections. `DICT_FORMAT_VERSION` is still at its
        // first version.
        assert_eq!(RSPINYIN_ABI_VERSION, 2);
        assert_eq!(DICT_FORMAT_VERSION, 1);
        assert_eq!(CONFIG_SCHEMA_VERSION, 2);
    }
}
