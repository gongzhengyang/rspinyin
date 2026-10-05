#!/usr/bin/env bash
#
# check-metrics-readers.sh - assert that every constant the layout parses out of the
# metrics block is read by something outside the parser.
#
# `src/layout/metrics.rs` reads the `CandidateMetrics` global of `ui/candidate.slint`
# back out of the source, so the layout arithmetic and the drawing cannot disagree.
# That parse is the one place a constant can hide: a metric the block declares and the
# parser lifts into `Metrics` but nothing ever reads is a number carried in three files
# and used in none -- the same dead weight a dead token is, but harder to see because
# the parse itself looks like a use. This gate closes that hole from the other side of
# `check-ui-spec.sh`, which already asserts that nothing the parser reads is undeclared.
#
# What counts as a reader:
#
#   * a reference in the code (never the comments) of a `.slint` source, excluding the
#     declaration line that introduces the name -- the same rule `check-ui-spec.sh`
#     applies, because a metric that only the drawing code reads is exactly as live as
#     one a Rust module reads;
#   * a mention in production Rust code: any crate's `src` tree, outside test-only
#     code. Test-only means a path under a `tests` directory, a file named `tests.rs`
#     or `*_tests.rs`, or an inline `#[cfg(test)]`-gated module, which is stripped by
#     brace matching. A `#[cfg(test)] mod x;` that pulls a separately named file is not
#     traced to that file, so a reader hidden there would be counted -- the narrow
#     direction for a gate that fails on zero readers.
#
# The comparison is textual, so a mention that is not a use (a same-named field of an
# unrelated type, a doc-free re-export) satisfies it. That is the granularity every
# grep gate in this directory works at; the gate's job is to make a zero-reader parse
# impossible to merge, not to prove the reader meaningful.
#
# Exit codes: 0 = pass, 1 = violation, 2 = usage or environment error.
#
# Self-test: `--self-test` copies the analysed tree into a scratch directory and
# injects a metric no reader knows, first bare -- which must be reported -- then with
# a reader only a `#[cfg(test)]` module sees, which must still be reported, and
# finally with a production reader, which must pass.
#
# The comparison lives in the python3 program embedded below rather than in a second
# file, the way the other audit scripts in this directory do it: a gate that is one
# file cannot be half-installed.
#
# Requires python3 (present by default on every distro in the platform baseline).

set -euo pipefail

usage() {
    cat <<'USAGE'
usage: scripts/check-metrics-readers.sh [--self-test] [--root DIR]

  --self-test   inject a reader-less metric into a scratch copy of the analysed
                tree and assert it is reported, then let a production reader in
                and assert the copy passes
  --root DIR    repository root (default: parent of this script)
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
            echo "check-metrics-readers: unknown argument: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

if ! command -v python3 >/dev/null 2>&1; then
    echo "check-metrics-readers: python3 is required to read the parse and the sources; install it and re-run" >&2
    exit 2
fi

# analyse ROOT: assert every parsed metric has a reader outside the parser.
analyse() {
    python3 - "$1" <<'PY'
import re
import sys
from pathlib import Path

ROOT = Path(sys.argv[1])

METRICS_RS = "crates/ime-ui/src/layout/metrics.rs"
UI_DIR = ROOT / "crates" / "ime-ui" / "ui"
CRATES_SRC = ROOT / "crates"

# The parse sites: the same shape `check-ui-spec.sh` reads, so the two gates can
# never disagree about which constants the layout lifts out of the block.
TAKE = re.compile(r'take(?:_count)?\(\s*found\s*,\s*"([A-Za-z0-9-]+)"\s*\)')

# A `.slint` property declaration. A line that declares `name` does not count as a
# read of it; the same minus-one rule `check-ui-spec.sh` applies.
DECLARATION = re.compile(
    r"^\s*(?:in-out|in|out)\s+property\s+<[^>]+>\s+([A-Za-z0-9_-]+)\s*:"
)

problems = []


def die(message):
    print(f"check-metrics-readers: {message}", file=sys.stderr)
    sys.exit(2)


def read_path(path):
    try:
        return path.read_text(encoding="utf-8")
    except OSError as error:
        die(f"cannot read {path}: {error}")


def code_of(line):
    """The line without its comment tail."""
    return line.split("//")[0]


def boundary(name):
    """A name, not the inside of a longer identifier."""
    return re.compile(r"(?<![\w-])" + re.escape(name) + r"(?![\w-])")


def slint_reads(name):
    """How often the `.slint` sources read `name` outside their comments."""
    pattern = boundary(name)
    count = 0
    for path in sorted(UI_DIR.rglob("*.slint")):
        for raw in read_path(path).splitlines():
            code = code_of(raw)
            if not code.strip():
                continue
            hits = len(pattern.findall(code))
            if hits == 0:
                continue
            declared = DECLARATION.match(code)
            if declared is not None and declared.group(1) == name:
                hits -= 1
            count += hits
    return count


def is_test_path(path):
    """A file only the test builds compile."""
    if "tests" in path.parts:
        return True
    return path.name == "tests.rs" or path.name.endswith("_tests.rs")


def production_lines(text):
    """The code lines of one file, with inline `#[cfg(test)]` modules removed.

    A `#[cfg(test)]` attribute either opens a module block -- skipped to its closing
    brace, comments and all -- or annotates a single item such as a `mod x;` pull or
    a test-only const, whose line is dropped. Comment-only lines carry neither, so an
    attribute keeps its effect across the comment block between it and its item.
    """
    skipping = False
    depth = 0
    pending = False
    for raw in text.splitlines():
        code = code_of(raw)
        stripped = code.strip()
        if skipping:
            depth += code.count("{") - code.count("}")
            if depth <= 0:
                skipping = False
            continue
        if not stripped:
            continue
        if stripped.startswith("#[cfg(test)]"):
            pending = True
            continue
        if pending:
            pending = False
            if ("mod" in stripped) and stripped.endswith("{"):
                skipping = True
                depth = code.count("{") - code.count("}")
                if depth <= 0:
                    skipping = False
            # A `mod x;` pull or an attributed const or function: the line itself is
            # test-only, and a file a `mod x;` names is caught by `is_test_path`.
            continue
        yield code


def rust_reads(name):
    """Whether any production Rust line outside the parser mentions `name`."""
    pattern = boundary(name.replace("-", "_"))
    for path in sorted(CRATES_SRC.glob("*/src/**/*.rs")):
        if path.relative_to(ROOT).as_posix() == METRICS_RS:
            continue
        if is_test_path(path):
            continue
        for code in production_lines(read_path(path)):
            if pattern.search(code):
                return True
    return False


metrics_text = read_path(ROOT / METRICS_RS)
names = TAKE.findall(metrics_text)
if not names:
    die(f"{METRICS_RS} no longer reads the metrics block")

for name in names:
    if slint_reads(name) == 0 and not rust_reads(name):
        problems.append(
            f'{METRICS_RS} parses "{name}" from the metrics block, and no .slint '
            "source or production Rust module reads it"
        )

if problems:
    print(
        "check-metrics-readers: FAIL - the metrics block has constants the parser"
        " lifts but nothing reads",
        file=sys.stderr,
    )
    for problem in problems:
        print(f"  - {problem}", file=sys.stderr)
    print(
        "check-metrics-readers: delete the constant from ui/candidate.slint and its"
        " parse from src/layout/metrics.rs together, or wire it in; a number three"
        " files carry and none uses is a defect, not a reserve",
        file=sys.stderr,
    )
    sys.exit(1)

print(
    f"check-metrics-readers: PASS ({len(names)} metrics parsed from the"
    " CandidateMetrics block, each read by a .slint source or a production"
    " Rust module)"
)
PY
}

# expect_clean ROOT DESCRIPTION: assert the analyser passes.
expect_clean() {
    local status=0 output=""
    if output="$(analyse "$1" 2>&1)"; then
        status=0
    else
        status=$?
    fi
    if [ "$status" -ne 0 ]; then
        echo "check-metrics-readers: self-test FAILED - $2 exited $status, expected 0" >&2
        echo "$output" >&2
        return 1
    fi
    return 0
}

# expect_violation ROOT EXPECTED_TEXT DESCRIPTION: assert a violation is reported.
expect_violation() {
    local status=0 output=""
    if output="$(analyse "$1" 2>&1)"; then
        status=0
    else
        status=$?
    fi
    if [ "$status" -eq 0 ]; then
        echo "check-metrics-readers: self-test FAILED - $3 was not detected" >&2
        echo "$output" >&2
        return 1
    fi
    case "$output" in
        *"$2"*) ;;
        *)
            echo "check-metrics-readers: self-test FAILED - $3 was reported for the wrong reason" >&2
            echo "check-metrics-readers: expected the report to mention: $2" >&2
            echo "$output" >&2
            return 1
            ;;
    esac
    return 0
}

# copy_tree DEST: everything the analyser reads, at the same relative paths.
copy_tree() {
    local destination="$1"
    mkdir -p -- "$destination"
    cp -R -- "$root/crates" "$destination/crates"
}

run_self_test() {
    local scratch tree
    scratch="$(mktemp -d "${TMPDIR:-/tmp}/rspinyin-metrics-readers.XXXXXX")"
    # shellcheck disable=SC2064  # expand the path now, not at trap time
    trap "rm -rf -- '$scratch'" EXIT

    echo "check-metrics-readers: self-test (one injected reader-less metric per scene)"

    tree="$scratch/tree"

    # 1. The untouched sources pass.
    copy_tree "$tree"
    expect_clean "$tree" "the untouched sources"

    # 2. A metric the parser lifts and nothing knows: the injected line makes the
    #    parser demand a constant no file reads, which is the defect this gate exists
    #    to catch.
    copy_tree "$tree"
    sed -i 's|            grid_gap: take(found, "grid-gap")?,|            grid_gap: take(found, "grid-gap")?,\n            ghost: take(found, "not-a-real-metric")?,|' \
        "$tree/crates/ime-ui/src/layout/metrics.rs"
    expect_violation "$tree" "not-a-real-metric" "a parsed metric with no reader"

    # 3. The same injection with a reader only a `#[cfg(test)]` module sees: test-only
    #    code must not keep a dead parse alive, so the violation stands.
    sed -i 's|^mod collapse_tests {|mod collapse_tests {\n    fn ghost_reader() {\n        let read = not_a_real_metric;\n        let _ = read;\n    }|' \
        "$tree/crates/ime-ui/src/renderer.rs"
    expect_violation "$tree" "not-a-real-metric" "a metric whose only reader is test-only"

    # 4. The same injection with a production reader: the positive path, proving the
    #    gate turns on a genuine mention rather than passing everything.
    printf '\n// Injected by the check-metrics-readers self-test.\npub fn ghost_metrics_reader() -> u8 {\n    let reader = not_a_real_metric;\n    reader\n}\n' \
        >>"$tree/crates/ime-ui/src/lib.rs"
    expect_clean "$tree" "the injected production reader"

    echo "check-metrics-readers: self-test PASS (4 scenes, all judged correctly)"
}

if [ "$mode" = "self-test" ]; then
    run_self_test
    exit 0
fi

analyse "$root"
