#!/usr/bin/env bash
#
# Fetch the upstream dictionary sources registered in data/sources.toml.
#
# Everything this script writes lands in data/raw/, which is not committed: the raw
# upstream files are large, and `sha256` in data/sources.toml pins them, so the fetch is
# reproducible without keeping a copy in the repository. The derived, committed artifacts
# (base.tsv, polyphone.tsv) are produced separately.
#
# Each source is downloaded, normalised into the `<id>.tsv` shape the compiler expects,
# and verified against the recorded digest. A digest mismatch is fatal: silently
# compiling a different upstream revision than the one that was measured would invalidate
# every quality number recorded in the architecture decision record.
#
# Usage:
#   data/fetch.sh              fetch every upstream source
#   data/fetch.sh <id>...      fetch only the named sources
#   data/fetch.sh --force      download even when the cached copy already matches
#   data/fetch.sh --record     fetch, then print the digests to paste into sources.toml
#
# A source whose file under data/raw/ already hashes to the value recorded in
# data/sources.toml is not downloaded again. The digest is what decides, never the file's
# presence: a cache that restores data/raw/ is re-verified rather than trusted, and only a
# file that fails the comparison goes to the network. `--force` overrides the check, and
# `--record` implies it because that mode exists to measure what upstream serves today.

set -euo pipefail

readonly repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly raw_dir="${repo_root}/data/raw"
readonly sources_toml="${repo_root}/data/sources.toml"

# Download locations. The URL in sources.toml is the human-facing project page; these are
# the machine-readable artifacts, and they are what the digest covers.
url_for() {
    case "$1" in
        pinyin-data) echo "https://raw.githubusercontent.com/mozillazg/pinyin-data/master/pinyin.txt" ;;
        jieba-dict)  echo "https://raw.githubusercontent.com/fxsjy/jieba/master/jieba/dict.txt" ;;
        unihan)      echo "https://www.unicode.org/Public/UCD/latest/ucd/Unihan.zip" ;;
        *) return 1 ;;
    esac
}

recorded_sha256() {
    python3 - "$sources_toml" "$1" <<'PY'
import sys, tomllib
with open(sys.argv[1], "rb") as fh:
    doc = tomllib.load(fh)
for src in doc.get("source", []):
    if src.get("id") == sys.argv[2]:
        print(src.get("sha256", "") or "")
        break
PY
}

# Whether the copy already under data/raw/ is the file the pin describes.
#
# Only the digest decides. A cached file that matches is byte for byte what a download
# followed by normalisation would have produced, so re-fetching it costs a transfer and
# proves nothing; a file that does not match is downloaded and re-verified below, which is
# where an upstream change and an accidental change to the normalisers are caught. An
# unpinned source has nothing to compare against and is always fetched.
cached_source_matches() {
    local id="$1" path="${raw_dir}/${id}.tsv" expected actual
    [[ -s "$path" ]] || return 1
    expected="$(recorded_sha256 "$id")"
    [[ -n "$expected" ]] || return 1
    actual="$(sha256sum "$path" | cut -d' ' -f1)"
    [[ "$expected" == "$actual" ]]
}

download() {
    local id="$1" dest="$2" url attempt
    url="$(url_for "$id")"
    echo "fetch: ${id} <- ${url}"
    # unicode.org truncates long transfers from some networks. A truncated archive is
    # indistinguishable from a corrupt one downstream, so retry with resume until the
    # transfer actually completes rather than accepting a partial file. Text sources are
    # guarded by the digest check below; only the zip can be validated cheaply here.
    for attempt in $(seq 1 40); do
        curl --fail --location --silent --show-error \
             --continue-at - --output "$dest" "$url" || true
        if [[ ! -s "$dest" ]]; then
            continue
        fi
        case "$id" in
            unihan)
                if unzip -t "$dest" >/dev/null 2>&1; then
                    return 0
                fi
                echo "  attempt ${attempt}: $(stat -c%s "$dest") bytes (incomplete)"
                ;;
            *)
                return 0
                ;;
        esac
    done
    echo "fetch: ${id}: download did not complete after 40 attempts" >&2
    return 1
}

# pinyin.txt:  `U+4E00: yī,yī  # 一`  ->  `一<TAB>yī,yī`
normalise_pinyin_data() {
    python3 - "$1" "$2" <<'PY'
import re, sys
out = []
for line in open(sys.argv[1], encoding="utf-8"):
    line = line.split("#", 1)[0].strip()
    if not line or ":" not in line:
        continue
    codepoint, readings = line.split(":", 1)
    readings = ",".join(r.strip() for r in readings.split(",") if r.strip())
    if not readings:
        continue
    try:
        char = chr(int(codepoint.strip().removeprefix("U+"), 16))
    except ValueError:
        continue
    out.append(f"{char}\t{readings}")
with open(sys.argv[2], "w", encoding="utf-8") as fh:
    fh.write("\n".join(out) + "\n")
print(f"  {len(out)} character readings")
PY
}

# dict.txt:  `词 123 n`  ->  `词<TAB>123`
normalise_jieba() {
    python3 - "$1" "$2" <<'PY'
import sys
out = []
for line in open(sys.argv[1], encoding="utf-8"):
    parts = line.split()
    if len(parts) < 2:
        continue
    word, freq = parts[0], parts[1]
    if not freq.isdigit():
        continue
    out.append(f"{word}\t{freq}")
with open(sys.argv[2], "w", encoding="utf-8") as fh:
    fh.write("\n".join(out) + "\n")
print(f"  {len(out)} words with frequencies")
PY
}

# Unihan_Readings.txt:  `U+4E00\tkMandarin\tyī`  ->  `一<TAB>yī`
normalise_unihan() {
    python3 - "$1" "$2" <<'PY'
import sys, zipfile
out = []
with zipfile.ZipFile(sys.argv[1]) as archive:
    with archive.open("Unihan_Readings.txt") as fh:
        for raw in fh:
            line = raw.decode("utf-8", "replace").split("#", 1)[0].strip()
            if not line:
                continue
            parts = line.split("\t")
            if len(parts) != 3 or parts[1] != "kMandarin":
                continue
            reading = parts[2].strip().split()[0] if parts[2].strip() else ""
            if not reading:
                continue
            try:
                char = chr(int(parts[0].removeprefix("U+"), 16))
            except ValueError:
                continue
            out.append(f"{char}\t{reading}")
with open(sys.argv[2], "w", encoding="utf-8") as fh:
    fh.write("\n".join(out) + "\n")
print(f"  {len(out)} Han readings")
PY
}

fetch_one() {
    local id="$1" force="$2" staging
    if [[ "$force" != true ]] && cached_source_matches "$id"; then
        echo "fetch: ${id}: cached copy matches the recorded digest"
        return 0
    fi
    staging="$(mktemp -d)"
    # Guarded and self-disarming: a `RETURN` trap stays installed for later function
    # returns too, so without the guard it fires again in `main`, where `staging` is
    # unset and `set -u` turns the cleanup into a spurious failure.
    trap 'rm -rf "${staging:-}"; trap - RETURN' RETURN

    download "$id" "${staging}/${id}"

    # Normalise first, then digest: sources.toml pins `data/raw/<id>.tsv`, the file the
    # compiler actually consumes, not the intermediate download. Normalisation is
    # deterministic, so this still detects an upstream change — and it additionally
    # catches an accidental change to the normaliser itself.
    local out="${raw_dir}/${id}.tsv" staged="${staging}/${id}.tsv"
    case "$id" in
        pinyin-data) normalise_pinyin_data "${staging}/${id}" "$staged" ;;
        jieba-dict)  normalise_jieba       "${staging}/${id}" "$staged" ;;
        unihan)      normalise_unihan      "${staging}/${id}" "$staged" ;;
    esac

    local actual expected
    actual="$(sha256sum "$staged" | cut -d' ' -f1)"
    expected="$(recorded_sha256 "$id")"
    if [[ -n "$expected" && "$expected" != "$actual" ]]; then
        echo "fetch: ${id}: digest mismatch" >&2
        echo "  expected ${expected}" >&2
        echo "  actual   ${actual}" >&2
        echo "  The upstream artifact changed, or the normaliser did. Re-measure before" >&2
        echo "  accepting it: the dictionary quality numbers were recorded against the" >&2
        echo "  pinned revision." >&2
        return 1
    fi

    mv "$staged" "$out"

    if [[ -z "$expected" ]]; then
        echo "  sha256 = \"${actual}\"   # <- record this in data/sources.toml"
    else
        echo "  digest verified"
    fi
}

main() {
    mkdir -p "$raw_dir"
    local record=false force=false ids=()
    for arg in "$@"; do
        case "$arg" in
            --record) record=true ;;
            --force) force=true ;;
            *) ids+=("$arg") ;;
        esac
    done
    if [[ ${#ids[@]} -eq 0 ]]; then
        ids=(pinyin-data jieba-dict unihan)
    fi
    # `--record` exists to measure what upstream serves today, so it must not accept the
    # cached copy it is about to re-measure.
    if [[ "$record" == true ]]; then
        force=true
    fi

    local failed=0
    for id in "${ids[@]}"; do
        if ! fetch_one "$id" "$force"; then
            failed=1
        fi
    done
    [[ $failed -eq 0 ]] || return 1

    if [[ "$record" == true ]]; then
        echo
        echo "Record the digests printed above in ${sources_toml}."
    fi
}

main "$@"
