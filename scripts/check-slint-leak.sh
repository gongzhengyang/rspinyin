#!/usr/bin/env bash
#
# check-slint-leak.sh - assert that ime-ui exports no Slint type.
#
# features.md 0.4 rule 11 / ADR-0000 obligation OB-4: the royalty-free licence
# forbids distributing an application that exposes the Slint API for third
# parties to program against. The candidate window must therefore stay a purely
# internal detail of ime-ui: no `slint::` type may appear in a `pub fn`, `pub
# struct`, `pub trait` or `pub` field.
#
# The gate parses the output of `cargo public-api -p ime-ui` and fails on any
# symbol whose path names Slint. Symbols that merely contain the word (a type
# called `SlintThemeConfig`, a field named `slint_free`) are not Slint API and
# are not reported.
#
# `cargo-public-api` must be installed; when it is missing the gate fails with
# the install command rather than passing quietly, because a skipped gate is
# indistinguishable from a satisfied one.
#
# Exit codes: 0 = pass, 1 = violation, 2 = usage or environment error.
#
# Self-test: `--self-test` runs the analyser over synthetic public-API documents
# (one leaking, one clean, one look-alike) and, when the tool is installed, over
# the real `ime-ui` output as an end-to-end smoke test. Injecting a real leak
# into the crate is not possible until `ime-ui` depends on Slint, so the
# synthetic documents exercise the exact detection path instead.
#
# Requires python3 (present by default on every distro in the platform baseline).

set -euo pipefail

usage() {
    cat <<'USAGE'
usage: scripts/check-slint-leak.sh [--self-test] [--input FILE] [--root DIR]

  --self-test        run the analyser over synthetic and (if available) real output
  --input FILE       analyse a captured `cargo public-api -p ime-ui` document
  --root DIR         repository root (default: parent of this script)
USAGE
}

mode="check"
input_file=""
root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"

while [ $# -gt 0 ]; do
    case "$1" in
        --self-test) mode="self-test"; shift ;;
        --input)
            input_file="${2:?--input needs a path}"
            shift 2
            ;;
        --root)
            root="${2:?--root needs a directory}"
            shift 2
            ;;
        -h | --help)
            usage
            exit 0
            ;;
        *)
            echo "check-slint-leak: unknown argument: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

if ! command -v python3 >/dev/null 2>&1; then
    echo "check-slint-leak: python3 is required to parse the public API; install it and re-run" >&2
    exit 2
fi

public_api_available() {
    cargo public-api --version >/dev/null 2>&1
}

require_public_api() {
    if ! public_api_available; then
        echo "check-slint-leak: cargo-public-api is not installed" >&2
        echo "check-slint-leak: install it with: cargo install cargo-public-api --locked" >&2
        echo "check-slint-leak: this gate cannot be skipped; it is a licence obligation (OB-4)" >&2
        exit 2
    fi
}

# capture: write the public API of ime-ui to stdout.
capture() {
    (cd -- "$root" && cargo public-api -p ime-ui --simplified) || {
        echo "check-slint-leak: 'cargo public-api -p ime-ui --simplified' failed under $root" >&2
        echo "check-slint-leak: fix the crate build (or the cargo-public-api version) and re-run" >&2
        exit 2
    }
}

# analyse FILE: report every exported symbol that names Slint.
analyse() {
    python3 - "$1" <<'PY'
import re
import sys
from pathlib import Path

# Slint paths as they appear in rustdoc output, plus the internal crates the
# generated bindings are built from. A match anywhere in a public item means the
# Slint API is reachable from outside the crate.
PATTERNS = (
    re.compile(r"\bslint\s*::"),
    re.compile(r"\bi_slint_"),
    re.compile(r"\bi-slint-"),
    re.compile(r"\bslint_generated"),
)

# cargo-public-api may colour its output; strip escape sequences before matching.
ANSI = re.compile(r"\x1b\[[0-9;]*[A-Za-z]")

path = sys.argv[1]
try:
    document = Path(path).read_text(encoding="utf-8", errors="replace")
except OSError as error:
    sys.exit(f"check-slint-leak: cannot read {path}: {error}")

problems = []
lines = document.splitlines()
for number, raw in enumerate(lines, start=1):
    line = ANSI.sub("", raw)
    for pattern in PATTERNS:
        if pattern.search(line):
            problems.append(f"{path}:{number}: {line.strip()}")
            break

if problems:
    print("check-slint-leak: FAIL - ime-ui exports Slint API", file=sys.stderr)
    for problem in problems:
        print(f"  - {problem}", file=sys.stderr)
    print(
        "check-slint-leak: keep Slint types inside private modules; the candidate "
        "window must not be a programmable Slint surface (0.4 rule 11, OB-4)",
        file=sys.stderr,
    )
    sys.exit(1)

print(f"check-slint-leak: PASS ({len(lines)} public API lines, no Slint symbol)")
PY
}

expect_clean() {
    # expect_clean FILE DESCRIPTION
    local status=0 output=""
    if output="$(analyse "$1" 2>&1)"; then
        status=0
    else
        status=$?
    fi
    if [ "$status" -ne 0 ]; then
        echo "check-slint-leak: self-test FAILED - $2 exited $status, expected 0" >&2
        echo "$output" >&2
        return 1
    fi
    return 0
}

expect_violation() {
    # expect_violation FILE EXPECTED_TEXT DESCRIPTION
    local status=0 output=""
    if output="$(analyse "$1" 2>&1)"; then
        status=0
    else
        status=$?
    fi
    if [ "$status" -eq 0 ]; then
        echo "check-slint-leak: self-test FAILED - $3 was not detected" >&2
        echo "$output" >&2
        return 1
    fi
    case "$output" in
        *"$2"*) ;;
        *)
            echo "check-slint-leak: self-test FAILED - $3 was reported for the wrong reason" >&2
            echo "check-slint-leak: expected the report to mention: $2" >&2
            echo "$output" >&2
            return 1
            ;;
    esac
    return 0
}

run_self_test() {
    local scratch leaky clean lookalike
    scratch="$(mktemp -d "${TMPDIR:-/tmp}/rspinyin-slint.XXXXXX")"
    # shellcheck disable=SC2064  # expand the path now, not at trap time
    trap "rm -rf -- '$scratch'" EXIT
    leaky="$scratch/leaky.txt"
    clean="$scratch/clean.txt"
    lookalike="$scratch/lookalike.txt"

    cat >"$clean" <<'EOF'
pub struct UiFrame {
    pub revision: u32,
}
impl UiFrame {
    pub fn revision(&self) -> u32
}
pub fn build_frame() -> UiFrame
EOF

    cat >"$leaky" <<'EOF'
pub struct CandidateWindow {
    pub handle: slint::Weak<slint::Window>,
}
pub fn window() -> slint :: Window
EOF

    cat >"$lookalike" <<'EOF'
pub struct SlintThemeConfig {
    pub slint_free: bool,
}
pub fn slint_window_stub() -> SlintThemeConfig
EOF

    echo "check-slint-leak: self-test (synthetic public-API documents)"
    expect_clean "$clean" "a clean public API"
    expect_violation "$leaky" "slint::" "an exported slint:: type"
    expect_clean "$lookalike" "identifiers that merely contain the word"

    if public_api_available; then
        local real="$scratch/ime-ui.txt"
        capture >"$real"
        expect_clean "$real" "the real ime-ui public API"
        echo "check-slint-leak: self-test PASS (detection verified, real ime-ui API clean)"
    else
        echo "check-slint-leak: self-test PASS (detection verified; cargo-public-api absent,"
        echo "check-slint-leak: real ime-ui API not smoke-tested)"
    fi
}

if [ "$mode" = "self-test" ]; then
    run_self_test
    exit 0
fi

if [ -n "$input_file" ]; then
    # A captured document needs no toolchain, which is what the self-test and
    # offline review use.
    analyse "$input_file"
else
    require_public_api
    captured="$(mktemp "${TMPDIR:-/tmp}/rspinyin-public-api.XXXXXX")"
    trap 'rm -f -- "$captured"' EXIT
    capture >"$captured"
    analyse "$captured"
fi
