#!/usr/bin/env bash
#
# check-readme-keys.sh - keep the README configuration samples and the first-start
# template key-for-key equal.
#
# The template is DEFAULT_CONFIG_TOML, the raw string literal in
# crates/ime-config/src/reload.rs: the document a fresh installation is given and
# the statement of the built-in defaults. The samples are the ```toml code blocks of
# README.md and README.zh.md. The gate fails when a key lives in one document and not
# in the other, in either direction, or when a document's schema_version has drifted
# from CONFIG_SCHEMA_VERSION in crates/ime-types/src/version.rs.
#
# Why a script: the samples used to declare schema_version = 1 against the contract's
# 2 and to omit a score of keys the template carries, and nothing caught it. Both
# documents are maintained by hand, so the only durable fix is a diff that runs in
# the audit suite.
#
# Exit codes: 0 = pass, 1 = drift detected, 2 = usage or environment error.
#
# Self-test: `--self-test` first runs the real comparison (the repository must pass
# its own gate), then feeds the same comparison drifted copies of the template -- one
# key dropped, one key added, the version bumped -- and asserts each drift is caught
# and named.
#
# Requires only coreutils, sed, awk and diff (no python3, no cargo): the check runs
# anywhere the repository is checked out.

set -euo pipefail

# Sorting and diffing must not depend on the caller's locale: the byte order of the
# key lists is what the comparison is made of.
export LC_ALL=C

usage() {
    cat <<'USAGE'
usage: scripts/check-readme-keys.sh [--self-test] [--root DIR]

  --self-test   inject drift into copies of the template and assert it is caught
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
            echo "check-readme-keys: unknown argument: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

template_rs="$root/crates/ime-config/src/reload.rs"
version_rs="$root/crates/ime-types/src/version.rs"
readmes=("$root/README.md" "$root/README.zh.md")

for file in "$template_rs" "$version_rs" "${readmes[@]}"; do
    if [ ! -f "$file" ]; then
        echo "check-readme-keys: required file is missing: $file" >&2
        exit 2
    fi
done

# The TOML document carried by the template's raw string literal: everything between
# the opening `r##"` and the closing `"##;` delimiter, both excluded.
template_toml() {
    sed -n '/r##"/,/^"##;/p' "$template_rs" | sed '1d;$d'
}

# The ```toml code blocks of a README: everything between the opening fence and the
# next closing fence, both excluded.
readme_toml() {
    sed -n '/^```toml/,/^```[[:space:]]*$/p' "$1" | sed '1d;$d'
}

# The fully-qualified keys of the TOML document on stdin, one per line:
# `schema_version` at the top level, `section.key` inside a table, `section.sub.key`
# inside a table of a table. Comment lines are skipped, and a key line is anything
# that starts with an identifier and carries an `=`.
toml_keys() {
    awk '
        /^[[:space:]]*#/ { next }
        /^[[:space:]]*\[/ {
            section = $0
            sub(/^[[:space:]]*\[/, "", section)
            sub(/\].*$/, "", section)
            gsub(/[[:space:]]/, "", section)
            next
        }
        /^[A-Za-z_][A-Za-z0-9_-]*[[:space:]]*=/ {
            key = $0
            sub(/[[:space:]]*=.*$/, "", key)
            if (section == "") print key; else print section "." key
        }
    '
}

# The schema_version the TOML document on stdin declares, or nothing.
toml_version() {
    awk '
        /^schema_version[[:space:]]*=/ {
            sub(/^[^=]*=/, "")
            gsub(/[^0-9]/, "")
            print
            exit
        }
    '
}

# The CONFIG_SCHEMA_VERSION constant of the frozen contract.
contract_version() {
    sed -n 's/^pub const CONFIG_SCHEMA_VERSION: u16 = \([0-9][0-9]*\);/\1/p' "$version_rs"
}

# compare SAMPLE_TOML NAME WORK
#
# Asserts the sample's key set and schema_version against the template's. Reports
# every difference and answers 1 on drift, 2 when an extractor came up empty; the
# caller decides whether that is a failure or the self-test's expectation.
compare() {
    local sample="$1" name="$2" work="$3" status=0

    template_toml | toml_keys | sort >"$work/template.keys"
    toml_keys <"$sample" | sort >"$work/sample.keys"

    if [ ! -s "$work/template.keys" ]; then
        echo "check-readme-keys: no keys found in DEFAULT_CONFIG_TOML; the extractor broke" >&2
        return 2
    fi
    if [ ! -s "$work/sample.keys" ]; then
        echo "check-readme-keys: no keys found in the $name sample; the extractor broke" >&2
        return 2
    fi

    if ! diff -u "$work/template.keys" "$work/sample.keys" >"$work/keys.diff"; then
        echo "check-readme-keys: FAIL - the $name sample and DEFAULT_CONFIG_TOML disagree on keys:" >&2
        cat "$work/keys.diff" >&2
        status=1
    fi

    local expected declared
    expected="$(contract_version)"
    if [ -z "$expected" ]; then
        echo "check-readme-keys: CONFIG_SCHEMA_VERSION not found in $version_rs; the extractor broke" >&2
        return 2
    fi

    declared="$(template_toml | toml_version)"
    if [ "$declared" != "$expected" ]; then
        echo "check-readme-keys: FAIL - DEFAULT_CONFIG_TOML declares schema_version = $declared," >&2
        echo "check-readme-keys:        but ime-types' CONFIG_SCHEMA_VERSION is $expected" >&2
        status=1
    fi
    declared="$(toml_version <"$sample")"
    if [ "$declared" != "$expected" ]; then
        echo "check-readme-keys: FAIL - the $name sample declares schema_version = $declared," >&2
        echo "check-readme-keys:        but ime-types' CONFIG_SCHEMA_VERSION is $expected" >&2
        status=1
    fi

    return "$status"
}

# run_real_check WORK
#
# The gate itself: every README sample against the template. Answers 1 on drift.
run_real_check() {
    local work="$1" readme name status=0
    template_toml >"$work/template.toml"
    for readme in "${readmes[@]}"; do
        name="$(basename "$readme")"
        readme_toml "$readme" >"$work/$name.toml"
        if ! compare "$work/$name.toml" "$name" "$work"; then
            status=1
        fi
    done
    return "$status"
}

work="$(mktemp -d "${TMPDIR:-/tmp}/rspinyin-readme-keys.XXXXXX")"
trap 'rm -rf -- "$work"' EXIT

if [ "$mode" = "self-test" ]; then
    echo "check-readme-keys: self-test (drifting copies of the template)"
    if ! run_real_check "$work"; then
        echo "check-readme-keys: self-test FAILED - the repository's own documents already" >&2
        echo "check-readme-keys: drift; fix that before trusting the self-test" >&2
        exit 1
    fi

    # A key the template loses must be reported as a missing key. `abbrev` is the
    # line dropped; any key would do.
    grep -v '^abbrev = ' "$work/template.toml" >"$work/dropped.toml"
    if ! output="$(compare "$work/dropped.toml" "dropped-key fixture" "$work" 2>&1)"; then
        case "$output" in
            *"engine.abbrev"*) ;;
            *)
                echo "check-readme-keys: self-test FAILED - a dropped key was reported for the wrong reason" >&2
                printf '%s\n' "$output" >&2
                exit 1
                ;;
        esac
    else
        echo "check-readme-keys: self-test FAILED - a dropped key was not detected" >&2
        printf '%s\n' "$output" >&2
        exit 1
    fi

    # A key the template gains must be reported too, in the other direction of the
    # same diff. Appended at the end, it lands in the document's last section.
    cp "$work/template.toml" "$work/added.toml"
    printf 'bogus_key = true\n' >>"$work/added.toml"
    if ! output="$(compare "$work/added.toml" "added-key fixture" "$work" 2>&1)"; then
        case "$output" in
            *"diagnostics.bogus_key"*) ;;
            *)
                echo "check-readme-keys: self-test FAILED - an added key was reported for the wrong reason" >&2
                printf '%s\n' "$output" >&2
                exit 1
                ;;
        esac
    else
        echo "check-readme-keys: self-test FAILED - an added key was not detected" >&2
        printf '%s\n' "$output" >&2
        exit 1
    fi

    # A schema_version that drifts from the frozen contract must be reported even
    # when the keys agree.
    sed 's/^schema_version = [0-9][0-9]*/schema_version = 999/' "$work/template.toml" \
        >"$work/version.toml"
    if ! output="$(compare "$work/version.toml" "version fixture" "$work" 2>&1)"; then
        case "$output" in
            *"version fixture sample declares schema_version = 999"*) ;;
            *)
                echo "check-readme-keys: self-test FAILED - a drifted schema_version was reported for the wrong reason" >&2
                printf '%s\n' "$output" >&2
                exit 1
                ;;
        esac
    else
        echo "check-readme-keys: self-test FAILED - a drifted schema_version was not detected" >&2
        printf '%s\n' "$output" >&2
        exit 1
    fi

    echo "check-readme-keys: self-test PASS (a dropped key, an added key and a drifted"
    echo "check-readme-keys: schema_version are each caught, and the repository's own"
    echo "check-readme-keys: documents pass)"
    exit 0
fi

if run_real_check "$work"; then
    key_count="$(template_toml | toml_keys | sort | wc -l | tr -d ' ')"
    echo "check-readme-keys: PASS ($key_count keys, schema_version $(contract_version),"
    echo "check-readme-keys: ${#readmes[@]} README samples agree with the first-start template)"
    exit 0
fi
exit 1
