#!/usr/bin/env python3
"""Reproduce the dictionary-source measurements of ADR-0000.

The script answers three questions against the whitelisted sources only:

  * coverage   -- how much of the word list the L1 character table can spell,
  * risk       -- how many words contain a character with several readings, so
                  that the baseline reading can be wrong at all,
  * cost       -- how many FST keys the L3b expansion adds, for each CAP and
                  each frequency band, which is the cost half of the ADR's
                  cost/benefit curve.

The benefit half (how many of the wrong readings the expansion recovers) needs a
reference reading source. ADR-0000 excludes luna-pinyin (LGPL-3.0) and CC-CEDICT
(CC BY-SA 4.0) from the repository, so the script takes one with
`--reference FILE` in the same `word<TAB>reading` form the L3c table uses. With
the same reference the ADR's numbers reproduce: convert the reference once with

    python3 -c "import sys; ..." > /tmp/reference.tsv

or any tool that writes `word<TAB>reading` lines.

Usage:

    python3 scripts/dict-probe.py                     # whitelisted sources only
    python3 scripts/dict-probe.py --reference ref.tsv  # plus coverage/error rates
    python3 scripts/dict-probe.py --limit 20000        # quick run

Exit status is non-zero when a measured number drifts from the value ADR-0000
records by more than one percentage point, so the probe can guard the sources.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

# Tone-marked vowels and the umlaut, folded onto the plain letters the syllable
# table is written in. The same table lives in xtask/src/dictc/source.rs; keeping
# the two in step is what makes the probe measure what dictc compiles.
TONE_MARKS = {
    "ā": "a", "á": "a", "ǎ": "a", "à": "a",
    "ē": "e", "é": "e", "ě": "e", "è": "e",
    "ī": "i", "í": "i", "ǐ": "i", "ì": "i",
    "ō": "o", "ó": "o", "ǒ": "o", "ò": "o",
    "ū": "u", "ú": "u", "ǔ": "u", "ù": "u",
    "ǖ": "v", "ǘ": "v", "ǚ": "v", "ǜ": "v", "ü": "v",
    "ń": "n", "ň": "n", "ǹ": "n",
    "ḿ": "m",
    "ế": "ê", "ề": "ê", "ể": "ê", "ễ": "ê", "ệ": "ê",
}

# Numbers a reading may carry as tone digits.
TONE_DIGITS = set("012345")

# Separators inside a reading.
SEPARATORS = set("'-·| ")

# Ranges the dictionary accepts as word characters (Unicode Han blocks).
HAN_RANGES = (
    (0x3400, 0x4DBF),
    (0x4E00, 0x9FFF),
    (0xF900, 0xFAFF),
    (0x20000, 0x2FA1F),
)

# Values ADR-0000 records, used by the drift check. Percentages are compared
# with a one-point tolerance, key counts with a two-percent tolerance.
ADR = {
    "l1_characters": 44435,
    "l1_polyphone_share_pct": 19.4,
    "multi_char_words": 337465,
    "keys_cap1": 337465,
    "keys_cap2_full": 604216,
    "keys_cap4_full": 931602,
    "keys_cap8_full": 1254908,
    "keys_cap16_full": 1521958,
    "growth_cap4_full_pct": 176.0,
    "growth_top50k_cap4_pct": 23.0,
    "growth_top10k_cap4_pct": 4.0,
    "growth_top2k_cap4_pct": 1.0,
    "l3a_error_pct_by_count": 4.3,
    "l3a_error_pct_weighted": 1.9,
    "recovered_top50k_cap4_pct": 58.1,
    "recovered_full_cap4_pct": 60.2,
}

TOLERANCE_POINTS = 1.0
TOLERANCE_COUNTS_PCT = 2.0


def fold(raw: str) -> str | None:
    """Folds one source reading onto its toneless, table-spelled form."""
    out = []
    for char in raw:
        if char in TONE_MARKS:
            out.append(TONE_MARKS[char])
        elif char in TONE_DIGITS or char in SEPARATORS:
            continue
        elif char == "v" or char == "V":
            out.append("v")
        elif char.isascii() and char.isalpha():
            out.append(char.lower())
        elif char == "ê":
            out.append("ê")
        else:
            return None
    text = "".join(out)
    # Standard orthography drops the umlaut after j, q, x and y.
    result = []
    for char in text:
        if char == "v" and result and result[-1] in "jqxy":
            result.append("u")
        else:
            result.append(char)
    return "".join(result) or None


def load_syllables(root: Path) -> set[str]:
    """Reads the 411-syllable table out of the Rust source that owns it."""
    source = root / "crates/ime-core/src/segment/syllable.rs"
    text = source.read_text(encoding="utf-8")
    start = text.index("pub static SYLLABLES")
    start = text.index("&[", start)
    end = text.index("];", start)
    table = set(re.findall(r'"([^"]+)"', text[start:end]))
    if len(table) < 400:
        raise SystemExit(f"{source}: parsed only {len(table)} syllables")
    return table


def segment(text: str, count: int, syllables: set[str], max_len: int = 6):
    """Cuts `text` into exactly `count` syllables, longest syllable first.

    Returns the syllable list, or None when no cut with that many syllables
    exists -- which is what makes a reading with the wrong syllable count
    detectable.
    """
    if count <= 0 or count > len(text):
        return None
    length = len(text)
    reachable = [[None] * (count + 1) for _ in range(length + 1)]
    reachable[0][0] = 0
    for offset in range(length):
        for used in range(count):
            if reachable[offset][used] is None:
                continue
            size = min(max_len, length - offset)
            while size >= 1:
                candidate = text[offset:offset + size]
                if candidate in syllables and reachable[offset + size][used + 1] is None:
                    reachable[offset + size][used + 1] = offset
                size -= 1
    if reachable[length][count] is None:
        return None
    out = []
    offset, used = length, count
    while used > 0:
        previous = reachable[offset][used]
        if previous is None:
            return None
        out.append(text[previous:offset])
        offset, used = previous, used - 1
    out.reverse()
    return out


def load_l1(path: Path, syllables: set[str]):
    """Loads the L1 character table, keeping the source order of the readings.

    Returns the table and the raw statistics ADR-0000 records: the number of
    characters and the share of them whose source row lists more than one
    reading. The table itself deduplicates readings that fold onto the same
    toneless syllable, because two spellings of one syllable are one key.
    """
    table: dict[str, list[str]] = {}
    raw_characters = 0
    raw_polyphone = 0
    illegal = 0
    for line in path.read_text(encoding="utf-8").splitlines():
        columns = line.split("\t")
        if len(columns) < 2 or not columns[0]:
            continue
        character = columns[0][0]
        raw_characters += 1
        raw = [reading for reading in columns[1].split(",") if reading.strip()]
        if len(raw) > 1:
            raw_polyphone += 1
        readings = table.setdefault(character, [])
        for reading in raw:
            folded = fold(reading)
            if folded is None or folded not in syllables:
                illegal += 1
                continue
            if folded not in readings:
                readings.append(folded)
    share = 100.0 * raw_polyphone / max(1, raw_characters)
    print(f"L1: {raw_characters} characters, {raw_polyphone} with several readings "
          f"({share:.1f}%), {illegal} readings outside the table")
    return table, {
        "l1_characters": float(raw_characters),
        "l1_polyphone_share_pct": share,
    }


def load_frequencies(path: Path) -> list[tuple[str, int]]:
    """Loads `word<TAB>frequency` rows, keeping the heaviest row per word."""
    best: dict[str, int] = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        columns = line.rsplit("\t", 1)
        if len(columns) != 2:
            continue
        word = columns[0].strip()
        try:
            frequency = int(columns[1])
        except ValueError:
            continue
        if not word:
            continue
        if frequency > best.get(word, -1):
            best[word] = frequency
    return sorted(best.items(), key=lambda item: -item[1])


def is_han(text: str) -> bool:
    """True when every character of `text` is a Han character."""
    for char in text:
        point = ord(char)
        if not any(low <= point <= high for low, high in HAN_RANGES):
            return False
    return bool(text)


def readings_of(word: str, l1: dict[str, list[str]]):
    """Every reading of every character, or None when one is uncovered."""
    per_character = []
    for character in word:
        readings = l1.get(character)
        if not readings:
            return None
        per_character.append(readings)
    return per_character


def baseline_key(per_character) -> str:
    """The L3a key: the first reading of each character."""
    return "'".join(readings[0] for readings in per_character)


def expand(per_character, cap: int) -> list[str]:
    """The cartesian product of the readings, most likely combination first."""
    if cap <= 0:
        return []
    combinations = [(0, ())]
    for readings in per_character:
        extended = []
        for rank, indices in combinations:
            for index in range(len(readings)):
                extended.append((rank + index, indices + (index,)))
        extended.sort(key=lambda item: (item[0], item[1]))
        combinations = extended[:cap]
    keys = []
    for _, indices in combinations:
        keys.append("'".join(per_character[position][index]
                             for position, index in enumerate(indices)))
    return keys


def reference_reading(raw: str, syllables: set[str], count: int):
    """Turns one reference reading into a syllable list, or None."""
    pieces = [piece for piece in re.split(r"['\-·|\s]+", raw) if piece]
    if len(pieces) == count:
        folded = [fold(piece) for piece in pieces]
        if all(piece in syllables for piece in folded if piece):
            return folded
        return None
    folded = fold(raw)
    if folded is None:
        return None
    return segment(folded, count, syllables)


def load_reference(path: Path, syllables: set[str]) -> dict[str, str]:
    """Loads `word<TAB>reading` rows as canonical keys."""
    rows: dict[str, str] = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        columns = line.split("\t")
        if len(columns) < 2:
            continue
        word = columns[0].strip()
        syllables_list = reference_reading(columns[1], syllables, len(word))
        if syllables_list is None:
            continue
        rows[word] = "'".join(syllables_list)
    return rows


def measure(frequencies, l1, syllables, bands, caps, reference, limit, l1_stats):
    """Runs the coverage, risk and cost measurements."""
    multi_char = [(word, freq) for word, freq in frequencies if len(word) > 1]
    if limit:
        multi_char = multi_char[:limit]
    total_weight = sum(freq for _, freq in multi_char)

    covered = []
    uncovered = 0
    for word, frequency in multi_char:
        if not is_han(word):
            continue
        per_character = readings_of(word, l1)
        if per_character is None:
            uncovered += 1
            continue
        covered.append((word, frequency, per_character))

    covered_weight = sum(frequency for _, frequency, _ in covered)
    polyphone_words = sum(
        1 for _, _, per_character in covered
        if any(len(readings) > 1 for readings in per_character)
    )
    polyphone_weight = sum(
        frequency for _, frequency, per_character in covered
        if any(len(readings) > 1 for readings in per_character)
    )

    print()
    print(f"words: {len(frequencies)} total, {len(multi_char)} multi-character")
    print(f"L1 coverage: {len(covered)}/{len(multi_char)} multi-character words "
          f"({100.0 * len(covered) / max(1, len(multi_char)):.2f}% of words, "
          f"{100.0 * covered_weight / max(1, total_weight):.2f}% of frequency mass), "
          f"{uncovered} words have an uncovered character")
    print(f"polyphone risk: {polyphone_words} words carry a character with several "
          f"readings ({100.0 * polyphone_words / max(1, len(covered)):.2f}% of words, "
          f"{100.0 * polyphone_weight / max(1, covered_weight):.2f}% weighted)")

    keys = {}
    for cap in caps:
        keys[cap] = sum(len(expand(per_character, cap)) for _, _, per_character in covered)
    base = len(covered)
    print()
    print("cost, full expansion:")
    for cap in caps:
        growth = 100.0 * (keys[cap] - base) / max(1, base)
        print(f"  CAP={cap:<2} keys={keys[cap]:>9} growth={growth:+7.2f}%")

    ranked = sorted(covered, key=lambda item: -item[1])
    print()
    print("cost, frequency-targeted expansion (CAP=4):")
    for band in bands:
        selection = ranked[:band] if band else ranked
        extra = sum(len(expand(per_character, 4)) - 1 for _, _, per_character in selection)
        growth = 100.0 * extra / max(1, base)
        label = f"top {band}" if band else "all"
        print(f"  {label:<10} added keys={extra:>8} growth={growth:+7.2f}%")
        keys.setdefault(("band", band), extra)

    benefit = {}
    if reference:
        compared = 0
        wrong = 0
        wrong_weight = 0
        recovered_full = 0
        recovered_band = 0
        wrong_weight_full = 0
        wrong_weight_band = 0
        rank_of = {word: position for position, (word, _, _) in enumerate(ranked)}
        for word, frequency, per_character in covered:
            expected = reference.get(word)
            if expected is None:
                continue
            compared += 1
            actual = baseline_key(per_character)
            if actual == expected:
                continue
            wrong += 1
            wrong_weight += frequency
            in_band = rank_of.get(word, len(ranked)) < 50000
            if expected in expand(per_character, 4):
                recovered_full += 1
                wrong_weight_full += frequency
                if in_band:
                    recovered_band += 1
                    wrong_weight_band += frequency
        print()
        print(f"reference: {len(reference)} words, {compared} comparable")
        print(f"L3a error: {wrong}/{compared} words "
              f"({100.0 * wrong / max(1, compared):.2f}% by count, "
              f"{100.0 * wrong_weight / max(1, covered_weight):.2f}% weighted)")
        print(f"L3b recovery, full CAP=4: {100.0 * recovered_full / max(1, wrong):.2f}% by count, "
              f"{100.0 * wrong_weight_full / max(1, wrong_weight):.2f}% weighted")
        print(f"L3b recovery, top-50k CAP=4: {100.0 * recovered_band / max(1, wrong):.2f}% by count, "
              f"{100.0 * wrong_weight_band / max(1, wrong_weight):.2f}% weighted")
        benefit = {
            "l3a_error_pct_by_count": 100.0 * wrong / max(1, compared),
            "l3a_error_pct_weighted": 100.0 * wrong_weight / max(1, covered_weight),
            "recovered_top50k_cap4_pct": 100.0 * wrong_weight_band / max(1, wrong_weight),
            "recovered_full_cap4_pct": 100.0 * wrong_weight_full / max(1, wrong_weight),
        }

    measured = dict(l1_stats)
    measured.update({
        "multi_char_words": len(multi_char),
        "keys_cap1": keys.get(1, 0),
        "keys_cap2_full": keys.get(2, 0),
        "keys_cap4_full": keys.get(4, 0),
        "keys_cap8_full": keys.get(8, 0),
        "keys_cap16_full": keys.get(16, 0),
        "growth_cap4_full_pct": 100.0 * (keys.get(4, 0) - base) / max(1, base),
        "growth_top50k_cap4_pct": 100.0 * keys.get(("band", 50000), 0) / max(1, base),
        "growth_top10k_cap4_pct": 100.0 * keys.get(("band", 10000), 0) / max(1, base),
        "growth_top2k_cap4_pct": 100.0 * keys.get(("band", 2000), 0) / max(1, base),
    })
    measured.update(benefit)
    return measured


def check(measured) -> int:
    """Compares the measurements with ADR-0000 and reports the drift."""
    print()
    print("drift against ADR-0000 (one point of tolerance):")
    failures = 0
    for key, recorded in ADR.items():
        if key not in measured:
            continue
        actual = measured[key]
        if key.startswith("keys_"):
            tolerance = TOLERANCE_COUNTS_PCT * recorded / 100.0
        elif key == "multi_char_words" or key == "l1_characters":
            tolerance = TOLERANCE_COUNTS_PCT * recorded / 100.0
        else:
            tolerance = TOLERANCE_POINTS
        delta = actual - recorded
        status = "ok" if abs(delta) <= tolerance else "DRIFT"
        if status == "DRIFT":
            failures += 1
        print(f"  {key:<34} adr={recorded:>10.1f} measured={actual:>10.1f} "
              f"delta={delta:+8.2f} {status}")
    return failures


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parent.parent,
                        help="repository root (default: the parent of scripts/)")
    parser.add_argument("--l1", default="data/raw/pinyin-data.tsv")
    parser.add_argument("--l4", default="data/raw/jieba-dict.tsv")
    parser.add_argument("--reference", type=Path,
                        help="word<TAB>reading file used for the error-rate measurements")
    parser.add_argument("--limit", type=int, default=0,
                        help="measure only the first N multi-character words")
    parser.add_argument("--no-check", action="store_true",
                        help="print the measurements without comparing them to ADR-0000")
    args = parser.parse_args()

    syllables = load_syllables(args.root)
    print(f"syllable table: {len(syllables)} entries")
    l1, l1_stats = load_l1(args.root / args.l1, syllables)
    frequencies = load_frequencies(args.root / args.l4)
    reference = load_reference(args.reference, syllables) if args.reference else None
    if args.reference:
        print(f"reference: {len(reference)} readings from {args.reference}")

    measured = measure(frequencies, l1, syllables, [2000, 10000, 50000, 0],
                       [1, 2, 4, 8, 16], reference, args.limit, l1_stats)
    if args.no_check:
        return 0
    return 1 if check(measured) else 0


if __name__ == "__main__":
    sys.exit(main())
