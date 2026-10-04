#!/usr/bin/env bash
#
# check-fsync-rename.sh - assert that every write-a-temporary-file-then-rename path in
# the workspace flushes the temporary to the device before the rename.
#
# rename(2) makes the swap atomic, but only the fsync of the temporary makes the
# swapped-in contents complete: without it a power loss can leave a complete name over
# an incomplete file. The library's write-and-rename paths -- the dictionary writer,
# the user store's backup and export, the config migration and writeback, the UI
# mirror -- all document the same division of labour, and this gate keeps a new path
# from forgetting the flush half: it greps the whole workspace for
# write-temp-then-rename paths and asserts their fsync coverage agrees.
#
# What is asserted:
#
#   1. every `fs::rename` call under `crates/` is classified. A rename whose source is
#      a temporary file (the argument names `temp`/`temporary`/`temp_path`) is a
#      publish path and must show a `sync_all` within the window above the call, or
#      route its write through a same-file `write_private` helper whose own body
#      fsyncs;
#   2. a rename of an *existing* file -- a log rotation, a damaged file moved to
#      quarantine, an unreadable document moved aside -- publishes no new bytes and is
#      listed in MOVES_EXISTING below, with its reason. The list can only shrink;
#   3. a publish path whose fsync is still missing is listed in DEFERRED with its
#      reason, the same shrink-only shape the UI-spec gate uses for unwired tokens:
#      a held entry passes with a note (the gap is known and owned), while an entry
#      whose violation has disappeared fails the gate until its entry is dropped;
#   4. the analysed set is non-empty: a source shape the scan no longer recognises
#      must fail loudly rather than pass vacuously.
#
# Exit codes: 0 = pass, 1 = violation, 2 = usage or environment error.
#
# Self-test: `--self-test` builds a scratch tree of fixture files -- a compliant
# inline path, a compliant write_private-helper path, a path missing its fsync, an
# unclassified rename, a listed move of an existing file, the deferred entry still
# holding, and the deferred entry gone live -- and asserts the gate classifies each
# one correctly.
#
# Requires python3 (present by default on every distro in the platform baseline).

set -euo pipefail

usage() {
    cat <<'USAGE'
usage: scripts/check-fsync-rename.sh [--self-test] [--root DIR]

  --self-test   build a scratch tree of fixture paths and assert the gate classifies
                each one correctly
  --root DIR    repository root (default: parent of this script)
USAGE
}

mode="check"
root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
while [[ $# -gt 0 ]]; do
    case "$1" in
        --self-test) mode="self-test" ;;
        --root) root="$2"; shift ;;
        *) usage >&2; exit 2 ;;
    esac
    shift
done

run_gate() {
    python3 - "$root" <<'PYEOF'
import re
import sys
from pathlib import Path

root = Path(sys.argv[1])
crates = root / "crates"
if not crates.is_dir():
    print(f"check-fsync-rename: no crates/ directory under {root}", file=sys.stderr)
    sys.exit(2)

# The window of lines above a rename call in which the temporary's fsync must appear.
WINDOW = 12

# Renames of an existing file: nothing new is published, so there is no temporary to
# flush. Matched by (path suffix, literal snippet) so line drift cannot silently widen
# the list; each entry carries the reason it is not a publish path.
MOVES_EXISTING = [
    (
        "ime-diag/src/log.rs",
        "fs::rename(from, to)",
        "log rotation moves an already-durable file",
    ),
    (
        "ime-dict/src/recover/quarantine.rs",
        "fs::rename(target, &destination)",
        "a damaged file is moved aside whole; no bytes are written",
    ),
    (
        "ime-config/src/reload/load.rs",
        "fs::rename(path, &target)",
        "an unreadable document is moved aside whole; no bytes are written",
    ),
]

# Publish paths whose fsync is still missing, pending the same fix the sibling paths
# carry. Same shape as the UI-spec gate's deferred list: an entry whose violation has
# disappeared is a failure until the entry is dropped.
DEFERRED = [
    (
        "ime-diag/src/uiframe/mirror.rs",
        "fs::rename(&temporary, path)",
        "the frame mirror's write_private does not fsync yet; the fix is the "
        "flush its sibling write-and-rename paths carry",
    ),
]

RENAME = re.compile(r"\b(?:std::)?fs::rename\(")
TEMP_SOURCE = re.compile(r"\bfs::rename\(\s*&?\w*(?:temp|temporary)\w*\s*,")
SYNC = "sync_all"
WRITE_HELPER = re.compile(r"^fn write_private\b")


def helper_fsyncs(lines):
    """Whether a same-file `fn write_private` body contains a sync_all."""
    inside = False
    for line in lines:
        if WRITE_HELPER.match(line):
            inside = True
            continue
        if inside:
            if line.startswith("}"):
                return False
            if SYNC in line:
                return True
    return False


violations = []
notes = []
analysed = 0
for path in sorted(crates.rglob("*.rs")):
    lines = path.read_text(encoding="utf-8").splitlines()
    helper_ok = helper_fsyncs(lines)
    for number, line in enumerate(lines, start=1):
        if not RENAME.search(line):
            continue
        analysed += 1
        relative = path.relative_to(root).as_posix()

        deferred = next(
            (
                entry
                for entry in DEFERRED
                if relative.endswith(entry[0]) and entry[1] in line
            ),
            None,
        )
        moves = next(
            (
                entry
                for entry in MOVES_EXISTING
                if relative.endswith(entry[0]) and entry[1] in line
            ),
            None,
        )

        window = lines[max(0, number - 1 - WINDOW) : number - 1]
        flushes = any(SYNC in above for above in window) or helper_ok

        if deferred is not None:
            if flushes:
                violations.append(
                    f"{relative}:{number}: DEFERRED ENTRY NO LONGER HOLDS -- the path "
                    f"now fsyncs; drop it from DEFERRED ({deferred[2]})"
                )
            else:
                notes.append(
                    f"{relative}:{number}: deferred ({deferred[2]})"
                )
            continue
        if moves is not None:
            continue
        if not TEMP_SOURCE.search(line):
            violations.append(
                f"{relative}:{number}: rename is neither a temporary-file publish nor "
                "listed in MOVES_EXISTING; classify it in scripts/check-fsync-rename.sh"
            )
            continue
        if flushes:
            continue
        violations.append(
            f"{relative}:{number}: a write-temp-then-rename path without a sync_all "
            f"within {WINDOW} lines above it (or a fsyncing write_private helper)"
        )

if analysed == 0:
    print(
        "check-fsync-rename: no fs::rename call found under crates/; the scan no "
        "longer recognises the sources and must not pass vacuously",
        file=sys.stderr,
    )
    sys.exit(1)

if violations:
    print("check-fsync-rename: write-temp-then-rename fsync coverage findings:")
    for violation in violations:
        print(f"  {violation}")
    sys.exit(1)

for note in notes:
    print(f"check-fsync-rename: {note}")
print(f"check-fsync-rename: {analysed} rename paths audited, fsync coverage consistent")
PYEOF
}

write_fixture_tree() {
    # $1 = scratch root, $2 = content of the ime-diag mirror fixture ("missing" or
    # "fixed"); the mirror fixture is laid out under the real suffix so the gate's own
    # DEFERRED entry classifies it.
    local tree="$1" mirror="$2"
    mkdir -p "$tree/crates/demo/src" "$tree/crates/ime-diag/src/uiframe"

    cat > "$tree/crates/demo/src/inline_ok.rs" <<'EOF'
fn write_and_rename(temp: &str, target: &str) -> io::Result<()> {
    let mut file = File::create(temp)?;
    file.write_all(b"x")?;
    file.sync_all()?;
    drop(file);
    fs::rename(temp, target)
}
EOF

    cat > "$tree/crates/demo/src/helper_ok.rs" <<'EOF'
fn publish(path: &str, text: &str) -> io::Result<()> {
    let temporary = temporary_path(path);
    write_private(&temporary, text)?;
    fs::rename(&temporary, path)
}

fn write_private(path: &str, text: &str) -> io::Result<()> {
    let mut file = File::create(path)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()
}
EOF

    cat > "$tree/crates/demo/src/missing_fsync.rs" <<'EOF'
fn write_and_rename(temp: &str, target: &str) -> io::Result<()> {
    let mut file = File::create(temp)?;
    file.write_all(b"x")?;
    drop(file);
    fs::rename(temp, target)
}
EOF

    cat > "$tree/crates/demo/src/unclassified.rs" <<'EOF'
fn publish(artifact: &str, target: &str) -> io::Result<()> {
    fs::rename(artifact, target)
}
EOF

    cat > "$tree/crates/ime-diag/src/log.rs" <<'EOF'
fn rename_if_exists(from: &str, to: &str) -> io::Result<()> {
    fs::rename(from, to)
}
EOF

    if [[ "$mirror" == "missing" ]]; then
        cat > "$tree/crates/ime-diag/src/uiframe/mirror.rs" <<'EOF'
fn write_atomically(path: &str, text: &str) -> io::Result<()> {
    let temporary = temporary_path(path);
    write_private(&temporary, text)?;
    fs::rename(&temporary, path)
}

fn write_private(path: &str, text: &str) -> io::Result<()> {
    let mut file = File::create(path)?;
    file.write_all(text.as_bytes())?;
    Ok(())
}
EOF
    elif [[ "$mirror" == "fixed" ]]; then
        cat > "$tree/crates/ime-diag/src/uiframe/mirror.rs" <<'EOF'
fn write_atomically(path: &str, text: &str) -> io::Result<()> {
    let temporary = temporary_path(path);
    write_private(&temporary, text)?;
    fs::rename(&temporary, path)
}

fn write_private(path: &str, text: &str) -> io::Result<()> {
    let mut file = File::create(path)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    Ok(())
}
EOF
    fi
}

self_test() {
    local scratch out
    scratch="$(mktemp -d)"
    out="$(mktemp)"
    trap 'rm -rf "$scratch" "$out"' EXIT

    # Run 1: the compliant fixtures pass and the two misbehaving ones are named.
    write_fixture_tree "$scratch" none
    if bash "$0" --root "$scratch" >"$out" 2>&1; then
        echo "check-fsync-rename self-test: a tree with a missing fsync passed" >&2
        cat "$out" >&2
        exit 1
    fi
    grep -q "missing_fsync.rs" "$out" ||
        { echo "check-fsync-rename self-test: the missing fsync was not named" >&2; exit 1; }
    grep -q "unclassified.rs" "$out" ||
        { echo "check-fsync-rename self-test: the unclassified rename was not named" >&2; exit 1; }
    grep -q "inline_ok.rs" "$out" &&
        { echo "check-fsync-rename self-test: a compliant path was named" >&2; exit 1; }
    grep -q "helper_ok.rs" "$out" &&
        { echo "check-fsync-rename self-test: the helper path was named" >&2; exit 1; }
    grep -q "ime-diag/src/log.rs" "$out" &&
        { echo "check-fsync-rename self-test: a listed move was named" >&2; exit 1; }

    # Run 2: the deferred entry still holding passes, with a note naming the path.
    write_fixture_tree "$scratch" missing
    rm "$scratch/crates/demo/src/missing_fsync.rs" "$scratch/crates/demo/src/unclassified.rs"
    if ! bash "$0" --root "$scratch" >"$out" 2>&1; then
        echo "check-fsync-rename self-test: a held deferred entry failed the gate" >&2
        cat "$out" >&2
        exit 1
    fi
    grep -q "deferred" "$out" ||
        { echo "check-fsync-rename self-test: the held deferred entry was not noted" >&2; exit 1; }
    grep -q "uiframe/mirror.rs" "$out" ||
        { echo "check-fsync-rename self-test: the deferred note did not name the path" >&2; exit 1; }

    # Run 3: the deferred entry gone live fails with the drop-the-entry message.
    write_fixture_tree "$scratch" fixed
    rm "$scratch/crates/demo/src/missing_fsync.rs" "$scratch/crates/demo/src/unclassified.rs"
    if bash "$0" --root "$scratch" >"$out" 2>&1; then
        echo "check-fsync-rename self-test: a live deferred entry passed" >&2
        exit 1
    fi
    grep -q "NO LONGER HOLDS" "$out" ||
        {
            echo "check-fsync-rename self-test: a fixed deferred path did not ask for its entry to be dropped" >&2
            cat "$out" >&2
            exit 1
        }

    echo "check-fsync-rename self-test: all fixture classifications behaved"
}

if [[ "$mode" == "self-test" ]]; then
    self_test
else
    run_gate
fi
