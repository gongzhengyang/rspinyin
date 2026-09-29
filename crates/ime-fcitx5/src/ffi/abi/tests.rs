//! Tests for the frozen ABI surface: the version handshake, the callback table, the
//! raw-buffer readers, the engine callbacks and the panic guard every entry point runs
//! under.
//!
//! Every test drives the entry points directly, through the same panic guard the host
//! goes through, so no test needs a Fcitx5 process, a display server or a dictionary.

use std::ffi::c_void;
use std::panic::panic_any;
use std::ptr;

use ime_types::ImeError;

use crate::ffi::{PanicReport, catch_ffi, guard_ffi, guard_ffi_with};

use super::lifecycle::{PluginContext, Registration, register};

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

/// Runs the registration step the entry point runs, against a table carrying
/// `abi_version`, and returns the line a refusal was reported with.
///
/// The production entry point hands the process-wide context its own table, which carries
/// the matching version, so the refusal branch is reachable only from here — with a
/// context and a table of the test's own, through the same function the host's call
/// reaches. The two other halves of the same condition are asserted where they are
/// decided: the handshake stays rejected, and the addon that depends on it is declined.
fn registration_with(abi_version: u32) -> String {
    let context = PluginContext::new();
    let table = vtable_with_abi_version(abi_version);
    let refused = match register(&context, &table) {
        Registration::Refused(line) => line,
        Registration::Accepted(_) => panic!("a table with the wrong ABI version was accepted"),
    };
    assert!(
        !context.handshake.is_accepted(),
        "a refused table must leave the handshake rejected"
    );
    assert!(
        !on_addon_init(context_pointer(&context)),
        "the addon behind a refused handshake must be declined"
    );
    refused
}

#[test]
fn test_register_refuses_a_mismatched_abi_version_and_reports_the_frozen_code() {
    let host = RSPINYIN_ABI_VERSION + 1;
    let line = registration_with(host);
    let expected =
        format!("platform/fcitx5/version-mismatch: host={host} required={RSPINYIN_ABI_VERSION}");
    assert_eq!(
        line, expected,
        "the refusal reaches the crash channel under the frozen code, and the code is \
         what diagnostics and tests match on"
    );
}

#[test]
fn test_register_refuses_an_older_abi_version_as_well_as_a_newer_one() {
    // The comparison is equality, not ordering: a table from an older build is refused
    // for the same reason a newer one is, because neither layout is the one this side
    // was compiled against.
    let line = registration_with(RSPINYIN_ABI_VERSION.wrapping_sub(1));
    assert!(
        line.starts_with("platform/fcitx5/version-mismatch"),
        "an older table must be reported the same way: {line}"
    );
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
fn test_guard_ffi_records_a_contained_panic_on_the_crash_channel() {
    // The other half of the guard's contract: a panic is not only turned into a value, it
    // is written where an operator can find it. The sink is the guard's own seam, so this
    // is the line the production path hands to the host's log.
    let mut written: Vec<String> = Vec::new();
    let result = guard_ffi_with(
        false,
        |line: &str| written.push(line.to_owned()),
        || panic_any("the engine blew up"),
    );
    assert!(!result, "a panicking body must yield the fallback value");
    assert_eq!(
        written,
        [String::from("rspinyin: ffi/panic: the engine blew up")],
        "the crash line carries the code and the panic's own message"
    );
}

/// A vtable slot that panics, written the way every production entry point is: the body
/// runs under the guard, so the panic becomes the slot's fallback value.
extern "C" fn panicking_key_event(
    _context: *mut c_void,
    _ic_id: u64,
    _event: *const FcitxKeyEvent,
) -> bool {
    guard_ffi(false, || panic_any("the engine blew up"))
}

#[test]
fn test_a_panicking_vtable_slot_answers_its_fallback_instead_of_unwinding() {
    // The one entry point a test can make panic: it replaces the slot with one that does.
    // What has to hold is the property the guard exists for — a panic raised inside a
    // callback becomes the callback's documented fallback value instead of unwinding into
    // the C++ caller, which is undefined behaviour — and that the boundary keeps working
    // afterwards.
    let table = RspinyinVtable {
        on_key_event: panicking_key_event,
        ..RSPINYIN_VTABLE
    };
    let handled = (table.on_key_event)(ptr::null_mut(), 7, ptr::null());
    assert!(
        !handled,
        "the fallback value must be what crosses the boundary"
    );

    let event = FcitxKeyEvent {
        sym: 0x61,
        state: 0,
        is_release: false,
        time_ms: 5,
    };
    assert!(
        !on_key_event(ptr::null_mut(), 7, ptr::from_ref(&event)),
        "a contained panic must leave the entry points answering as they did"
    );
}

/// `catch_unwind` is a no-op under `panic = "abort"`, and the process would abort instead
/// of returning the entry point's fallback -- the failure mode the whole boundary exists
/// to prevent. The workspace profile states the requirement in a comment; this is what
/// holds it to the profile the test binary was built with.
///
/// It is a `const` assertion rather than a `#[test]` because the answer is a property of
/// the compilation, not of the run: a build with `panic = "abort"` should not produce a
/// test binary at all, let alone one whose only failing test is this.
#[allow(dead_code)]
const THE_PROFILE_UNWINDS: () = assert!(
    cfg!(panic = "unwind"),
    "the FFI panic guard needs a profile that unwinds"
);

#[test]
fn test_every_vtable_slot_tolerates_a_null_context() {
    // The host hands the context back unchanged, so a null one is the degenerate case of
    // a wiring defect rather than a value any callback may trust. Every slot is driven
    // with one: the rule is that no entry point may unwind, and the ones that answer a
    // value must answer their documented fallback.
    let outcome = std::panic::catch_unwind(|| {
        assert!(!on_addon_init(ptr::null_mut()));
        on_addon_destroy(ptr::null_mut());
        assert!(!on_activate(ptr::null_mut(), 0));
        on_deactivate(ptr::null_mut(), 0);
        on_reset(ptr::null_mut(), 0);
        assert!(!on_key_event(ptr::null_mut(), 0, ptr::null()));
        on_focus_in(ptr::null_mut(), 0);
        on_focus_out(ptr::null_mut(), 0);
        on_commit_string(ptr::null_mut(), 0, ptr::null(), 0);
        on_set_preedit(ptr::null_mut(), 0, ptr::null(), 0, 0);
        on_clear_preedit(ptr::null_mut(), 0);
    });
    assert!(
        outcome.is_ok(),
        "no entry point may unwind on a null context"
    );
}

/// The sources that declare an `extern "C"` entry point.
///
/// The panic guard cannot be observed at run time: the only way to make a production
/// entry point panic is to ship the injection that makes it panic. What can be checked is
/// the property the guard's contract states — every `extern "C"` body opens with
/// `guard_ffi` — and it is checked against the sources that ship, so an entry point added
/// without the guard fails here instead of unwinding into C++.
const ENTRY_SOURCES: [(&str, &str); 3] = [
    ("ffi/abi.rs", include_str!("../abi.rs")),
    ("ffi/abi/engine.rs", include_str!("engine.rs")),
    ("ffi/abi/lifecycle.rs", include_str!("lifecycle.rs")),
];

/// Whether `line` defines an `extern "C"` function rather than naming one as a type.
///
/// Two shapes have to be told from a definition. A field of the callback table spells the
/// same two words followed by a signature (`extern "C" fn(*mut c_void) -> bool`), and a
/// comment may name a definition while describing it; both are answered `false`, the first
/// because a definition has a name between `fn` and the parenthesis, the second because a
/// comment line is not code.
fn defines_extern_c_function(line: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.starts_with("//") {
        return false;
    }
    let Some(rest) = trimmed.split("extern \"C\" fn").nth(1) else {
        return false;
    };
    rest.trim_start()
        .starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
}

#[test]
fn test_every_ffi_entry_point_runs_under_the_panic_guard() {
    let mut entries = 0;
    for (name, source) in ENTRY_SOURCES {
        for (index, line) in source.lines().enumerate() {
            if !defines_extern_c_function(line) {
                continue;
            }
            entries += 1;
            // The body's first statement, comments skipped: the guard has to be what the
            // body does first, and a comment above it is not a statement. The search starts
            // from the line that opens the body rather than from the line after the `fn`,
            // because a signature may span several lines and a body read from one line below
            // the `fn` would report a parameter as its first statement.
            let mut lines = source.lines().skip(index);
            let opening = lines.by_ref().find(|line| line.contains('{'));
            let after_brace = opening
                .and_then(|line| line.split_once('{'))
                .map_or("", |(_, rest)| rest);
            let body = std::iter::once(after_brace)
                .chain(lines)
                .map(str::trim_start)
                .find(|line| !line.is_empty() && !line.starts_with("//"));
            assert!(
                body.is_some_and(|line| line.starts_with("guard_ffi(")),
                "{name}:{}: an `extern \"C\"` body must open with the panic guard",
                index + 1
            );
        }
    }
    assert_eq!(
        entries, 13,
        "the entry points this walks are the ones that ship: the nine engine callbacks \
         and the four exported symbols"
    );
}

#[test]
fn test_defines_extern_c_function_tells_a_definition_from_a_table_field() {
    // The walk above is only as good as this predicate: a field of the callback table
    // would otherwise be counted as an entry point and the count would drift silently.
    for definition in [
        "pub extern \"C\" fn on_activate(_context: *mut c_void) -> bool {",
        "    pub extern \"C\" fn fcitx_addon_factory_instance() -> *mut c_void {",
    ] {
        assert!(defines_extern_c_function(definition), "{definition}");
    }
    for other in [
        "    pub on_activate: extern \"C\" fn(*mut c_void, u64) -> bool,",
        "unsafe extern \"C\" {",
        "    // forwards to extern \"C\" fn rspinyin_plugin_init() in the glue",
    ] {
        assert!(!defines_extern_c_function(other), "{other}");
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
