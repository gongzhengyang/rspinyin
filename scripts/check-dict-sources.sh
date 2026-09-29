#!/usr/bin/env bash
#
# check-dict-sources.sh - keep copyleft dictionary data out of base.dict.
#
# ADR-0000 decision 2 is irreversible in practice: once copyleft data is mixed
# into the built dictionary, stripping it out again means rebuilding every word
# frequency and every expanded key. The whitelist is therefore enforced at build
# time, and this script is its CI half.
#
# For every `data/raw/*.tsv` the script asserts that
#
#   * the file name (`<id>.tsv`) is registered in `data/sources.toml`;
#   * the entry declares a licence identifier and a SHA256;
#   * the recorded SHA256 matches the file, unless the entry is project-derived;
#   * the licence is permissive (MIT / Apache-2.0 / BSD / ISC / Unicode / CC0),
#     and the source is not on the excluded list of ADR-0000 (luna-pinyin,
#     CC-CEDICT, commercial IME dictionaries, research-only word lists).
#
# Upstream sources are hash-pinned: a present file with an empty `sha256` fails
# with the exact command to compute the value, so the gate cannot be satisfied by
# an unfinished entry. Project-generated files (`kind = "derived"`) are exempt
# from pinning because they change whenever the generator changes, but they still
# need a licence entry.
#
# `data/raw/` is empty until the dictionary sources land; the check then passes
# vacuously and says so.
#
# Exit codes: 0 = pass, 1 = violation, 2 = usage or environment error.
#
# Self-test: `--self-test` builds a fixture tree in a temporary directory and
# asserts each violation class is detected and the clean tree is accepted.
#
# Requires python3 (present by default on every distro in the platform baseline).

set -euo pipefail

usage() {
    cat <<'USAGE'
usage: scripts/check-dict-sources.sh [--self-test] [--raw-dir DIR] [--sources FILE] [--root DIR]

  --self-test      build a fixture tree and assert each violation is detected
  --raw-dir DIR    directory of raw sources (default: <root>/data/raw)
  --sources FILE   allowlist file (default: <root>/data/sources.toml)
  --root DIR       repository root (default: parent of this script)
USAGE
}

mode="check"
raw_dir=""
sources_file=""
root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"

while [ $# -gt 0 ]; do
    case "$1" in
        --self-test) mode="self-test"; shift ;;
        --raw-dir)
            raw_dir="${2:?--raw-dir needs a directory}"
            shift 2
            ;;
        --sources)
            sources_file="${2:?--sources needs a path}"
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
            echo "check-dict-sources: unknown argument: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

[ -n "$raw_dir" ] || raw_dir="$root/data/raw"
[ -n "$sources_file" ] || sources_file="$root/data/sources.toml"

if ! command -v python3 >/dev/null 2>&1; then
    echo "check-dict-sources: python3 is required to hash and parse sources; install it and re-run" >&2
    exit 2
fi

# check RAW_DIR SOURCES_FILE: validate every raw source against the allowlist.
check() {
    python3 - "$1" "$2" <<'PY'
import hashlib
import os
import re
import sys
from pathlib import Path

raw_dir, sources_path = Path(sys.argv[1]), Path(sys.argv[2])

# ADR-0000 decision 2: permissive licences only, so the derived database cannot
# be pulled into a copyleft.
PERMISSIVE = frozenset(
    {
        "MIT",
        "Apache-2.0",
        "BSD-2-Clause",
        "BSD-3-Clause",
        "ISC",
        "Zlib",
        "0BSD",
        "Unicode-3.0",
        "Unicode-DFS-2016",
        "CC0-1.0",
    }
)

# Sources ADR-0000 excludes, matched case-insensitively against id, url and file.
EXCLUDED = {
    "luna": "LGPL-3.0 data; keep it a user-importable extension, never part of base.dict",
    "cedict": "CC BY-SA 4.0; ShareAlike would relicense the derived database",
    "sogou": "commercial dictionary, no licence",
    "baidu": "commercial dictionary, no licence",
    "qq": "commercial dictionary, no licence",
    "thuocl": "licence not yet verified; register it with an ADR before use",
    "subtlex": "research-only licence",
}

REQUIRED_FIELDS = ("id", "layer", "url", "license", "spdx", "retrieved", "sha256", "permissive")
LAYER = re.compile(r"^L[1-5][abc]?$")
DATE = re.compile(r"^\d{4}-\d{2}-\d{2}$")
IGNORED_SUFFIXES = (".md", ".gitkeep", ".gitignore")

problems = []


def parse_allowlist(text):
    """Parse the [[source]] tables, with tomllib when the interpreter has it."""
    try:
        import tomllib  # Python 3.11+, available on the CI image
    except ImportError:
        tomllib = None

    if tomllib is not None:
        try:
            document = tomllib.loads(text)
        except tomllib.TOMLDecodeError as error:
            sys.exit(f"check-dict-sources: {sources_path}: invalid TOML: {error}")
        unknown = sorted(key for key in document if key != "source")
        if unknown:
            sys.exit(
                f"check-dict-sources: {sources_path}: unsupported table(s): "
                f"{', '.join(unknown)}; only [[source]] is understood"
            )
        return document.get("source", [])

    # Minimal fallback for interpreters older than 3.11: the subset of TOML this
    # project's allowlist uses, and nothing else.
    entries = []
    current = None
    for number, raw in enumerate(text.splitlines(), start=1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        if line.startswith("[[") and line.endswith("]]"):
            table = line[2:-2].strip()
            if table != "source":
                sys.exit(
                    f"check-dict-sources: {sources_path}:{number}: unsupported table "
                    f"[[{table}]]; only [[source]] is understood"
                )
            current = {}
            entries.append(current)
            continue
        key, separator, value = line.partition("=")
        if not separator or current is None:
            sys.exit(
                f"check-dict-sources: {sources_path}:{number}: expected `key = value` "
                f"inside a [[source]] table"
            )
        value = value.strip()
        if value.startswith('"') and value.endswith('"') and len(value) >= 2:
            current[key.strip()] = value[1:-1]
        elif value in ("true", "false"):
            current[key.strip()] = value == "true"
        else:
            sys.exit(
                f"check-dict-sources: {sources_path}:{number}: unsupported value {value!r}; "
                f"only quoted strings, true and false are supported"
            )
    return entries


if not sources_path.is_file():
    print(f"check-dict-sources: FAIL - allowlist not found: {sources_path}", file=sys.stderr)
    sys.exit(1)

entries = parse_allowlist(sources_path.read_text(encoding="utf-8"))
if not entries:
    print(f"check-dict-sources: FAIL - {sources_path} declares no [[source]]", file=sys.stderr)
    sys.exit(1)

declared = {}
for entry in entries:
    if not isinstance(entry, dict):
        problems.append(f"{sources_path}: entry is not a table")
        continue
    identifier = str(entry.get("id", "")).strip()
    if not identifier:
        problems.append(f"{sources_path}: a [[source]] entry has no `id`")
        continue
    if identifier in declared:
        problems.append(f"{sources_path}: duplicate source id `{identifier}`")
    declared[identifier] = entry

    for field in REQUIRED_FIELDS:
        if field not in entry:
            problems.append(f"{sources_path}: `{identifier}` is missing the `{field}` field")
    layer = str(entry.get("layer", ""))
    if layer and not LAYER.match(layer):
        problems.append(f"{sources_path}: `{identifier}` has unsupported layer `{layer}`")
    if entry.get("permissive") is not True:
        problems.append(
            f"{sources_path}: `{identifier}` is not marked permissive; ADR-0000 allows "
            f"permissive sources only (add an ADR before admitting a copyleft source)"
        )
    retrieved = str(entry.get("retrieved", ""))
    if retrieved and not DATE.match(retrieved):
        problems.append(
            f"{sources_path}: `{identifier}` has malformed `retrieved` date `{retrieved}`"
        )
    # `spdx` is the machine-readable identifier and the one that is validated;
    # `license` is a human-readable label, checked for copyleft markers below.
    spdx = str(entry.get("spdx", ""))
    if spdx:
        identifiers = [
            token
            for token in re.split(r"[\s()]+", spdx)
            if token and token not in ("OR", "AND", "WITH")
        ]
        unknown = sorted(token for token in identifiers if token not in PERMISSIVE)
        if unknown:
            problems.append(
                f"{sources_path}: `{identifier}` declares non-permissive spdx "
                f"`{spdx}` ({', '.join(unknown)}); ADR-0000 decision 2 requires "
                f"MIT / Apache-2.0 / BSD / ISC / Unicode / CC0"
            )
    label = f"{entry.get('license', '')} {spdx}".lower()
    for marker in ("lgpl", "agpl", "gpl", "by-sa", "sharealike"):
        if marker in label:
            problems.append(
                f"{sources_path}: `{identifier}` declares the copyleft licence "
                f"`{entry.get('license', '')}` ({marker}); ADR-0000 excludes it"
            )
            break
    haystack = f"{identifier} {entry.get('url', '')}".lower()
    for needle, reason in sorted(EXCLUDED.items()):
        if needle in haystack:
            problems.append(
                f"{sources_path}: `{identifier}` matches the excluded source `{needle}` "
                f"({reason})"
            )

present = []
if raw_dir.is_dir():
    for path in sorted(raw_dir.iterdir()):
        if not path.is_file() or path.name.startswith("."):
            continue
        if path.suffix == ".tsv":
            present.append(path)
        elif path.suffix not in IGNORED_SUFFIXES:
            problems.append(
                f"{path}: only `.tsv` sources are validated; move this file out of {raw_dir}"
            )

for path in present:
    identifier = path.stem
    entry = declared.get(identifier)
    if entry is None:
        problems.append(
            f"{path}: source `{identifier}` is not registered in {sources_path}; "
            f"add a [[source]] entry (ADR-0000 requires an open licence for every source)"
        )
        continue
    actual = hashlib.sha256(path.read_bytes()).hexdigest()
    recorded = str(entry.get("sha256", "")).strip().lower()
    derived = str(entry.get("kind", "upstream")).strip() == "derived"
    if not recorded:
        if derived:
            continue
        problems.append(
            f"{path}: `sha256` is empty for the upstream source `{identifier}`; "
            f"pin it to {actual} in {sources_path}"
        )
        continue
    if recorded != actual:
        problems.append(
            f"{path}: sha256 mismatch for `{identifier}`; {sources_path} records "
            f"{recorded}, the file hashes to {actual}"
        )
    if not str(entry.get("retrieved", "")).strip():
        problems.append(
            f"{path}: `retrieved` is empty for the upstream source `{identifier}`; "
            f"record the download date in {sources_path}"
        )

if problems:
    print("check-dict-sources: FAIL", file=sys.stderr)
    for problem in problems:
        print(f"  - {problem}", file=sys.stderr)
    sys.exit(1)

if not present:
    print(
        f"check-dict-sources: PASS (vacuous: no .tsv sources in {raw_dir} yet; "
        f"{len(declared)} source(s) declared)"
    )
else:
    print(
        f"check-dict-sources: PASS ({len(present)} raw source(s) verified against "
        f"{sources_path}, {len(declared)} declared)"
    )
PY
}

hash_of() {
    python3 - "$1" <<'PY'
import hashlib
import sys
from pathlib import Path

print(hashlib.sha256(Path(sys.argv[1]).read_bytes()).hexdigest())
PY
}

expect_clean() {
    # expect_clean DESCRIPTION
    local status=0 output=""
    if output="$(check "$raw_dir" "$sources_file" 2>&1)"; then
        status=0
    else
        status=$?
    fi
    if [ "$status" -ne 0 ]; then
        echo "check-dict-sources: self-test FAILED - $1 exited $status, expected 0" >&2
        echo "$output" >&2
        return 1
    fi
    return 0
}

expect_violation() {
    # expect_violation EXPECTED_TEXT DESCRIPTION
    local status=0 output=""
    if output="$(check "$raw_dir" "$sources_file" 2>&1)"; then
        status=0
    else
        status=$?
    fi
    if [ "$status" -eq 0 ]; then
        echo "check-dict-sources: self-test FAILED - $2 was not detected" >&2
        echo "$output" >&2
        return 1
    fi
    case "$output" in
        *"$1"*) ;;
        *)
            echo "check-dict-sources: self-test FAILED - $2 was reported for the wrong reason" >&2
            echo "check-dict-sources: expected the report to mention: $1" >&2
            echo "$output" >&2
            return 1
            ;;
    esac
    return 0
}

write_allowlist() {
    # write_allowlist FILE ALPHA_SHA256 PERMISSIVE
    cat >"$1" <<EOF
# rspinyin dictionary source allowlist (self-test fixture).
[[source]]
id = "alpha"
kind = "upstream"
layer = "L1"
url = "https://example.invalid/alpha"
license = "MIT"
spdx = "MIT"
retrieved = "2026-09-29"
sha256 = "$2"
permissive = $3

[[source]]
id = "beta"
kind = "derived"
layer = "L2"
url = "https://example.invalid/beta"
license = "MIT OR Apache-2.0"
spdx = "MIT OR Apache-2.0"
retrieved = "2026-09-29"
sha256 = ""
permissive = true

[[source]]
id = "gamma"
kind = "upstream"
layer = "L2"
url = "https://example.invalid/gamma"
license = "Apache-2.0"
spdx = "Apache-2.0"
retrieved = "2026-09-29"
sha256 = ""
permissive = true
EOF
}

run_self_test() {
    local scratch alpha_hash
    scratch="$(mktemp -d "${TMPDIR:-/tmp}/rspinyin-dict.XXXXXX")"
    # shellcheck disable=SC2064  # expand the path now, not at trap time
    trap "rm -rf -- '$scratch'" EXIT
    raw_dir="$scratch/raw"
    sources_file="$scratch/sources.toml"
    mkdir -p "$raw_dir"

    printf '词\t拼音\t权重\n银行\tyin2hang2\t30000\n' >"$raw_dir/alpha.tsv"
    printf '词\t拼音\n测试\tce4shi4\n' >"$raw_dir/beta.tsv"
    alpha_hash="$(hash_of "$raw_dir/alpha.tsv")"
    write_allowlist "$sources_file" "$alpha_hash" true

    echo "check-dict-sources: self-test (fixture tree: $scratch)"
    expect_clean "the pinned fixture"

    printf '词\t拼音\t权重\n恶意\te4yi4\t1\n' >"$raw_dir/rogue.tsv"
    expect_violation "rogue" "an unregistered source"
    rm -f -- "$raw_dir/rogue.tsv"

    printf '\n# appended after pinning\n' >>"$raw_dir/alpha.tsv"
    expect_violation "sha256 mismatch" "a source whose contents changed after pinning"

    printf '词\t拼音\t权重\n银行\tyin2hang2\t30000\n' >"$raw_dir/alpha.tsv"
    expect_clean "the restored fixture"

    printf '词\t拼音\n未固定\twei4gu4ding4\n' >"$raw_dir/gamma.tsv"
    expect_violation "pin it to" "an unpinned upstream source"
    rm -f -- "$raw_dir/gamma.tsv"

    printf '词\t拼音\n繁體\ttai2ti3\n' >"$raw_dir/luna-pinyin.tsv"
    expect_violation "not registered" "a file with no allowlist entry"
    rm -f -- "$raw_dir/luna-pinyin.tsv"

    write_allowlist "$sources_file" "$alpha_hash" false
    expect_violation "not marked permissive" "a non-permissive allowlist entry"
    write_allowlist "$sources_file" "$alpha_hash" true

    cat >>"$sources_file" <<'EOF'

[[source]]
id = "luna-pinyin"
kind = "upstream"
layer = "L2"
url = "https://example.invalid/luna"
license = "LGPL-3.0"
spdx = "LGPL-3.0"
retrieved = "2026-09-29"
sha256 = ""
permissive = true
EOF
    expect_violation "excluded source" "an ADR-0000 excluded source in the allowlist"

    write_allowlist "$sources_file" "$alpha_hash" true
    expect_clean "the fixture after removing the violations"
    echo "check-dict-sources: self-test PASS (5 violation classes detected, clean tree accepted)"
}

if [ "$mode" = "self-test" ]; then
    run_self_test
    exit 0
fi

check "$raw_dir" "$sources_file"
