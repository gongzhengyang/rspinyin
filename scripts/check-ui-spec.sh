#!/usr/bin/env bash
#
# check-ui-spec.sh - assert that the candidate window's specification tables and
# the files that draw from them declare and reference the same tokens.
#
# features.md 3.1.1 (geometry), 3.1.4 (the 4dp grid, its exception table and the
# type scale) and 3.2 (colour tokens) are the authoritative tables. The window
# draws from `ui/candidate.slint` and `ui/theme.slint`, and `src/theme.rs`
# repeats the palette because the contrast self-check has to evaluate it without
# a renderer. Three copies of one table drift apart unless something compares
# them, and the comparison cannot go through Slint -- building a global needs a
# window and a display server, which a gate must not require -- so the files are
# read as text and the colour expressions are evaluated with the conversions
# Slint itself applies: the `rgba()` builtin truncates a fraction,
# `with-alpha()` rounds one. That is the same rule `src/theme/slint_palette.rs`
# asserts, so this gate and that unit test cannot disagree.
#
# What is asserted:
#
#   1. every size row of 3.1.1 carries the value `CandidateMetrics` declares,
#      and every constant `src/layout/metrics.rs` reads is declared there;
#   2. every colour row of 3.2 carries the bytes `theme.slint` computes and the
#      bytes `theme.rs` holds, for both schemes;
#   3. every `length` constant sits on the 4dp grid of 3.1.4 unless that
#      section's exception table lists it -- the table is parsed, never restated
#      here -- and every font size is on 3.1.4's type scale;
#   4. no token and no constant is dead. A name that neither a `.slint` source
#      draws with nor a Rust module computes with has to be listed as deferred
#      below, together with the specification that still owes its wiring, so the
#      list can only shrink: a deferred name that has become live fails the gate
#      until its entry is dropped.
#
# Exit codes: 0 = pass, 1 = violation, 2 = usage or environment error.
#
# Self-test: `--self-test` copies the analysed files into a scratch tree and
# injects one violation per assertion class -- a drifted size, a drifted colour
# on each side of the comparison, a 4dp violation, a removed grid exception, a
# size off the type scale, a dead token and a deferral that has become live --
# asserting a non-zero exit for each and a zero exit for the untouched copy.
#
# The comparison lives in the python3 program embedded below rather than in a
# second file, the way every other audit script in this directory does it: a
# gate that is one file cannot be half-installed.
#
# Requires python3 (present by default on every distro in the platform baseline).

set -euo pipefail

usage() {
    cat <<'USAGE'
usage: scripts/check-ui-spec.sh [--self-test] [--root DIR]

  --self-test   inject one violation per assertion class into a scratch copy of
                the analysed files and assert each one is reported
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
            echo "check-ui-spec: unknown argument: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

if ! command -v python3 >/dev/null 2>&1; then
    echo "check-ui-spec: python3 is required to read the spec tables; install it and re-run" >&2
    exit 2
fi

# analyse ROOT: compare the specification tables with the declarations.
analyse() {
    python3 - "$1" <<'PY'
import math
import re
import sys
from collections import namedtuple
from pathlib import Path

ROOT = Path(sys.argv[1])

FEATURES = "docs/dev/features.md"
THEME_SLINT = "crates/ime-ui/ui/theme.slint"
CANDIDATE_SLINT = "crates/ime-ui/ui/candidate.slint"
THEME_RS = "crates/ime-ui/src/theme.rs"
SCHEME_RS = "crates/ime-ui/src/theme/scheme.rs"
METRICS_RS = "crates/ime-ui/src/layout/metrics.rs"
UI_DIR = "crates/ime-ui/ui"
SRC_DIR = "crates/ime-ui/src"

SECTION_SIZES = "#### 3.1.1 候选框整体几何"
SECTION_GRID = "#### 3.1.4 4dp 网格与字阶"
SECTION_COLOURS = "### 3.2 色彩 Token 与深浅色"

# The reference-closure budget: a name that is dead *and* unlisted may not
# exceed this. A listed name is one the specification itself still owes, which
# is why the budget is not spent on it.
TOKEN_BUDGET = 3
CONSTANT_BUDGET = 2

# Colour tokens nothing draws yet. Each entry names the specification that owns
# the wiring, so the entry can be judged rather than trusted; the gate fails
# when a name becomes live, which keeps the list from outliving its reason.
DEFERRED_TOKENS = {
    "accent-on": "3.2 marks it reserved: text on a solid accent fill, which no surface draws yet",
    "text-separator": "the header draws one string, so the span-level separator of 3.1.1 has no host",
}

# Constants of `CandidateMetrics` that neither the window nor a Rust module
# reads yet, with the specification that owns them. Empty: the candidate grid
# draws the cell interior of 3.1.1, so every constant is live.
DEFERRED_CONSTANTS = {}

# The 3.1.1 rows, and the `CandidateMetrics` constants each one fixes. The unit
# is the one the row spells the value in, and the index is the position of that
# value among the values of the same unit in the row, counted from zero.
SIZE_ROWS = [
    ("窗口外边距（阴影预留）", [("shadow-margin", "dp", 0)]),
    ("容器圆角", [("container-radius", "dp", 0)]),
    ("容器描边", [("stroke-width", "dp", 0)]),
    ("容器内边距", [("container-padding", "dp", 0)]),
    ("候选框最小宽度", [("min-width", "dp", 0)]),
    ("候选框最大宽度", [("max-width", "dp", 0)]),
    ("Header 高度", [("header-height", "dp", 0), ("header-height-compact", "dp", 1)]),
    ("Header 水平内边距", [("header-padding-h", "dp", 0)]),
    ("Header 字号/字重", [("font-size-header", "sp", 0), ("font-size-small", "sp", 2)]),
    (
        "Header 状态图标",
        [
            ("header-icon-size", "dp", 0),
            ("header-icon-gap", "dp", 2),
            ("header-text-gap", "dp", 3),
        ],
    ),
    ("Header 分隔线", [("separator-height", "dp", 0)]),
    ("候选区内边距", [("container-padding", "dp", 0)]),
    ("候选单元最小宽度", [("cell-min-width", "dp", 0)]),
    ("候选单元高度", [("cell-height", "dp", 0)]),
    ("候选单元内边距", [("cell-padding-h", "dp", 0), ("cell-padding-v", "dp", 1)]),
    ("候选单元圆角", [("cell-radius", "dp", 0)]),
    ("候选序号字号", [("font-size-small", "sp", 0), ("number-gap", "dp", 0)]),
    ("候选文本字号/字重", [("font-size-cell", "sp", 0)]),
    ("候选注音字号", [("font-size-small", "sp", 0), ("annotation-gap", "dp", 0)]),
    ("网格列间距", [("grid-gap", "dp", 0)]),
    ("网格行间距", [("grid-gap", "dp", 0)]),
    (
        "单行最大候选数",
        [
            ("max-per-row", "count", 0),
            ("min-per-row", "count", 1),
            ("max-per-row-limit", "count", 2),
        ],
    ),
    ("光标指示箭头", [("cursor-arrow-height", "dp", 0), ("cursor-arrow-width", "dp", 1)]),
]

# The 3.2 rows: the table token, the property `theme.slint` declares it as, the
# field `src/theme.rs` holds it in, and how the row derives its value. The
# mapping is by name only, never by value, so a rename on either side is a
# failure rather than a silent miss.
COLOUR_ROWS = [
    ("surface.base", "surface-base", "surface_base", "base"),
    ("surface.stroke", "surface-stroke", "surface_stroke", "colour"),
    ("text.primary", "text-primary", "text_primary", "colour"),
    ("text.secondary", "text-secondary", "text_secondary", "colour"),
    ("text.annotation", "text-annotation", "text_annotation", "colour"),
    ("text.separator", "text-separator", "text_separator", "colour"),
    ("accent.default", "accent", None, "accent"),
    ("accent.on", "accent-on", "accent_on", "colour"),
    ("state.hover", "state-hover", "state_hover", "colour"),
    ("state.selected.bg", "state-selected-bg", "selected_bg_alpha", "accent-alpha"),
    ("state.selected.stroke", "state-selected-stroke", "selected_stroke_alpha", "accent-alpha"),
    ("state.pressed", "state-pressed", "state_pressed", "colour"),
    ("separator", "separator", "separator", "colour"),
    ("shadow.inner", "shadow-inner", "shadow_inner", "colour"),
    ("shadow.outer", "shadow-outer", "shadow_outer", "colour"),
    ("status.dot.active", "status-dot-active", None, "accent-derived"),
    ("status.dot.idle", "status-dot-idle", "status_dot_idle", "colour"),
]

# Every colour property `theme.slint` declares: the 3.2 table's seventeen rows
# plus `surface-fill`, the base at `base-alpha`, which is what the panel is
# filled with.
EXPECTED_COLOUR_PROPERTIES = [
    "surface-base",
    "surface-fill",
    "surface-stroke",
    "text-primary",
    "text-secondary",
    "text-annotation",
    "text-separator",
    "accent",
    "accent-on",
    "state-hover",
    "state-selected-bg",
    "state-selected-stroke",
    "state-pressed",
    "separator",
    "shadow-inner",
    "shadow-outer",
    "status-dot-active",
    "status-dot-idle",
]

# The three values the Rust side writes, in the order the file declares them.
# A fourth input would be a fourth write, and the claim that a theme switch is a
# property update rather than a rebuild would stop holding.
EXPECTED_INPUTS = ["dark", "accent", "base-alpha"]

problems = []


class Unparsed(Exception):
    """A declaration the analyser does not know how to read."""


def die(message):
    print(f"check-ui-spec: {message}", file=sys.stderr)
    sys.exit(2)


def fail(message):
    problems.append(message)


def format_number(value):
    if value == int(value):
        return str(int(value))
    return repr(value)


def read(relative):
    path = ROOT / relative
    try:
        return path.read_text(encoding="utf-8")
    except OSError as error:
        die(f"cannot read {path}: {error}")


# --------------------------------------------------------------------------
# features.md
# --------------------------------------------------------------------------

HEADING = re.compile(r"^#{2,4} ")


def section(text, opening):
    """The body of one features.md section, up to the next heading."""
    lines = text.splitlines()
    start = None
    for index, line in enumerate(lines):
        if line.startswith(opening):
            start = index
            break
    if start is None:
        die(f"{FEATURES} no longer has the section {opening!r}")
    body = []
    for line in lines[start + 1 :]:
        if HEADING.match(line):
            break
        body.append(line)
    return "\n".join(body)


def table_rows(block):
    """The data rows of every markdown table in a section.

    The header row of each table is dropped: it names the columns rather than
    listing a value, and it is the row the separator line follows.
    """
    lines = [line.strip() for line in block.splitlines() if line.strip().startswith("|")]
    rows = []
    for index, line in enumerate(lines):
        cells = [cell.strip() for cell in line.strip("|").split("|")]
        if is_separator(cells):
            continue
        following = lines[index + 1] if index + 1 < len(lines) else ""
        if following and is_separator(following.strip("|").split("|")):
            continue
        rows.append(cells)
    return rows


def is_separator(cells):
    return bool(cells) and all(re.fullmatch(r":?-{2,}:?", cell.strip()) for cell in cells)


def unquote(cell):
    return cell.strip().strip("`").strip()


def cell_numbers(cell, unit):
    """Every `<number><unit>` a cell spells, in the order it spells them."""
    return [float(value) for value in re.findall(r"(\d+(?:\.\d+)?)\s*" + unit, cell)]


def cell_counts(cell):
    """Every integer a cell spells, in the order it spells them."""
    return [float(value) for value in re.findall(r"\d+", cell)]


# --------------------------------------------------------------------------
# Slint conversions
# --------------------------------------------------------------------------


def truncate_alpha(fraction):
    """The byte Slint's `rgba()` builtin writes: `(255 * a) as u8`, truncated."""
    return max(0, min(int(255.0 * fraction), 255))


def round_alpha(fraction):
    """The byte Slint's `with-alpha()` writes: `(a * 255) as u8` after rounding."""
    return max(0, min(math.floor(fraction * 255.0 + 0.5), 255))


# --------------------------------------------------------------------------
# The .slint sources
# --------------------------------------------------------------------------

DECLARATION = re.compile(
    r"^(in-out|in|out)\s+property\s+<([^>]+)>\s+([A-Za-z0-9_-]+)\s*:\s*(.+);$"
)
Declaration = namedtuple("Declaration", "name direction type expression")

CONDITIONAL = re.compile(r"dark\s*\?\s*(.+?)\s*:\s*(.+)")
RGBA_CALL = re.compile(r"rgba\(\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*,\s*([0-9.]+)\s*\)")
WITH_ALPHA = re.compile(r"([A-Za-z0-9_-]+)\.with-alpha\(\s*(.+?)\s*\)")
HEX = re.compile(r"^#([0-9A-Fa-f]{6})$")


def code_only(line):
    return line.split("//")[0].rstrip()


def global_block(text, opening, relative):
    """The lines of one `global` declaration, as (line number, line) pairs."""
    lines = text.splitlines()
    start = None
    for index, line in enumerate(lines):
        if line.strip().startswith(opening):
            start = index
            break
    if start is None:
        die(f"{relative} no longer declares {opening!r}")
    body = []
    for number, line in enumerate(lines[start + 1 :], start=start + 2):
        if line.strip() == "}":
            return body
        body.append((number, line))
    die(f"{relative}: {opening!r} is never closed")


def declarations(block, relative):
    """The scalar properties of a `global`, keyed by name, in file order."""
    found = {}
    for number, raw in block:
        line = code_only(raw).strip()
        if not line:
            continue
        match = DECLARATION.fullmatch(line)
        if match is None:
            if "property" in line:
                die(f"{relative}:{number}: cannot read {line!r}")
            continue
        direction, type_name, name, expression = match.groups()
        if name in found:
            die(f"{relative}:{number}: {name} is declared twice")
        found[name] = Declaration(name, direction, type_name, expression.strip())
    return found


def hex_bytes(digits):
    return (int(digits[0:2], 16), int(digits[2:4], 16), int(digits[4:6], 16), 255)


def parse_table_colour(cell):
    """The bytes a 3.2 cell specifies, and the fraction it spells beside them.

    A cell may be a hex literal, the `rgba()` builtin, `accent @ <alpha>`, or
    `accent.default`. The second element is the alpha fraction the cell spells,
    or None when it spells none.
    """
    text = unquote(cell)
    match = re.fullmatch(r"#([0-9A-Fa-f]{6})\s*@\s*([0-9.]+)", text)
    if match:
        return hex_bytes(match.group(1)), float(match.group(2))
    match = HEX.fullmatch(text)
    if match:
        return hex_bytes(match.group(1)), None
    match = RGBA_CALL.fullmatch(text)
    if match:
        red, green, blue, alpha = match.groups()
        return (int(red), int(green), int(blue), truncate_alpha(float(alpha))), float(alpha)
    match = re.fullmatch(r"accent\s*@\s*([0-9.]+)", text)
    if match:
        return None, float(match.group(1))
    if text == "accent.default":
        return None, None
    raise Unparsed(f"{FEATURES} 3.2 spells a colour as {text!r}")


def evaluate_colour(expression, properties, dark, accent, base_alpha, depth=0):
    """A colour expression, evaluated the way the Slint runtime would."""
    text = expression.strip()
    if depth > 8:
        raise Unparsed(f"expression nests too deeply: {expression!r}")
    conditional = CONDITIONAL.fullmatch(text)
    if conditional:
        chosen = conditional.group(1) if dark else conditional.group(2)
        return evaluate_colour(chosen, properties, dark, accent, base_alpha, depth + 1)
    match = HEX.fullmatch(text)
    if match:
        return hex_bytes(match.group(1))
    match = RGBA_CALL.fullmatch(text)
    if match:
        red, green, blue, alpha = match.groups()
        return (int(red), int(green), int(blue), truncate_alpha(float(alpha)))
    match = WITH_ALPHA.fullmatch(text)
    if match:
        base = evaluate_colour(match.group(1), properties, dark, accent, base_alpha, depth + 1)
        fraction = evaluate_alpha(match.group(2), properties, dark, base_alpha, depth + 1)
        return (base[0], base[1], base[2], round_alpha(fraction))
    if text == "accent":
        return accent
    if text in properties:
        return evaluate_colour(
            properties[text].expression, properties, dark, accent, base_alpha, depth + 1
        )
    raise Unparsed(f"theme.slint computes a colour as {expression!r}")


def evaluate_alpha(expression, properties, dark, base_alpha, depth=0):
    """An alpha expression, evaluated as the fraction it spells."""
    text = expression.strip()
    if depth > 8:
        raise Unparsed(f"expression nests too deeply: {expression!r}")
    conditional = CONDITIONAL.fullmatch(text)
    if conditional:
        chosen = conditional.group(1) if dark else conditional.group(2)
        return evaluate_alpha(chosen, properties, dark, base_alpha, depth + 1)
    if text == "base-alpha":
        return base_alpha
    if text in properties:
        return evaluate_alpha(properties[text].expression, properties, dark, base_alpha, depth + 1)
    return float(text)


def name_pattern(name):
    """A name, not the inside of a longer hyphenated one."""
    return re.compile(r"(?<![\w-])" + re.escape(name) + r"(?![\w-])")


def slint_sources():
    return sorted((ROOT / UI_DIR).rglob("*.slint"))


def reference_counts(names):
    """How often each name is read across the .slint sources.

    A declaration is not a read, so the property that introduces a name does not
    count as a use of it.
    """
    counts = dict.fromkeys(names, 0)
    patterns = {name: name_pattern(name) for name in names}
    for path in slint_sources():
        try:
            text = path.read_text(encoding="utf-8")
        except OSError as error:
            die(f"cannot read {path}: {error}")
        for raw in text.splitlines():
            line = code_only(raw)
            if not line.strip():
                continue
            declaration = DECLARATION.fullmatch(line.strip())
            for name, pattern in patterns.items():
                hits = len(pattern.findall(line))
                if declaration is not None and declaration.group(3) == name:
                    hits -= 1
                counts[name] += hits
    return counts


def rust_sources(excluded):
    """Every Rust source of ime-ui except the ones named, as (path, text)."""
    sources = []
    for path in sorted((ROOT / SRC_DIR).rglob("*.rs")):
        relative = path.relative_to(ROOT).as_posix()
        if relative in excluded:
            continue
        try:
            sources.append((relative, path.read_text(encoding="utf-8")))
        except OSError as error:
            die(f"cannot read {path}: {error}")
    return sources


def consumed_by_rust(name, sources):
    pattern = name_pattern(name)
    for relative, text in sources:
        if pattern.search(text):
            return relative
    return None


# --------------------------------------------------------------------------
# src/theme.rs
# --------------------------------------------------------------------------

CHANNEL = re.compile(r"\b([rgba]):\s*([A-Za-z0-9_]+)")
SCALAR = re.compile(r"^\s*(\w+):\s*(\d+),\s*$", re.M)
RGBA_BLOCK = re.compile(r"(\w+):\s*Rgba8\s*\{([^}]*)\}", re.S)


def channel_value(text):
    if text == "OPAQUE_ALPHA":
        return 255
    if text.lower().startswith("0x"):
        return int(text, 16)
    return int(text)


def channels(field, where):
    found = dict(CHANNEL.findall(field))
    values = []
    for key in "rgba":
        if key not in found:
            die(f"{where} has no {key} channel")
        values.append(channel_value(found[key]))
    return tuple(values)


def palette(text, constant):
    """One scheme's palette: its colour fields and its alpha scalars."""
    match = re.search(r"const " + constant + r": Palette = Palette \{(.*?)\n\};", text, re.S)
    if match is None:
        die(f"{THEME_RS} no longer declares {constant}")
    body = match.group(1)
    colours = {}
    for field in RGBA_BLOCK.finditer(body):
        colours[field.group(1)] = channels(field.group(2), f"{constant}.{field.group(1)}")
    scalars = {
        name: int(value) for name, value in SCALAR.findall(RGBA_BLOCK.sub("", body))
    }
    return colours, scalars


def accent_constant(text, constant):
    match = re.search(r"const " + constant + r": Rgba8 = Rgba8 \{([^}]*)\}", text, re.S)
    if match is None:
        die(f"{SCHEME_RS} no longer declares {constant}")
    return channels(match.group(1), constant)


def metric_value(declaration):
    text = declaration.expression.strip()
    if text.endswith("px"):
        text = text[:-2]
    try:
        return float(text)
    except ValueError:
        return None


# --------------------------------------------------------------------------
# The assertions
# --------------------------------------------------------------------------


def check_sizes(features, metrics):
    """3.1.1's size rows against the constants `CandidateMetrics` declares."""
    rows = {}
    for cells in table_rows(section(features, SECTION_SIZES)):
        if len(cells) >= 2:
            rows[cells[0]] = cells[1]
    mapped = {label for label, _ in SIZE_ROWS}
    if set(rows) != mapped:
        fail(
            "features.md 3.1.1 has rows this gate does not map: "
            + ", ".join(sorted(set(rows) ^ mapped))
        )
    checked = 0
    for label, entries in SIZE_ROWS:
        cell = rows.get(label)
        if cell is None:
            fail(f"features.md 3.1.1 no longer has the row {label!r}")
            continue
        for name, unit, index in entries:
            if unit == "dp":
                values = cell_numbers(cell, "dp")
            elif unit == "sp":
                values = cell_numbers(cell, "sp")
            else:
                values = cell_counts(cell)
            if index >= len(values):
                fail(f"features.md 3.1.1 {label!r} no longer spells the value of {name}")
                continue
            expected = values[index]
            declared = metrics.get(name)
            if declared is None:
                fail(f"candidate.slint does not declare {name}, which 3.1.1 {label!r} fixes")
                continue
            actual = metric_value(declared)
            if actual is None:
                fail(f"candidate.slint declares {name} as {declared.expression!r}, not a number")
                continue
            if actual != expected:
                fail(
                    f"3.1.1 {label!r} says {name} is {format_number(expected)}{unit}, "
                    f"candidate.slint declares {format_number(actual)}"
                )
            checked += 1
    return checked


def check_colours(features, theme_properties, theme_rs_text, scheme_rs_text):
    """3.2's colour rows against theme.slint's defaults and theme.rs's palette."""
    rows = {}
    for cells in table_rows(section(features, SECTION_COLOURS)):
        if len(cells) >= 4:
            rows[unquote(cells[0])] = cells
    mapped = {token for token, _, _, _ in COLOUR_ROWS}
    if set(rows) != mapped:
        fail(
            "features.md 3.2 has tokens this gate does not map: "
            + ", ".join(sorted(set(rows) ^ mapped))
        )

    declared_names = sorted(
        name for name, entry in theme_properties.items() if entry.type == "color"
    )
    if declared_names != sorted(EXPECTED_COLOUR_PROPERTIES):
        fail(
            "theme.slint's colour properties are not 3.2's tokens: "
            + ", ".join(sorted(set(declared_names) ^ set(EXPECTED_COLOUR_PROPERTIES)))
        )
    inputs = [name for name, entry in theme_properties.items() if entry.direction == "in"]
    if inputs != EXPECTED_INPUTS:
        fail(f"theme.slint takes {inputs} from Rust, not {EXPECTED_INPUTS}")

    fill = theme_properties.get("surface-fill")
    if fill is None or fill.expression.strip() != "surface-base.with-alpha(base-alpha)":
        fail("theme.slint's surface-fill is no longer `surface-base.with-alpha(base-alpha)`")
    if not re.search(r"surface_fill:\s*with_alpha\(self\.surface_base, base_alpha\)", theme_rs_text):
        fail(f"{THEME_RS} no longer derives surface_fill from the base and the base alpha")

    base_declaration = theme_properties.get("base-alpha")
    if base_declaration is None:
        fail("theme.slint no longer declares base-alpha")
        return 0
    base_fraction = float(base_declaration.expression)
    match = re.search(r"const DEFAULT_BASE_ALPHA: u8 = (\d+);", theme_rs_text)
    if match is None:
        die(f"{THEME_RS} no longer declares DEFAULT_BASE_ALPHA")
    default_base_alpha = int(match.group(1))

    palettes = {
        True: palette(theme_rs_text, "DARK_PALETTE"),
        False: palette(theme_rs_text, "LIGHT_PALETTE"),
    }
    accents = {
        True: accent_constant(scheme_rs_text, "DEFAULT_ACCENT_DARK"),
        False: accent_constant(scheme_rs_text, "DEFAULT_ACCENT_LIGHT"),
    }

    checked = 0
    for token, slint_name, rust_field, kind in COLOUR_ROWS:
        cells = rows.get(token)
        if cells is None:
            fail(f"features.md 3.2 no longer lists {token}")
            continue
        declared = theme_properties.get(slint_name)
        if declared is None:
            fail(f"theme.slint does not declare {slint_name}, which 3.2 calls {token}")
            continue
        for dark, cell in ((True, cells[1]), (False, cells[2])):
            scheme = "dark" if dark else "light"
            where = f"3.2 {token} ({slint_name}, {scheme})"
            colours, scalars = palettes[dark]
            accent_bytes = accents[dark]
            expected, fraction = parse_table_colour(cell)
            checked += 1
            if kind == "colour":
                actual = evaluate_colour(
                    declared.expression, theme_properties, dark, accent_bytes, base_fraction
                )
                if actual != expected:
                    fail(f"{where}: the table says {expected}, theme.slint computes {actual}")
                rust = colours.get(rust_field)
                if rust is None:
                    fail(f"{where}: {THEME_RS} no longer carries {rust_field}")
                elif rust != expected:
                    fail(f"{where}: the table says {expected}, {rust_field} holds {rust}")
            elif kind == "base":
                if expected is None or fraction is None:
                    fail(f"{where}: the table no longer spells `#RRGGBB @ <alpha>`")
                    continue
                actual = evaluate_colour(
                    declared.expression, theme_properties, dark, accent_bytes, base_fraction
                )
                if actual != expected:
                    fail(f"{where}: the table says {expected}, theme.slint computes {actual}")
                rust = colours.get(rust_field)
                if rust != expected:
                    fail(f"{where}: the table says {expected}, {rust_field} holds {rust}")
                if fraction != base_fraction:
                    fail(
                        f"{where}: the table says the acrylic alpha is {fraction}, "
                        f"theme.slint ships {base_fraction}"
                    )
                elif round_alpha(fraction) != default_base_alpha:
                    fail(
                        f"{where}: {format_number(fraction)} x 255 is {round_alpha(fraction)}, "
                        f"but DEFAULT_BASE_ALPHA is {default_base_alpha}"
                    )
            elif kind == "accent":
                actual = evaluate_colour(
                    declared.expression, theme_properties, dark, accent_bytes, base_fraction
                )
                if actual != expected:
                    fail(f"{where}: the table says {expected}, theme.slint ships {actual}")
                if accent_bytes != expected:
                    fail(f"{where}: the table says {expected}, {SCHEME_RS} holds {accent_bytes}")
            elif kind == "accent-derived":
                if unquote(cell) != "accent.default":
                    fail(f"{where}: the table now spells {unquote(cell)!r}, not `accent.default`")
                if declared.expression.strip() != "accent":
                    fail(
                        f"{where}: theme.slint derives it from {declared.expression!r} "
                        "instead of from the accent property"
                    )
                if not re.search(r"status_dot_active:\s*accent\b", theme_rs_text):
                    fail(f"{where}: {THEME_RS} no longer derives status_dot_active from the accent")
            elif kind == "accent-alpha":
                call = WITH_ALPHA.fullmatch(declared.expression.strip())
                if call is None or call.group(1) != "accent":
                    fail(
                        f"{where}: theme.slint derives it from {declared.expression!r}, "
                        "not from `accent.with-alpha(...)`"
                    )
                    continue
                if fraction is None:
                    fail(f"{where}: the table no longer spells `accent @ <alpha>`")
                    continue
                actual_fraction = evaluate_alpha(
                    call.group(2), theme_properties, dark, base_fraction
                )
                if actual_fraction != fraction:
                    fail(
                        f"{where}: the table says the accent alpha is {fraction}, "
                        f"theme.slint ships {actual_fraction}"
                    )
                rust = scalars.get(rust_field)
                if rust is None:
                    fail(f"{where}: {THEME_RS} no longer carries {rust_field}")
                elif rust != round_alpha(fraction):
                    fail(
                        f"{where}: {format_number(fraction)} x 255 rounds to "
                        f"{round_alpha(fraction)}, but {rust_field} is {rust}"
                    )
    return checked


def check_grid(features, metrics):
    """3.1.4's 4dp grid, its exception table and its type scale."""
    block = section(features, SECTION_GRID)
    match = re.search(r"必须是\s*`(\d+)dp`\s*的整数倍", block)
    if match is None:
        die(f"{FEATURES} 3.1.4 no longer states the grid step")
    step = int(match.group(1))
    match = re.search(r"字号阶为\s*`([\d\s/]+)sp`", block)
    if match is None:
        die(f"{FEATURES} 3.1.4 no longer states the type scale")
    scale = {float(value) for value in re.findall(r"\d+", match.group(1))}

    exceptions = {}
    for cells in table_rows(block):
        if len(cells) < 2:
            continue
        value = re.fullmatch(r"(\d+)dp", unquote(cells[0]))
        if value is None:
            continue
        for name in re.findall(r"`([A-Za-z0-9-]+)`", cells[1]):
            exceptions[name] = float(value.group(1))
    if not exceptions:
        die(f"{FEATURES} 3.1.4's exception table is empty or unreadable")

    checked = 0
    for name, entry in metrics.items():
        value = metric_value(entry)
        if name.startswith("font-size-"):
            if value is None:
                fail(f"candidate.slint declares {name} as {entry.expression!r}, not a number")
            elif value not in scale:
                fail(
                    f"candidate.slint declares {name}: {format_number(value)}px, "
                    "which is not on the type scale of 3.1.4"
                )
            continue
        if entry.type != "length":
            continue
        if value is None:
            fail(f"candidate.slint declares {name} as {entry.expression!r}, not a number")
            continue
        checked += 1
        if name in exceptions:
            if value != exceptions[name]:
                fail(
                    f"3.1.4 lists {name} as a {format_number(exceptions[name])}dp exception, "
                    f"candidate.slint declares {format_number(value)}"
                )
            elif value % step == 0:
                fail(
                    f"3.1.4 lists {name} as an exception, but {format_number(value)} is "
                    f"already a multiple of {step}; drop the row"
                )
        elif value % step != 0:
            fail(
                f"candidate.slint declares {name}: {format_number(value)}px, which is not "
                f"a multiple of the {step}dp grid of 3.1.4 and is not in its exception table"
            )
    for name in sorted(exceptions):
        if name not in metrics:
            fail(f"3.1.4 lists {name} as an exception, but candidate.slint does not declare it")
    return checked, step, len(exceptions), len(scale)


def check_references(theme_properties, metrics):
    """The reference closure: no token and no constant may be dead."""
    tokens = [name for name, entry in theme_properties.items() if entry.type == "color"]
    constants = list(metrics)
    counts = reference_counts(tokens + constants)
    sources = rust_sources({METRICS_RS})

    dead_tokens = [name for name in tokens if counts.get(name, 0) == 0]
    dead_constants = []
    for name in constants:
        if counts.get(name, 0) > 0:
            continue
        if consumed_by_rust(name.replace("-", "_"), sources) is not None:
            continue
        dead_constants.append(name)

    for name in dead_tokens:
        if name not in DEFERRED_TOKENS:
            fail(
                f"theme.slint declares {name} and no .slint source draws it; "
                "wire it or record the specification that owes it in DEFERRED_TOKENS"
            )
    for name in sorted(DEFERRED_TOKENS):
        if name not in dead_tokens:
            fail(f"{name} is drawn now; drop its entry from DEFERRED_TOKENS")
    for name in dead_constants:
        if name not in DEFERRED_CONSTANTS:
            fail(
                f"candidate.slint declares {name} and neither a .slint source nor a Rust "
                "module reads it; wire it or record it in DEFERRED_CONSTANTS"
            )
    for name in sorted(DEFERRED_CONSTANTS):
        if name not in dead_constants:
            fail(f"{name} is read now; drop its entry from DEFERRED_CONSTANTS")

    unexplained_tokens = len([name for name in dead_tokens if name not in DEFERRED_TOKENS])
    unexplained_constants = len(
        [name for name in dead_constants if name not in DEFERRED_CONSTANTS]
    )
    if unexplained_tokens > TOKEN_BUDGET:
        fail(f"{unexplained_tokens} tokens are dead and unexplained; the budget is {TOKEN_BUDGET}")
    if unexplained_constants > CONSTANT_BUDGET:
        fail(
            f"{unexplained_constants} constants are dead and unexplained; "
            f"the budget is {CONSTANT_BUDGET}"
        )
    return tokens, constants, dead_tokens, dead_constants


def main():
    features = read(FEATURES)
    theme_text = read(THEME_SLINT)
    candidate_text = read(CANDIDATE_SLINT)
    theme_rs_text = read(THEME_RS)
    scheme_rs_text = read(SCHEME_RS)
    metrics_rs_text = read(METRICS_RS)

    theme_properties = declarations(
        global_block(theme_text, "export global Theme", THEME_SLINT), THEME_SLINT
    )
    metrics = declarations(
        global_block(candidate_text, "export global CandidateMetrics", CANDIDATE_SLINT),
        CANDIDATE_SLINT,
    )

    read_metrics = set(
        re.findall(r"take(?:_count)?\(\s*found\s*,\s*\"([A-Za-z0-9-]+)\"\s*\)", metrics_rs_text)
    )
    if not read_metrics:
        die(f"{METRICS_RS} no longer reads the metrics block")
    for name in sorted(read_metrics):
        if name not in metrics:
            fail(f"{METRICS_RS} reads {name} from the metrics block, which does not declare it")

    sizes = check_sizes(features, metrics)
    colours = check_colours(features, theme_properties, theme_rs_text, scheme_rs_text)
    grid, step, exception_count, scale_size = check_grid(features, metrics)
    tokens, constants, dead_tokens, dead_constants = check_references(theme_properties, metrics)

    if problems:
        print("check-ui-spec: FAIL - the specification and the sources disagree", file=sys.stderr)
        for problem in problems:
            print(f"  - {problem}", file=sys.stderr)
        print(
            "check-ui-spec: features.md 3.1.1, 3.1.4 and 3.2 are the authoritative tables; "
            "correct the declaration or the table, never this gate",
            file=sys.stderr,
        )
        sys.exit(1)

    print(
        "check-ui-spec: PASS ("
        f"{sizes} sizes from 3.1.1, "
        f"{colours} colours from 3.2 across theme.slint and theme.rs, "
        f"{grid} lengths on the {step}dp grid of 3.1.4 with {exception_count} exceptions, "
        f"{scale_size} type-scale sizes, "
        f"{len(tokens)} tokens and {len(constants)} constants declared, "
        f"{len(dead_tokens)} tokens and {len(dead_constants)} constants deferred)"
    )


try:
    main()
except Unparsed as error:
    die(f"cannot read a declaration: {error}")
except ValueError as error:
    die(f"cannot read a number: {error}")
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
        echo "check-ui-spec: self-test FAILED - $2 exited $status, expected 0" >&2
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
        echo "check-ui-spec: self-test FAILED - $3 was not detected" >&2
        echo "$output" >&2
        return 1
    fi
    case "$output" in
        *"$2"*) ;;
        *)
            echo "check-ui-spec: self-test FAILED - $3 was reported for the wrong reason" >&2
            echo "check-ui-spec: expected the report to mention: $2" >&2
            echo "$output" >&2
            return 1
            ;;
    esac
    return 0
}

# copy_tree DEST: the files the analyser reads, at the same relative paths.
copy_tree() {
    local destination="$1"
    mkdir -p -- "$destination/docs/dev" "$destination/crates/ime-ui"
    cp -- "$root/docs/dev/features.md" "$destination/docs/dev/features.md"
    cp -R -- "$root/crates/ime-ui/ui" "$destination/crates/ime-ui/ui"
    cp -R -- "$root/crates/ime-ui/src" "$destination/crates/ime-ui/src"
}

run_self_test() {
    local scratch
    scratch="$(mktemp -d "${TMPDIR:-/tmp}/rspinyin-ui-spec.XXXXXX")"
    # shellcheck disable=SC2064  # expand the path now, not at trap time
    trap "rm -rf -- '$scratch'" EXIT

    echo "check-ui-spec: self-test (one injected violation per assertion class)"

    # fresh_tree: an untouched copy of everything the analyser reads.
    fresh_tree() {
        rm -rf -- "$scratch/tree"
        copy_tree "$scratch/tree"
    }

    fresh_tree
    expect_clean "$scratch/tree" "the untouched sources"

    # 1. A size that drifts away from 3.1.1.
    fresh_tree
    sed -i 's/cell-height: 36px/cell-height: 38px/' "$scratch/tree/crates/ime-ui/ui/candidate.slint"
    expect_violation "$scratch/tree" "cell-height" "a size that drifted from 3.1.1"

    # 2. A colour that drifts in the Slint global.
    fresh_tree
    sed -i 's/0\.62/0.60/' "$scratch/tree/crates/ime-ui/ui/theme.slint"
    expect_violation "$scratch/tree" "text-secondary" "a colour that drifted in theme.slint"

    # 3. A colour that drifts in the Rust palette.
    fresh_tree
    sed -i 's/a: 158,/a: 157,/' "$scratch/tree/crates/ime-ui/src/theme.rs"
    expect_violation "$scratch/tree" "text-secondary" "a colour that drifted in theme.rs"

    # 4. A colour that drifts in the 3.2 table itself.
    fresh_tree
    sed -i 's/rgba(242,242,247,0.62)/rgba(242,242,247,0.64)/' "$scratch/tree/docs/dev/features.md"
    expect_violation "$scratch/tree" "text-secondary" "a colour that drifted in the 3.2 table"

    # 5. A length off the 4dp grid of 3.1.4.
    fresh_tree
    sed -i 's/shadow-inner-spread: 4px/shadow-inner-spread: 6px/' \
        "$scratch/tree/crates/ime-ui/ui/candidate.slint"
    expect_violation "$scratch/tree" "shadow-inner-spread" "a length off the 4dp grid"

    # 6. An exception dropped from 3.1.4's table, which must fail on the value it covers.
    fresh_tree
    sed -i 's/`grid-gap`、//' "$scratch/tree/docs/dev/features.md"
    expect_violation "$scratch/tree" "grid-gap" "an exception missing from the 3.1.4 table"

    # 7. A font size off the type scale of 3.1.4.
    fresh_tree
    sed -i 's/font-size-cell: 15px/font-size-cell: 13px/' \
        "$scratch/tree/crates/ime-ui/ui/candidate.slint"
    expect_violation "$scratch/tree" "font-size-cell" "a font size off the type scale"

    # 8. A token nothing draws, which no deferral covers.
    fresh_tree
    sed -i 's/^}$/    out property <color> unused-token: #000000;\n}/' \
        "$scratch/tree/crates/ime-ui/ui/theme.slint"
    expect_violation "$scratch/tree" "unused-token" "a token nothing draws"

    # 9. A deferral that has become live, which must not outlive its reason. The token
    # swapped in has to be one the list still defers, or the injected scene would be
    # reported for the token it removed rather than for the one it drew.
    fresh_tree
    sed -i 's/Theme\.shadow-inner/Theme.accent-on/' \
        "$scratch/tree/crates/ime-ui/ui/candidate.slint"
    expect_violation "$scratch/tree" "accent-on" "a deferral that has become live"

    echo "check-ui-spec: self-test PASS (9 injected violations, all reported)"
}

if [ "$mode" = "self-test" ]; then
    run_self_test
    exit 0
fi

analyse "$root"
