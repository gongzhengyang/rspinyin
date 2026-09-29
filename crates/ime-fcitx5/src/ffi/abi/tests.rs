//! Tests for the frozen ABI surface: the version handshake, the callback table, the
//! raw-buffer readers and the engine callbacks.
//!
//! Every test drives the entry points directly, through the same panic guard the host
//! goes through, so no test needs a Fcitx5 process, a display server or a dictionary.

use std::ffi::c_void;
use std::panic::panic_any;
use std::ptr;

use ime_types::ImeError;

use crate::ffi::{PanicReport, catch_ffi, guard_ffi};

use super::lifecycle::PluginContext;

use super::*;

/// Builds the production table with a different ABI version.
fn vtable_with_abi_version(abi_version: u32) -> RspinyinVtable {
    RspinyinVtable {
        abi_version,
        ..RSPINYIN_VTABLE
    }
}

/// Points at `context` the way the host does.
fn context_pointer(context: &PluginContext) -> *mut c_void {
    ptr::from_ref(context).cast_mut().cast::<c_void>()
}

#[test]
fn test_vtable_declares_the_frozen_abi_version() {
    assert_eq!(RSPINYIN_VTABLE.abi_version, RSPINYIN_ABI_VERSION);
}

#[test]
fn test_handshake_accepts_vtable_with_matching_abi_version() {
    let handshake = RspinyinHandshake::new();
    assert!(!handshake.is_accepted());
    assert!(handshake.accept(&RSPINYIN_VTABLE).is_ok());
    assert!(handshake.is_accepted());
}

#[test]
fn test_handshake_rejects_vtable_with_mismatched_abi_version() {
    let handshake = RspinyinHandshake::new();
    let newer = vtable_with_abi_version(RSPINYIN_ABI_VERSION + 1);
    let result = handshake.accept(&newer);
    assert!(result.is_err(), "a mismatched ABI version must be rejected");
    assert!(!handshake.is_accepted());
    if let Err(err) = result {
        assert!(matches!(err, ImeError::Fcitx5VersionMismatch { .. }));
        assert!(
            err.to_string()
                .starts_with("platform/fcitx5/version-mismatch")
        );
    }
}

#[test]
fn test_on_addon_init_returns_false_after_mismatched_abi_version() {
    let context = PluginContext::new();
    let older = vtable_with_abi_version(RSPINYIN_ABI_VERSION.wrapping_sub(1));
    assert!(context.handshake.accept(&older).is_err());
    assert!(!on_addon_init(context_pointer(&context)));
}

#[test]
fn test_on_addon_init_returns_true_after_accepted_handshake() {
    let context = PluginContext::new();
    assert!(context.handshake.accept(&RSPINYIN_VTABLE).is_ok());
    assert!(on_addon_init(context_pointer(&context)));
}

#[test]
fn test_on_addon_init_returns_false_for_null_context() {
    assert!(!on_addon_init(ptr::null_mut()));
}

#[test]
fn test_guard_ffi_returns_the_body_value_when_it_succeeds() {
    assert!(guard_ffi(false, || true));
    assert_eq!(guard_ffi(0, || 7), 7);
}

#[test]
fn test_guard_ffi_returns_fallback_when_the_body_panics() {
    let mut body_ran = false;
    let result = guard_ffi(false, || {
        body_ran = true;
        panic_any("engine blew up");
    });
    assert!(body_ran, "the guarded body must have run");
    assert!(!result, "a panicking body must yield the fallback value");
}

#[test]
fn test_guard_ffi_reports_string_panic_payloads() {
    // Both string shapes a panic payload can take must reach the crash log.
    let static_str: Result<(), PanicReport> = catch_ffi(|| panic_any("boom"));
    assert!(static_str.is_err());
    if let Err(report) = static_str {
        assert_eq!(report.message, "boom");
    }

    let owned: Result<(), PanicReport> = catch_ffi(|| panic_any(String::from("bang")));
    assert!(owned.is_err());
    if let Err(report) = owned {
        assert_eq!(report.message, "bang");
    }
}

#[test]
fn test_guard_ffi_reports_non_string_panic_payload() {
    let report: Result<(), PanicReport> = catch_ffi(|| panic_any(42u32));
    assert!(report.is_err());
    if let Err(report) = report {
        assert_eq!(report.message, "<non-string panic payload>");
        assert!(report.crash_line().starts_with("rspinyin: ffi/panic:"));
    }
}

#[test]
fn test_bytes_from_raw_rejects_null_pointer() {
    // SAFETY: a null pointer with a zero length is the invalid shape under test;
    // no bytes are read.
    let bytes = unsafe { bytes_from_raw(ptr::null(), 0) };
    assert!(bytes.is_none());
}

#[test]
fn test_bytes_from_raw_rejects_zero_length() {
    let host_buffer = *b"ab";
    // SAFETY: the pointer is valid for the two bytes that exist; the length is
    // what makes the shape invalid, so nothing is read.
    let bytes = unsafe { bytes_from_raw(host_buffer.as_ptr(), 0) };
    assert!(bytes.is_none());
}

#[test]
fn test_bytes_from_raw_borrows_host_bytes() {
    let host_buffer = *b"abc";
    // SAFETY: the pointer and length describe exactly the live array above, and
    // the borrow does not outlive it.
    let bytes = unsafe { bytes_from_raw(host_buffer.as_ptr(), host_buffer.len()) };
    assert_eq!(bytes, Some(&host_buffer[..]));
}

#[test]
fn test_on_key_event_returns_false_so_the_key_is_not_swallowed() {
    let event = FcitxKeyEvent {
        sym: 0x61,
        state: 0,
        is_release: false,
        time_ms: 5,
    };
    assert!(!on_key_event(ptr::null_mut(), 7, ptr::from_ref(&event)));
}

#[test]
fn test_on_key_event_returns_false_for_null_event() {
    assert!(!on_key_event(ptr::null_mut(), 7, ptr::null()));
}

#[cfg(not(fcitx5_host))]
#[cfg(not(fcitx5_host))]
#[test]
fn test_on_commit_string_ignores_invalid_buffer_without_panicking() {
    let commit = || on_commit_string(ptr::null_mut(), 7, ptr::null(), 0);
    let outcome = std::panic::catch_unwind(commit);
    assert!(
        outcome.is_ok(),
        "an invalid commit must be diagnosed, not panicked"
    );
}

#[test]
fn test_on_set_preedit_ignores_invalid_buffer_without_panicking() {
    let set_preedit = || on_set_preedit(ptr::null_mut(), 7, ptr::null(), 0, 0);
    let outcome = std::panic::catch_unwind(set_preedit);
    assert!(
        outcome.is_ok(),
        "an invalid preedit must be diagnosed, not panicked"
    );
}

#[cfg(fcitx5_host)]
#[test]
fn test_plugin_init_registers_the_vtable_with_the_host_glue() {
    let context = rspinyin_plugin_init();
    assert!(
        !context.is_null(),
        "the host glue must accept a table with the frozen ABI version"
    );
    assert!(on_addon_init(context));
}
