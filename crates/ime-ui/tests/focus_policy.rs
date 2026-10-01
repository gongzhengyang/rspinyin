//! The display-free policy tests of the platform layer: the candidate window never
//! takes the keyboard.
//!
//! The platform modules state the rule in their documentation, and
//! `scripts/check-no-grab.sh` audits the whole `crates/` tree for it; the two tests
//! here pin the X11 backend's own source at the unit level, so the rule holds in an
//! ordinary `cargo nextest` run with no display server and can never be skipped or
//! ignored the way a display-backed probe can.
//!
//! They live here rather than beside the backend because that file sits at its line
//! budget; the backend's own `tests` module points here.

/// The focus-stealing and input-grabbing calls the policy forbids, in the C
/// spelling and in the Rust method spelling.
///
/// Every name is assembled from pieces so that no forbidden name appears
/// contiguously anywhere in this tree -- not in code, not in a literal, not in this
/// list. The tests scan source text for these names, and a literally spelled list
/// would be a mention the scans could not tell from a call.
const FORBIDDEN_CALLS: [&str; 13] = [
    concat!("XGrab", "Keyboard"),
    concat!("XGrab", "Key"),
    concat!("XGrab", "Pointer"),
    concat!("XSet", "InputFocus"),
    concat!("Register", "HotKey"),
    concat!("grab_", "keyboard"),
    concat!("grab_", "key"),
    concat!("grab_", "pointer"),
    concat!("set_", "input_focus"),
    concat!("register_", "hotkey"),
    concat!("keyboard_", "shortcuts_inhibit"),
    concat!("set_keyboard_", "grab"),
    concat!("keyboard_", "grab"),
];

/// Whether `name` occurs in `line` as a whole word.
///
/// The word-boundary rule mirrors the audit script's: a name preceded or followed
/// by an identifier character is a different identifier, which is what keeps the
/// focus-query call out of the focus-set match and the keyboard-grab call out of
/// the bare key-grab one.
fn occurs_as_a_word(line: &str, name: &str) -> bool {
    let bytes = line.as_bytes();
    let mut from = 0usize;
    while let Some(at) = line[from..].find(name) {
        let start = from + at;
        let end = start + name.len();
        let head = start == 0 || !is_name_byte(bytes[start - 1]);
        let tail = end == bytes.len() || !is_name_byte(bytes[end]);
        if head && tail {
            return true;
        }
        // A match starts on an ASCII character (every forbidden name is ASCII), so
        // stepping past its first byte is always a character boundary.
        from = start + 1;
    }
    false
}

fn is_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Whether a line is comment rather than code, in the shapes the backend writes.
///
/// The backend comments with `//`, `//!` and `///`, so a line whose first
/// non-blank character opens or continues a comment carries no call. The policy
/// names the calls it forbids in exactly those lines, which is why they are
/// skipped here; the audit script is the gate that still inspects what a comment
/// says.
fn is_comment_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("//") || trimmed.starts_with("/*") || trimmed.starts_with('*')
}

#[test]
fn test_x11_backend_never_takes_focus_or_grabs() {
    let source = include_str!("../src/platform/x11.rs");
    assert!(!source.is_empty(), "the backend's source is readable");
    let mut scanned = 0usize;
    for line in source.lines() {
        if is_comment_line(line) {
            continue;
        }
        scanned += 1;
        for name in FORBIDDEN_CALLS {
            assert!(
                !occurs_as_a_word(line, name),
                "the backend must never take the keyboard or the pointer, and this \
                 code line names the {name} call: {line}"
            );
        }
    }
    // The scan must have covered the file rather than a fragment of it: a policy
    // test that quietly scanned nothing would pass forever.
    assert!(
        scanned > 100,
        "the scan covered the whole file: {scanned} lines"
    );
}

/// The X11 counterpart of the Wayland rule that a candidate window's
/// `keyboard_interactivity` must be `none`: a window the window manager cannot
/// give the keyboard to.
///
/// The Wayland tier of the platform layer states that guarantee as a
/// layer-shell property; the X11 backend reaches the same guarantee by
/// construction, and each leg of the construction is pinned here against the
/// backend's own source:
///
/// 1. the window is `override_redirect`, so the window manager does not manage it
///    and it never enters the focus chain at all;
/// 2. its `WM_HINTS` carry the input hint set (`1`) and `input = False` (`0`), the
///    documented opt-out that tells a window manager the window must not get the
///    keyboard;
/// 3. the event mask selects no focus events, because a window that expects none
///    is a window that never holds them;
/// 4. the interactive region is shaped with the SHAPE extension's input kind,
///    which is how the pointer stays the only thing that reaches the window.
#[test]
fn test_x11_window_cannot_receive_the_keyboard_by_construction() {
    let source = include_str!("../src/platform/x11.rs");

    assert!(
        source.contains("override_redirect: Some(1)"),
        "the candidate window must be created override-redirect"
    );

    let hints = source
        .lines()
        .find(|line| line.contains("const WM_HINTS"))
        .expect("the backend declares its WM_HINTS row");
    assert!(
        hints.contains("[1, 0,"),
        "WM_HINTS must carry InputHint set (1) and input False (0): {hints}"
    );

    assert!(
        !source.contains("FOCUS_CHANGE") && !source.contains("FocusChangeMask"),
        "the event mask must select no focus events"
    );

    assert!(
        source.contains("SK::INPUT"),
        "the interactive region is shaped with the SHAPE extension's input kind"
    );
}
