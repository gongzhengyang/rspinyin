#!/usr/bin/env bash
#
# check-no-grab.sh - keep focus-stealing and input-grabbing calls out of the crates.
#
# The candidate window must never take the keyboard: losing the application's focus
# is the project's highest-severity defect (features.md 0.4 rule 5, AGENTS.md
# prohibited item 20). The rule is wider than the one call: a window that grabs the
# keyboard or the pointer, registers a global hotkey or inhibits the platform's
# shortcut handling steals input exactly as a focus change does, so every call in
# that family is banned outright:
#
#   XGrabKey, XGrabKeyboard, XGrabPointer, XSetInputFocus, RegisterHotKey
#   grab_key, grab_keyboard, grab_pointer, set_input_focus, register_hotkey
#   keyboard_shortcuts_inhibit, set_keyboard_grab, keyboard_grab
#
# The C spellings cover the X11 and Win32 APIs; the snake_case spellings cover the
# Rust wrappers (x11rb's method names, the Wayland `keyboard_shortcuts_inhibit`
# protocol, GTK-style `set_keyboard_grab` accessors). Both are matched with word
# boundaries, so `get_input_focus` is untouched and `XGrabKey` does not swallow
# `XGrabKeyboard`.
#
# Comments are the one place the names may appear, and only to forbid them: the
# platform modules document the rule by naming the calls they never make. A comment
# block that states the policy -- it carries one of the cues `never`, `no grab` or
# `must not` -- is exempt as a whole, contiguous block, because a policy sentence on
# one line and the names it forbids on the next are one text. A comment that names a
# forbidden call without stating the policy fails: a mention that neither forbids nor
# explains is how a real call gets excused. String and char literals are blanked
# before the search, on the check-unsafe precedent: a name inside a literal is not a
# call, and the unit-level scan in `crates/ime-ui/tests/focus_policy.rs` pins the
# backend's source independently of this script.
#
# The scan covers `crates/**.rs` only. `xtask` is a build tool that runs on the
# developer's machine rather than inside the user's session; the policy guards the
# plugin and the crates that feed it. A tree with no `crates/` directory is an
# environment error, not a pass, so a wrong `--root` cannot look green.
#
# Exit codes: 0 = pass, 1 = violation, 2 = usage or environment error.
#
# Self-test: `--self-test` builds a scratch crate tree, injects each violation class
# in turn, asserts a non-zero exit with the matching name, and asserts that policy
# comments, a file outside `crates/` and the clean tree are all accepted. The project
# tree is never touched.
#
# Requires python3 (present by default on every distro in the platform baseline).

set -euo pipefail

usage() {
    cat <<'USAGE'
usage: scripts/check-no-grab.sh [--self-test] [--root DIR]

  --self-test   inject violations into a scratch tree and assert they are detected
  --root DIR    tree to scan (default: parent of this script)
USAGE
}

mode="check"
root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"

while [ $# -gt 0 ]; do
    case "$1" in
        --self-test) mode="self-test"; shift ;;
        --root)
            root="${2:?--root needs a directory}"
            shift 2
            ;;
        -h | --help)
            usage
            exit 0
            ;;
        *)
            echo "check-no-grab: unknown argument: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

if ! command -v python3 >/dev/null 2>&1; then
    echo "check-no-grab: python3 is required to strip literals and track comments; install it and re-run" >&2
    exit 2
fi

# scan ROOT: report every forbidden call in code, and every mention in a comment
# block that does not state the policy.
scan() {
    python3 - "$1" <<'PY'
import os
import re
import sys

root = os.path.abspath(sys.argv[1])

# 0.4 rule 5: the candidate window never takes the keyboard, never grabs the
# pointer, and never registers a global hotkey. Word boundaries on every name, and
# the longer C names ahead of their prefixes so `XGrabKeyboard` is reported once,
# as itself.
FORBIDDEN = re.compile(
    r"\b(XGrabKeyboard|XGrabKey|XGrabPointer|XSetInputFocus|RegisterHotKey"
    r"|grab_keyboard|grab_pointer|grab_key|set_input_focus|register_hotkey"
    r"|keyboard_shortcuts_inhibit|set_keyboard_grab|keyboard_grab)\b"
)

# The cues that make a comment block a statement of the policy rather than a
# mention. Case-insensitive, because the platform docs write both "never" and "No".
POLICY_CUE = re.compile(r"never|no grab|must not", re.IGNORECASE)

SCAN_ROOT = "crates/"
SKIP_DIRS = frozenset(
    {".git", ".cargo", "target", "node_modules", "vendor", "dist", "build"}
)
# Raw string prefixes: r"...", r#"..."#, br#"..."#, cr#"..."#.
RAW_STRING = re.compile(r'(?:b|c)?r(#*)"')


def split_source(source):
    """Walks one file the way Rust lexes it.

    Returns `(cleaned, spans)`: `cleaned` is the source with string and char
    literals blanked -- comments, offsets and line breaks preserved -- and `spans`
    lists every comment as a `(start, end)` byte range, so a match in `cleaned`
    can be told to be in code or in a comment. Lifetimes (`'a`) are left alone, on
    the check-unsafe precedent.
    """
    out = list(source)
    spans = []
    size = len(source)
    index = 0

    def blank(start, end):
        for position in range(start, min(end, size)):
            if out[position] != "\n":
                out[position] = " "

    while index < size:
        char = source[index]
        pair = source[index:index + 2]

        if pair == "//":
            end = source.find("\n", index)
            end = size if end < 0 else end
            spans.append((index, end))
            index = end
            continue

        if pair == "/*":
            depth = 1
            cursor = index + 2
            while cursor < size and depth:
                if source[cursor:cursor + 2] == "/*":
                    depth += 1
                    cursor += 2
                elif source[cursor:cursor + 2] == "*/":
                    depth -= 1
                    cursor += 2
                else:
                    cursor += 1
            spans.append((index, cursor))
            index = cursor
            continue

        raw = RAW_STRING.match(source, index)
        if raw:
            hashes = raw.group(1)
            terminator = '"' + hashes
            cursor = raw.end()
            end = source.find(terminator, cursor)
            end = size if end < 0 else end + len(terminator)
            blank(index, end)
            index = end
            continue

        if char == '"':
            cursor = index + 1
            while cursor < size:
                if source[cursor] == "\\":
                    cursor += 2
                    continue
                if source[cursor] == '"':
                    cursor += 1
                    break
                cursor += 1
            blank(index, cursor)
            index = cursor
            continue

        if char == "'":
            # A char literal is 'x', '\n' or '\u{...}'; anything else starting with
            # a quote is a lifetime ('a, 'static, '_) and must survive.
            escaped = source[index + 1:index + 2] == "\\"
            closing = source[index + 2:index + 3] == "'"
            if escaped or closing:
                cursor = index + 1
                while cursor < size:
                    if source[cursor] == "\\":
                        cursor += 2
                        continue
                    if source[cursor] == "'":
                        cursor += 1
                        break
                    cursor += 1
                blank(index, cursor)
                index = cursor
                continue

        index += 1

    return "".join(out), spans


def line_starts_of(cleaned):
    """The byte offset of every line's first character, plus the sentinel end."""
    starts = [0]
    for position, char in enumerate(cleaned):
        if char == "\n":
            starts.append(position + 1)
    return starts


def line_of(starts, offset):
    """The one-based line number `offset` sits on."""
    low, high = 0, len(starts) - 1
    while low < high:
        middle = (low + high + 1) // 2
        if starts[middle] <= offset:
            low = middle
        else:
            high = middle - 1
    return low + 1


def comment_lines_of(spans, starts):
    """The set of one-based line numbers any comment touches."""
    touched = set()
    for start, end in spans:
        first = line_of(starts, start)
        last = line_of(starts, max(start, end - 1))
        touched.update(range(first, last + 1))
    return touched


def states_policy(lines, number, comment_lines):
    """True when the contiguous comment run around `number` states the policy.

    The run is expanded over adjacent comment-covered lines before the cues are
    asked, so a sentence that forbids the calls and the line that names one of them
    are read as one text, the way the platform modules document the rule.
    """
    low = high = number
    while low > 1 and (low - 1) in comment_lines:
        low -= 1
    while high < len(lines) and (high + 1) in comment_lines:
        high += 1
    return any(POLICY_CUE.search(lines[position - 1]) for position in range(low, high + 1))


if not os.path.isdir(os.path.join(root, "crates")):
    print(f"check-no-grab: no crates/ directory under {root}; wrong --root?", file=sys.stderr)
    sys.exit(2)

problems = []
scanned = 0

for directory, subdirectories, filenames in os.walk(root):
    subdirectories[:] = sorted(
        name
        for name in subdirectories
        if name not in SKIP_DIRS and not os.path.islink(os.path.join(directory, name))
    )
    for filename in sorted(filenames):
        if not filename.endswith(".rs"):
            continue
        path = os.path.join(directory, filename)
        relative = os.path.relpath(path, root).replace(os.sep, "/")
        if not relative.startswith(SCAN_ROOT):
            continue
        scanned += 1
        with open(path, "r", encoding="utf-8", errors="replace") as handle:
            source = handle.read()
        cleaned, spans = split_source(source)
        starts = line_starts_of(cleaned)
        lines = source.splitlines()
        comment_lines = comment_lines_of(spans, starts)

        for match in FORBIDDEN.finditer(cleaned):
            name = match.group(0)
            number = line_of(starts, match.start())
            in_comment = any(start <= match.start() < end for start, end in spans)
            if in_comment:
                if states_policy(lines, number, comment_lines):
                    continue
                problems.append(
                    f"{relative}:{number}: `{name}` named in a comment that does not state "
                    f"the policy; a comment may name a forbidden call only to forbid it "
                    f"(say never, no grab or must not)"
                )
            else:
                problems.append(
                    f"{relative}:{number}: `{name}` in code; the candidate window must "
                    f"never take the keyboard or the pointer (0.4 rule 5)"
                )

if problems:
    print("check-no-grab: FAIL", file=sys.stderr)
    for problem in problems:
        print(f"  - {problem}", file=sys.stderr)
    print(
        "check-no-grab: a window that takes focus or grabs input is the project's "
        "highest-severity defect; remove the call",
        file=sys.stderr,
    )
    sys.exit(1)

print(
    f"check-no-grab: PASS ({scanned} Rust files scanned under crates/, "
    "no focus-stealing or grabbing call in code)"
)
PY
}

expect_clean() {
    # expect_clean ROOT DESCRIPTION
    local status=0 output=""
    if output="$(scan "$1" 2>&1)"; then
        status=0
    else
        status=$?
    fi
    if [ "$status" -ne 0 ]; then
        echo "check-no-grab: self-test FAILED - $2 exited $status, expected 0" >&2
        echo "$output" >&2
        return 1
    fi
    return 0
}

expect_violation() {
    # expect_violation ROOT EXPECTED_TEXT DESCRIPTION
    local status=0 output=""
    if output="$(scan "$1" 2>&1)"; then
        status=0
    else
        status=$?
    fi
    if [ "$status" -eq 0 ]; then
        echo "check-no-grab: self-test FAILED - $3 was not detected" >&2
        echo "$output" >&2
        return 1
    fi
    case "$output" in
        *"$2"*) ;;
        *)
            echo "check-no-grab: self-test FAILED - $3 was reported for the wrong reason" >&2
            echo "check-no-grab: expected the report to mention: $2" >&2
            echo "$output" >&2
            return 1
            ;;
    esac
    return 0
}

run_self_test() {
    local scratch
    scratch="$(mktemp -d "${TMPDIR:-/tmp}/rspinyin-grab.XXXXXX")"
    # shellcheck disable=SC2064  # expand the path now, not at trap time
    trap "rm -rf -- '$scratch'" EXIT

    mkdir -p "$scratch/crates/ime-ui/src/platform"
    mkdir -p "$scratch/crates/ime-core/src"
    mkdir -p "$scratch/xtask/src"

    # The policy is documented by naming the calls it forbids; a clean tree accepts
    # that, and it accepts a hotkey helper outside crates/ too, which is what proves
    # the scan is scoped to the crates rather than to the tree.
    cat >"$scratch/crates/ime-ui/src/platform/x11.rs" <<'EOF'
//! The X11 backend.
//!
//! # Never takes keyboard focus
//!
//! The window is override-redirect and this module never calls
//! `set_input_focus`; neither `grab_keyboard` nor `grab_pointer` is ever sent.
pub fn backend_id() -> &'static str {
    "x11"
}
EOF

    cat >"$scratch/crates/ime-core/src/lib.rs" <<'EOF'
//! The decoder.
pub fn decode() {}
EOF

    cat >"$scratch/xtask/src/main.rs" <<'EOF'
fn main() {
    let _ = register_hotkey();
}

fn register_hotkey() -> bool {
    false
}
EOF

    echo "check-no-grab: self-test (scratch tree: $scratch)"
    expect_clean "$scratch" "the clean scratch tree, policy comments and xtask included"

    cat >>"$scratch/crates/ime-core/src/lib.rs" <<'EOF'

pub fn steal() {
    let cookie = conn.grab_keyboard(true);
}
EOF
    expect_violation "$scratch" "grab_keyboard" "a grab_keyboard call in code"

    cat >"$scratch/crates/ime-core/src/lib.rs" <<'EOF'
//! The decoder.
pub fn decode() {}

pub fn focus() {
    XSetInputFocus(dpy, window, 0, 0);
}
EOF
    expect_violation "$scratch" "XSetInputFocus" "an injected XSetInputFocus call"

    cat >"$scratch/crates/ime-core/src/lib.rs" <<'EOF'
//! The decoder.
pub fn decode() {}

pub fn inhibit() {
    manager.keyboard_shortcuts_inhibit();
}
EOF
    expect_violation "$scratch" "keyboard_shortcuts_inhibit" "a shortcut-inhibition call"

    cat >"$scratch/crates/ime-core/src/lib.rs" <<'EOF'
//! The decoder.
pub fn decode() {}

pub fn grab() {
    window.set_keyboard_grab();
}
EOF
    expect_violation "$scratch" "set_keyboard_grab" "a toolkit keyboard-grab accessor"

    cat >"$scratch/crates/ime-core/src/lib.rs" <<'EOF'
//! The decoder.
pub fn decode() {}

pub fn hotkey() {
    RegisterHotKey(hwnd, 1, 0, 0);
}
EOF
    expect_violation "$scratch" "RegisterHotKey" "a global hotkey registration"

    # A comment that names a call without forbidding it is how a real call gets
    # excused, so it fails even though no code matches.
    cat >"$scratch/crates/ime-core/src/lib.rs" <<'EOF'
//! The decoder.
pub fn decode() {}

// grab_pointer is worth another look.
pub fn note() {}
EOF
    expect_violation "$scratch" "does not state the policy" \
        "a comment mention that states no policy"

    # The same mention inside a sentence that forbids the call is the policy the
    # platform docs are written in, and must stay accepted.
    cat >"$scratch/crates/ime-core/src/lib.rs" <<'EOF'
//! The decoder.
pub fn decode() {}

// We never call grab_pointer: the pointer belongs to the application.
pub fn note() {}
EOF
    expect_clean "$scratch" "a policy comment naming the call it forbids"

    cat >"$scratch/crates/ime-core/src/lib.rs" <<'EOF'
//! The decoder.
pub fn decode() {}
EOF
    expect_clean "$scratch" "the tree after removing the violation"

    echo "check-no-grab: self-test PASS (6 violation classes detected; policy comments, the outside-crates file and the clean tree accepted)"
}

if [ "$mode" = "self-test" ]; then
    run_self_test
    exit 0
fi

scan "$root"
