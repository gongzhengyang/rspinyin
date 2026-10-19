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
        # One THUOCL file per domain; the domain set the normaliser merges lives in
        # download_source, and the two must not drift apart.
        thuocl:*)    echo "https://raw.githubusercontent.com/thunlp/THUOCL/master/data/THUOCL_${1#thuocl:}.txt" ;;
        # The huayu attachment endpoint. The token that completes it is scraped from
        # the thread page on every fetch; see download_huayu.
        huayu)       echo "http://bbs.pinyin.thunisoft.com/forum.php?mod=attachment&aid=" ;;
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

# The thread that publishes the huayu word list, and the page the attachment
# token is scraped from.
readonly HUAYU_THREAD="http://bbs.pinyin.thunisoft.com/forum.php?mod=viewthread&tid=30049"

# The attachment token of the first download link on the thread page.
#
# The page is GBK-encoded Discuz HTML, where the link itself is plain ASCII; the
# separator may arrive raw or as its `&amp;` entity, and the token is URL-encoded
# base64 that must be passed on exactly as written.
attachment_aid() {
    python3 - "$1" <<'PY'
import re, sys
html = open(sys.argv[1], "rb").read().decode("gbk", "replace")
matches = re.findall(r"mod=attachment&(?:amp;)?aid=([A-Za-z0-9%+=]+)", html)
if matches:
    print(matches[0])
PY
}

# The huayu word list is a forum attachment, and Discuz reissues the attachment
# token per session: a stale or foreign token is answered with an error page
# instead of the archive. The token is therefore scraped fresh on every attempt,
# while the attachment itself has not changed since 2015 -- a new token is
# invisible to the pin, because the digest covers the decoded word list, never
# the page.
download_huayu() {
    local staging="$1" page aid zip_path attempt uwl_count
    page="${staging}/thread.html"
    zip_path="${staging}/huayu.zip"
    for attempt in $(seq 1 10); do
        curl --fail --location --silent --show-error --output "$page" "$HUAYU_THREAD" || true
        aid="$(attachment_aid "$page")"
        if [[ -z "$aid" ]]; then
            echo "  attempt ${attempt}: no attachment token on the thread page"
            continue
        fi
        curl --fail --location --silent --show-error \
             --output "$zip_path" "$(url_for huayu)${aid}" || true
        if [[ -s "$zip_path" ]] && unzip -t "$zip_path" >/dev/null 2>&1; then
            uwl_count="$(unzip -Z1 "$zip_path" | grep -c '\.uwl$' || true)"
            if [[ "$uwl_count" == 1 ]]; then
                unzip -p "$zip_path" '*.uwl' > "${staging}/huayu.uwl"
                return 0
            fi
            echo "  attempt ${attempt}: archive holds ${uwl_count} .uwl files, expected 1"
        fi
    done
    echo "fetch: huayu: attachment download did not succeed" >&2
    return 1
}

# Download every upstream artifact a source needs into `staging`. Most sources
# are a single file named after the source; THUOCL publishes one file per domain
# and the normaliser merges them -- this domain list and the normaliser's must
# not drift apart -- and the huayu attachment needs the token scrape above.
download_source() {
    local id="$1" staging="$2" domain
    case "$id" in
        thuocl)
            for domain in animal caijing car chengyu diming food IT law lishimingren medical poem; do
                download "thuocl:${domain}" "${staging}/thuocl.${domain}" || return 1
            done
            ;;
        huayu) download_huayu "$staging" ;;
        *)     download "$id" "${staging}/${id}" ;;
    esac
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

# THUOCL domain files: `词<TAB>词频` rows. Three defects make line-by-line
# parsing too optimistic: the place-name file is a single tab-separated line
# with a UTF-8 BOM, some animal names carry ASCII spaces inside the word, and
# scattered rows have a damaged (`3?`, `125472s`) or missing frequency. Fields
# are therefore walked pairwise within each line, inner spaces are removed from
# words, the first occurrence of a word in a fixed file order wins, and rows
# that still do not parse are dropped -- all deterministic, so the digest pins
# the merge. The weight column is the raw corpus count; banding is the
# compiler's business, not the fetch's.
normalise_thuocl() {
    python3 - "$1" "$2" <<'PY'
import sys
from pathlib import Path

DOMAINS = ["animal", "caijing", "car", "chengyu", "diming", "food",
           "IT", "law", "lishimingren", "medical", "poem"]

staging, out = Path(sys.argv[1]), sys.argv[2]
rows = {}
for domain in DOMAINS:
    text = (staging / f"thuocl.{domain}").read_text(encoding="utf-8-sig")
    for line in text.splitlines():
        fields = line.split("\t")
        for index in range(0, len(fields) - 1, 2):
            word = fields[index].strip().replace(" ", "")
            freq = fields[index + 1].strip()
            if not word or not freq.isdigit():
                continue
            rows.setdefault(word, freq)
with open(out, "w", encoding="utf-8") as fh:
    for word, freq in rows.items():
        fh.write(f"{word}\t{freq}\n")
print(f"  {len(rows)} domain words from {len(DOMAINS)} THUOCL files")
PY
}

# 野风系统词库.uwl: the huayu binary word list. The container layout follows the
# published format analysis, validated against this very file: a 0xC00-byte
# header (entry count at 0x44, section count at 0x48), then 1024-byte sections.
# A section starts with a 16-byte header whose last word counts the bytes in
# use; the body packs entries of a 4-byte head, 2-byte syllable codes and
# UTF-16LE word text. The head's low 7 bits minus one is the word's byte length
# and its top bit is the odd-syllable carry; the second byte's low nibble halves
# into the syllable count. Each 16-bit code splits into a 5-bit initial index
# and an 11-bit final index over the two fixed tables below. Final index 0 never
# occurs in the file and stays an empty sentinel, so a future appearance fails
# loudly downstream instead of decoding as a plausible syllable; `l`/`n` plus
# the `v`/`ue` finals spell with `ü`, the canonical spelling the compiler's
# syllable table stores. A section whose body stops parsing consistently is
# abandoned whole, which is stable, and stable is what the digest needs.
normalise_huayu() {
    python3 - "$1" "$2" <<'PY'
import sys

INITIALS = ["", "b", "c", "ch", "d", "f", "g", "h", "j", "k", "l", "m", "n",
            "p", "q", "r", "s", "sh", "t", "w", "x", "y", "z", "zh"]
FINALS = ["", "a", "ai", "an", "ang", "ao", "e", "ei", "en", "eng", "er", "i",
          "ia", "ian", "iang", "iao", "ie", "in", "ing", "iong", "iu", "o",
          "ong", "ou", "u", "ua", "uai", "uan", "uang", "ue", "ui", "un",
          "uo", "v"]

def syllable(initial, final):
    if initial in ("l", "n") and final == "v":
        return initial + "ü"
    if initial in ("l", "n") and final == "ue":
        return initial + "üe"
    return initial + final

data = open(sys.argv[1], "rb").read()
sections = int.from_bytes(data[0x48:0x4C], "little")
rows = []
abandoned = 0
for section in range(sections):
    base = 0xC00 + section * 0x400
    used = int.from_bytes(data[base + 12:base + 16], "little")
    if used == 0 or used > 0x400 - 16:
        continue
    pos = base + 16
    end = pos + used
    while pos < end and pos + 4 <= end:
        head_a = data[pos]
        head_b = data[pos + 1]
        freq = int.from_bytes(data[pos + 2:pos + 4], "little")
        word_bytes = (head_a & 0x7F) - 1
        syllables = (head_b & 0x0F) * 2 + (head_a >> 7)
        pos += 4
        if word_bytes <= 0 or syllables <= 0 or syllables > 12 \
                or pos + syllables * 2 + word_bytes > end:
            abandoned += 1
            break
        reading = []
        for _ in range(syllables):
            low, high = data[pos], data[pos + 1]
            pos += 2
            initial = INITIALS[low & 0x1F]
            final = FINALS[(low >> 5) + (high << 3)]
            reading.append(syllable(initial, final))
        word = data[pos:pos + word_bytes].decode("utf-16-le", "replace")
        pos += word_bytes
        rows.append(f"{word}\t{' '.join(reading)}\t{freq}")
with open(sys.argv[2], "w", encoding="utf-8") as fh:
    fh.write("\n".join(rows) + "\n")
print(f"  {len(rows)} words, {abandoned} sections abandoned on a bad entry")
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

    download_source "$id" "$staging"

    # Normalise first, then digest: sources.toml pins `data/raw/<id>.tsv`, the file the
    # compiler actually consumes, not the intermediate download. Normalisation is
    # deterministic, so this still detects an upstream change — and it additionally
    # catches an accidental change to the normaliser itself.
    local out="${raw_dir}/${id}.tsv" staged="${staging}/${id}.tsv"
    case "$id" in
        pinyin-data) normalise_pinyin_data "${staging}/${id}" "$staged" ;;
        jieba-dict)  normalise_jieba       "${staging}/${id}" "$staged" ;;
        unihan)      normalise_unihan      "${staging}/${id}" "$staged" ;;
        thuocl)      normalise_thuocl      "$staging" "$staged" ;;
        huayu)       normalise_huayu       "${staging}/huayu.uwl" "$staged" ;;
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
        ids=(pinyin-data jieba-dict unihan thuocl huayu)
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
