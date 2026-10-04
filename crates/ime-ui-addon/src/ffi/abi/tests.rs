//! Tests for the user-interface addon's C ABI.

use std::ptr;

use super::*;

#[test]
fn test_vtable_declares_the_version_the_glue_is_compiled_with() {
    // The C++ glue carries the same constant and refuses a table that disagrees, so a
    // bump on one side without the other is an ABI break rather than a refactor.
    assert_eq!(RSPINYIN_UI_VTABLE.abi_version, RSPINYIN_ABI_VERSION);
    assert_eq!(RSPINYIN_ABI_VERSION, 2);
}

#[test]
fn test_every_vtable_entry_is_a_callable() {
    // A null entry is what the glue's completeness check refuses; naming each one here
    // means a field added to the struct without an initialiser fails to compile rather
    // than reaching the glue as a null.
    let vt = &RSPINYIN_UI_VTABLE;
    let entries: [(bool, &str); 7] = [
        ((vt.on_addon_init as usize) != 0, "on_addon_init"),
        ((vt.on_addon_destroy as usize) != 0, "on_addon_destroy"),
        (
            (vt.on_input_panel_update as usize) != 0,
            "on_input_panel_update",
        ),
        ((vt.on_cursor_rect as usize) != 0, "on_cursor_rect"),
        ((vt.on_host_suspend as usize) != 0, "on_host_suspend"),
        ((vt.on_host_resume as usize) != 0, "on_host_resume"),
        ((vt.is_available as usize) != 0, "is_available"),
    ];
    for (is_set, name) in entries {
        assert!(is_set, "{name} must be set in the registered table");
    }
}

#[test]
fn test_activation_codes_map_and_unknown_ones_are_unavailable() {
    assert_eq!(activation_from_code(0), UiActivation::Active);
    assert_eq!(activation_from_code(1), UiActivation::NotRegistered);
    assert_eq!(activation_from_code(2), UiActivation::OtherUiActive);
    // A host newer than this build must not be able to make the plugin believe it took
    // over when it did not.
    assert_eq!(activation_from_code(3), UiActivation::Unavailable);
    assert_eq!(activation_from_code(u32::MAX), UiActivation::Unavailable);
}

#[test]
#[cfg(not(fcitx5_host))]
fn test_activate_ui_reports_unavailable_without_the_host_abi() {
    assert_eq!(activate_ui(), UiActivation::Unavailable);
    assert!(
        current_ui().is_none(),
        "no host is linked, so no user interface can be named"
    );
}

#[test]
#[cfg(fcitx5_host)]
fn test_activate_ui_reaches_the_host_abi_when_it_is_linked() {
    // With the glue linked there *is* a host to ask, so the answer must come from it
    // rather than from the pure-Rust stand-in. In a test binary no addon instance has
    // been created, so the manager has nothing to switch to and the honest answer is
    // `NotRegistered` — which is the distinction this test exists to pin: `Unavailable`
    // would mean the stand-in answered.
    assert_ne!(
        activate_ui(),
        UiActivation::Unavailable,
        "the linked glue must be reached, not the pure-Rust stand-in"
    );
}

#[test]
fn test_available_is_false_before_the_window_exists() {
    // Reporting `true` without a window would suppress ClassicUI while nothing draws in
    // its place, which is a worse failure than the default candidate window.
    assert!(!rspinyin_ui_available());
}

#[test]
fn test_suspend_and_resume_are_safe_without_a_host() {
    // Both run inside the panic guard, so they return `()` whatever happens; the point
    // of calling them is that a host which fires them before the window exists must not
    // take the process down.
    rspinyin_ui_suspend();
    rspinyin_ui_resume();
    assert!(
        !crate::ui_impl::is_host_ui_suspended(),
        "resume clears the suspension flag"
    );
}

#[test]
fn test_plugin_init_returns_a_stable_context() {
    let first = rspinyin_ui_plugin_init();
    let second = rspinyin_ui_plugin_init();
    assert!(!first.is_null(), "the handshake must hand over a context");
    assert_eq!(
        first, second,
        "the context is an identity token, so it must not move between calls"
    );
}

#[test]
#[cfg(not(fcitx5_host))]
fn test_factory_is_absent_without_the_host_abi() {
    // No glue is linked, so there is no factory to forward to. Null is what a host sees
    // for a library that contains no addon.
    assert!(fcitx_addon_factory_instance().is_null());
}

#[test]
#[cfg(fcitx5_host)]
fn test_factory_is_exported_with_the_host_abi() {
    // Fcitx5 resolves this name with `dlsym` after `dlopen`; a null or missing one is a
    // library that loads and contains no addon, which Fcitx5 reports as nothing at all.
    assert!(
        !fcitx_addon_factory_instance().is_null(),
        "the addon factory must be reachable once the glue is linked"
    );
}

#[test]
fn test_panel_update_refuses_a_null_snapshot() {
    assert!(!on_input_panel_update(
        ptr::null_mut(),
        1,
        ptr::null::<UiPanelSnapshot>()
    ));
}

#[test]
fn test_panel_update_reads_an_empty_panel_as_empty() {
    // The shape the host sends for a panel with no preedit and a hidden candidate list:
    // a null pointer with a zero length is empty, not invalid.
    let snapshot = UiPanelSnapshot {
        preedit_ptr: ptr::null(),
        preedit_len: 0,
        caret: 0,
        candidates_ptr: ptr::null(),
        candidates_len: 0,
        candidate_count: 0,
        cursor_index: -1,
        page: 0,
        total_pages: 0,
        page_size: 0,
    };
    assert!(
        !on_input_panel_update(ptr::null_mut(), 7, &snapshot),
        "a panel update is a state change to record, never a drawing instruction"
    );
    let mirror = crate::ui_impl::panel_mirror().expect("the panel must have been recorded");
    assert_eq!(mirror.ic, 7);
    assert!(mirror.preedit.is_empty());
    assert!(mirror.candidates.is_empty());
    assert_eq!(mirror.cursor_index, -1);
}

#[test]
fn test_bytes_from_raw_allow_empty_distinguishes_empty_from_invalid() {
    // SAFETY: a null pointer with a zero length is the empty-buffer shape this reader
    // exists to accept; no memory is read.
    let empty = unsafe { bytes_from_raw_allow_empty(ptr::null(), 0) };
    assert_eq!(empty, Some(&[][..]), "a zero length is an empty buffer");
    // SAFETY: as above — the reader refuses the pair before dereferencing anything, so
    // the call is safe even though the pointer is null.
    let invalid = unsafe { bytes_from_raw_allow_empty(ptr::null(), 4) };
    assert_eq!(
        invalid, None,
        "a non-zero length with a null pointer is not something the host can mean"
    );
    let data = *b"ab";
    // SAFETY: `data` is a live local array and `len` is its exact length, so the slice
    // the reader builds stays inside it for the whole of the borrow.
    let read = unsafe { bytes_from_raw_allow_empty(data.as_ptr(), data.len()) };
    assert_eq!(read, Some(&b"ab"[..]));
}

/// The ABI source, embedded at test-compile time.
///
/// The panic-guard invariant is a fact about which bodies call `guard_ffi`, not about
/// any value the slots return: the availability body reads two atomics and offers no
/// runtime panic to inject, so the structural assertions below read the source instead.
const ABI_SOURCE: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/ffi/abi.rs"));

/// Drops whole-line comments from `source`.
///
/// The structural search below matches on `extern "C" fn` signatures, and this file's
/// comments mention those spellings; stripping the lines first keeps a comment from
/// posing as a function.
fn without_comment_lines(source: &str) -> String {
    let mut stripped = String::with_capacity(source.len());
    for line in source.lines() {
        if line.trim_start().starts_with("//") {
            continue;
        }
        stripped.push_str(line);
        stripped.push('\n');
    }
    stripped
}

/// The text of the `extern "C" fn` called `name`, through to the next such function.
///
/// The boundary is exact enough for this file, whose `extern "C"` items are adjacent and
/// short. The chunk of the last one reaches the end of the source; that tail carries no
/// `guard_ffi` of its own, so it only makes the assertions below stricter.
fn extern_fn_source<'a>(source: &'a str, name: &str) -> Option<&'a str> {
    const MARKER: &str = "extern \"C\" fn ";
    let signature = format!("{MARKER}{name}(");
    let start = source.find(&signature)?;
    let after = start + MARKER.len();
    let end = source[after..]
        .find(MARKER)
        .map_or(source.len(), |offset| after + offset);
    Some(&source[start..end])
}

#[test]
fn test_every_ui_vtable_slot_body_runs_under_the_panic_guard() {
    // The C++ glue calls each slot directly, so a panic escaping a slot body would
    // unwind into C++ — undefined behaviour. Whether a body is wrapped is a property of
    // the source text, so the module's own invariant is pinned structurally: every slot
    // the registered table hands to the glue must spell `guard_ffi` in its body.
    let source = without_comment_lines(ABI_SOURCE);
    for name in [
        "on_addon_init",
        "on_addon_destroy",
        "on_input_panel_update",
        "on_cursor_rect",
        "on_host_suspend",
        "on_host_resume",
        "is_available",
    ] {
        let body = extern_fn_source(&source, name).expect("every slot is defined in abi.rs");
        assert!(
            body.contains("guard_ffi"),
            "vtable slot {name} must run its body inside `guard_ffi`"
        );
    }
}

#[test]
fn test_availability_answers_from_one_guarded_implementation() {
    // The host reaches the availability query through two entry points: the export it
    // resolves by name and the vtable slot the glue calls directly. Both must run the
    // same `crate::ui_impl::is_available` under the guard with the same `false`
    // fallback — a second body, or an unguarded one, would let the two answers drift
    // apart or take the process down.
    let source = without_comment_lines(ABI_SOURCE);
    for name in ["rspinyin_ui_available", "is_available"] {
        let body = extern_fn_source(&source, name).expect("both entries are defined in abi.rs");
        assert!(
            body.contains("guard_ffi(false") && body.contains("crate::ui_impl::is_available"),
            "{name} must answer from `crate::ui_impl::is_available` under the guard"
        );
    }
}
