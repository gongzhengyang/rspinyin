#!/usr/bin/env bash
#
# check-unsafe.sh - keep `unsafe` and `extern "C"` inside the audited files.
#
# features.md 0.4 rule 3 allows unsafe code in exactly three places:
#
#   crates/ime-fcitx5/src/ffi/**     (the engine addon's C ABI glue)
#   crates/ime-ui-addon/src/ffi/**   (the user-interface addon's C ABI glue)
#   crates/ime-dict/src/mmap.rs      (the only place that maps a file)
#
# The allowlist is file-precise: any `unsafe` or `extern` keyword anywhere else
# fails, because `[workspace.lints.rust] unsafe_code = "deny"` is the compiler's
# backstop and this script is the architectural one.
#
# There are two FFI directories because there are two cdylibs. ADR-0003 split the
# plugin into an input-method addon and a user-interface addon, each with its own
# C ABI and its own glue; both are `dlopen`'d by Fcitx5 and neither links the
# other, so each needs the same narrow allowance.
#
# Two further rules from AGENTS.md section 3.3 and 8.2 are enforced here:
#
#   * every `unsafe { ... }` block inside the allowed files must be preceded by a
#     `// SAFETY:` comment justifying the invariants it relies on;
#   * a crate-level `#![allow(unsafe_code)]` is rejected: it would disable the
#     workspace lint for the whole crate and make the file-precise allowlist
#     unenforceable. Allow the module that needs it instead, e.g.
#     `#[allow(unsafe_code)] mod ffi;`.
#
# Comments and string literals are stripped by a small Rust lexer before the
# search, so the word "unsafe" in a doc comment or in a `"..."` literal is not
# reported, and lifetimes (`'a`) are not mistaken for char literals. `syn`-based
# AST confirmation is not available: xtask has no parser dependency and this
# script must not add one.
#
# Exit codes: 0 = pass, 1 = violation, 2 = usage or environment error.
#
# Self-test: `--self-test` builds a scratch crate tree, injects each violation
# class in turn, asserts a non-zero exit with the matching message, removes it
# and asserts a zero exit. The project tree is never touched.
#
# Requires python3 (present by default on every distro in the platform baseline).

set -euo pipefail

usage() {
    cat <<'USAGE'
usage: scripts/check-unsafe.sh [--self-test] [--root DIR]

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
            echo "check-unsafe: unknown argument: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

if ! command -v python3 >/dev/null 2>&1; then
    echo "check-unsafe: python3 is required to strip comments and literals; install it and re-run" >&2
    exit 2
fi

# scan ROOT: report every unsafe/extern keyword outside the allowlist, every
# unsafe block without a SAFETY comment, and every crate-level unsafe_code allow.
scan() {
    python3 - "$1" <<'PY'
import os
import re
import sys

root = os.path.abspath(sys.argv[1])

# features.md 0.4 rule 3, file-precise.
ALLOWED_FILES = ("crates/ime-dict/src/mmap.rs",)
ALLOWED_DIRS = (
    "crates/ime-fcitx5/src/ffi/",
    "crates/ime-ui-addon/src/ffi/",
    # The test-only allocation counter. A `#[global_allocator]` needs
    # `unsafe impl GlobalAlloc`, and nothing else in the workspace can install one, so the
    # decoder's allocation budget would be unassertable without this path. The crate is a
    # `dev-dependency` only -- see the `--no-release-dependency` assertion below.
    "crates/alloc-count/src/",
)

SKIP_DIRS = frozenset(
    {".git", ".cargo", "target", "node_modules", "vendor", "dist", "build"}
)

KEYWORDS = re.compile(r"\b(unsafe|extern)\b")
CRATE_LEVEL_ALLOW = re.compile(r"#!\[[^\]]*allow\s*\([^)]*unsafe_code")
# Raw string prefixes: r"...", r#"..."#, br#"..."#, cr#"..."#. Plain and
# prefixed strings are handled by the quote branch inside the scanner.
RAW_STRING = re.compile(r'(?:b|c)?r(#*)"')
SAFETY_LOOKBACK = 5


def strip_comments_and_literals(source):
    """Blank out comments and literals, preserving offsets and line breaks."""
    out = list(source)
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
            blank(index, end)
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
            blank(index, cursor)
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

        # A plain string literal, which also covers `b"..."` and `c"..."`: the
        # prefix letter is an ordinary identifier character and stays behind.
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
            # A char literal is 'x', '\n' or '\u{...}'; anything else starting
            # with a quote is a lifetime ('a, 'static, '_) and must survive.
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

    return "".join(out)


def is_allowed(relative):
    return relative in ALLOWED_FILES or relative.startswith(ALLOWED_DIRS)


def safety_documented(lines, line_number, column):
    """True when a `// SAFETY:` comment justifies this position."""
    start = max(1, line_number - SAFETY_LOOKBACK)
    for number in range(start, line_number):
        if "SAFETY:" in lines[number - 1]:
            return True
    return "SAFETY:" in lines[line_number - 1][:column]


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
        scanned += 1
        with open(path, "r", encoding="utf-8", errors="replace") as handle:
            source = handle.read()
        cleaned = strip_comments_and_literals(source)
        lines = source.splitlines()

        for match in CRATE_LEVEL_ALLOW.finditer(cleaned):
            line_number = cleaned.count("\n", 0, match.start()) + 1
            problems.append(
                f"{relative}:{line_number}: crate-level `#![allow(unsafe_code)]` disables the "
                f"workspace lint for the whole crate; allow the module that needs it instead "
                f"(e.g. `#[allow(unsafe_code)] mod ffi;`)"
            )

        allowed = is_allowed(relative)
        for match in KEYWORDS.finditer(cleaned):
            keyword = match.group(1)
            line_number = cleaned.count("\n", 0, match.start()) + 1
            column = match.start() - (cleaned.rfind("\n", 0, match.start()) + 1)
            if not allowed:
                problems.append(
                    f"{relative}:{line_number}: `{keyword}` outside the audited paths "
                    f"({', '.join(ALLOWED_DIRS + ALLOWED_FILES)}); 0.4 rule 3 forbids it"
                )
                continue
            if keyword != "unsafe":
                continue
            tail = cleaned[match.end():]
            if not tail.lstrip().startswith("{"):
                continue
            if not safety_documented(lines, line_number, column):
                problems.append(
                    f"{relative}:{line_number}: `unsafe` block without a `// SAFETY:` comment "
                    f"on the preceding lines (AGENTS.md 3.3)"
                )

# The fourth allowed path is a test-only counter, so the allowance is only safe as long as
# nothing that ships depends on it. A `[dependencies]` entry anywhere in the workspace, or
# any mention at all in the two cdylib crates, would put `unsafe` back on the release path
# through a file this scan already blessed.
COUNTER_CRATE = "alloc-count"
RELEASE_CRATES = ("crates/ime-fcitx5", "crates/ime-ui-addon")

for crate in sorted(os.listdir(os.path.join(root, "crates"))):
    manifest = os.path.join(root, "crates", crate, "Cargo.toml")
    if not os.path.isfile(manifest):
        continue
    if crate == COUNTER_CRATE:
        # Its own manifest names it under `[package]`, which is not a dependency edge.
        continue
    with open(manifest, "r", encoding="utf-8", errors="replace") as handle:
        text = handle.read()
    relative = os.path.relpath(manifest, root).replace(os.sep, "/")
    if COUNTER_CRATE not in text:
        continue
    if f"crates/{crate}" in RELEASE_CRATES:
        problems.append(
            f"{relative}: the {COUNTER_CRATE} counter is a test-only crate and must never "
            f"be a dependency of a cdylib that ships; it carries the only `unsafe` outside "
            f"the FFI and mmap paths"
        )
        continue
    section = None
    for number, line in enumerate(text.splitlines(), start=1):
        stripped = line.strip()
        if stripped.startswith("[") and stripped.endswith("]"):
            section = stripped
            continue
        if section != "[dev-dependencies]" and COUNTER_CRATE in stripped:
            problems.append(
                f"{relative}:{number}: `{COUNTER_CRATE}` is listed under {section or 'no section'}; "
                f"it may only appear under [dev-dependencies], or `unsafe` re-enters the "
                f"release dependency graph"
            )

if problems:
    print("check-unsafe: FAIL", file=sys.stderr)
    for problem in problems:
        print(f"  - {problem}", file=sys.stderr)
    sys.exit(1)

print(f"check-unsafe: PASS ({scanned} Rust files scanned, unsafe stays in the audited paths)")
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
        echo "check-unsafe: self-test FAILED - $2 exited $status, expected 0" >&2
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
        echo "check-unsafe: self-test FAILED - $3 was not detected" >&2
        echo "$output" >&2
        return 1
    fi
    case "$output" in
        *"$2"*) ;;
        *)
            echo "check-unsafe: self-test FAILED - $3 was reported for the wrong reason" >&2
            echo "check-unsafe: expected the report to mention: $2" >&2
            echo "$output" >&2
            return 1
            ;;
    esac
    return 0
}

run_self_test() {
    local scratch
    scratch="$(mktemp -d "${TMPDIR:-/tmp}/rspinyin-unsafe.XXXXXX")"
    # shellcheck disable=SC2064  # expand the path now, not at trap time
    trap "rm -rf -- '$scratch'" EXIT

    mkdir -p "$scratch/crates/ime-core/src"
    mkdir -p "$scratch/crates/ime-dict/src"
    mkdir -p "$scratch/crates/ime-fcitx5/src/ffi"
    mkdir -p "$scratch/crates/ime-ui-addon/src/ffi"
    mkdir -p "$scratch/crates/alloc-count/src"
    mkdir -p "$scratch/crates/ime-ui/src"

    # A file whose only mentions of the keyword are in comments and literals.
    cat >"$scratch/crates/ime-core/src/lib.rs" <<'EOF'
//! The decoder never uses unsafe code.
// unsafe { } is forbidden here.
/* block comment mentioning unsafe { } */
pub const NOTE: &str = "unsafe is not allowed in this crate";
pub fn lifetime<'a>(value: &'a str) -> &'a str { value }
EOF

    cat >"$scratch/crates/ime-dict/src/mmap.rs" <<'EOF'
//! Dictionary mapping.
pub fn map() {
    // SAFETY: the mapping outlives the slice and is never unmapped.
    unsafe { std::ptr::null::<u8>() };
}
EOF

    cat >"$scratch/crates/ime-fcitx5/src/ffi/glue.rs" <<'EOF'
//! C ABI glue.
pub fn exported() {
    // SAFETY: the caller guarantees a valid pointer.
    unsafe { std::ptr::null::<u8>() };
}
extern "C" { pub fn fcitx_host_entry(); }
EOF

    # The second addon host. Its FFI directory carries the same allowance, so a clean
    # tree here is what proves the allowlist covers both cdylibs rather than only the
    # one it was written for.
    cat >"$scratch/crates/ime-ui-addon/src/ffi/glue.rs" <<'EOF'
//! User-interface C ABI glue.
pub fn exported() {
    // SAFETY: the caller guarantees a valid pointer.
    unsafe { std::ptr::null::<u8>() };
}
extern "C" { pub fn fcitx_ui_host_entry(); }
EOF

    # The test-only allocation counter. Its whole reason for existing is a
    # `unsafe impl GlobalAlloc`, so a clean tree here is what proves the fourth allowed
    # path is actually allowed rather than merely written down.
    cat >"$scratch/crates/alloc-count/src/lib.rs" <<'EOF'
//! Counting allocator for tests.
use std::alloc::{GlobalAlloc, Layout};
use std::sync::atomic::{AtomicUsize, Ordering};

pub static ALLOCATED: AtomicUsize = AtomicUsize::new(0);

/// A pass-through allocator that counts what it is asked for.
pub struct Counting;

// SAFETY: every method forwards to `System`, which upholds the `GlobalAlloc` contract;
// this type only adds a counter and never changes a pointer or a layout.
#[allow(unsafe_code)]
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATED.fetch_add(layout.size(), Ordering::Relaxed);
        // SAFETY: the caller upholds `GlobalAlloc::alloc`'s contract, which is what
        // `System::alloc` requires.
        unsafe { std::alloc::System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` came from this allocator's `alloc` with this same `layout`.
        unsafe { std::alloc::System.dealloc(ptr, layout) }
    }
}
EOF

    # Outside an allowed directory in the same crate, so the check is proven to be
    # path-scoped and not merely crate-scoped.
    cat >"$scratch/crates/ime-ui-addon/src/lib.rs" <<'EOF'
//! User-interface addon, no unsafe outside ffi.
pub fn start() {}
EOF

    cat >"$scratch/crates/ime-ui/src/lib.rs" <<'EOF'
//! View layer, no unsafe anywhere.
pub fn draw() {}
EOF

    echo "check-unsafe: self-test (scratch tree: $scratch)"
    expect_clean "$scratch" "the clean scratch tree"

    cat >>"$scratch/crates/ime-core/src/lib.rs" <<'EOF'
pub fn leaky() {
    unsafe { std::ptr::null::<u8>() };
}
EOF
    expect_violation "$scratch" "crates/ime-core/src/lib.rs" "an unsafe block outside the audited paths"

    cat >"$scratch/crates/ime-core/src/lib.rs" <<'EOF'
//! The decoder never uses unsafe code.
pub fn lifetime<'a>(value: &'a str) -> &'a str { value }
EOF
    expect_clean "$scratch" "the tree after removing the unsafe block"

    cat >"$scratch/crates/ime-dict/src/mmap.rs" <<'EOF'
//! Dictionary mapping.
pub fn map() {
    unsafe { std::ptr::null::<u8>() };
}
EOF
    expect_violation "$scratch" "without a \`// SAFETY:\` comment" \
        "an unsafe block in an audited path with no SAFETY comment"

    cat >"$scratch/crates/ime-dict/src/mmap.rs" <<'EOF'
//! Dictionary mapping.
pub fn map() {
    // SAFETY: the mapping outlives the slice and is never unmapped.
    unsafe { std::ptr::null::<u8>() };
}
EOF
    expect_clean "$scratch" "the audited unsafe block with its SAFETY comment"

    cat >"$scratch/crates/ime-fcitx5/src/lib.rs" <<'EOF'
#![allow(unsafe_code)]
//! Host integration layer.
EOF
    expect_violation "$scratch" "crate-level \`#![allow(unsafe_code)]\`" \
        "a crate-level unsafe_code allow"

    cat >"$scratch/crates/ime-fcitx5/src/lib.rs" <<'EOF'
//! Host integration layer.
#[allow(unsafe_code)]
mod ffi;
EOF
    expect_clean "$scratch" "a module-scoped unsafe_code allow"

    echo "check-unsafe: self-test PASS (3 violation classes detected, clean tree accepted)"
}

if [ "$mode" = "self-test" ]; then
    run_self_test
    exit 0
fi

scan "$root"
