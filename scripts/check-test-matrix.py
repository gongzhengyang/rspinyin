#!/usr/bin/env python3
"""Mechanically audit the traceability matrix of the test-case suite.

The suite lives in ``docs/dev/tests.md`` (the hub) and eight shards under
``docs/dev/tests/``. Three document facts must never drift apart:

  * the requirement-to-case matrix in section 2 of the hub: one
    ``REQ-<MODULE>-<NN>`` row per feature, whose TC cell binds case ids
    (ranges, single ids, and a parenthesised deepening part),
  * the case headings themselves (``### TC-<MODULE>-<NN> <title>``, hub
    plus shards), and
  * the per-case executability label, one of ``[可执行]``,
    ``[不可验证]`` and ``[待实现]``.

Over the whole corpus the audit asserts:

  1. every matrix row binds at least five non-deepening cases;
  2. the matrix TC set and the case-heading TC set are equal: phantom
     references (the matrix names a case no heading defines) and orphans
     (a case heading no row binds) are reported separately;
  3. every ``REQ-*`` referenced from a case heading, or from the hub's
     body bind field, exists as a matrix row;
  4. case ids are globally unique across hub and shards;
  5. every case carries exactly one executability label drawn from the
     enumeration, and no case may remain ``[待实现]``;
  6. where a hub case repeats its bindings in a body field
     (``对应功能条目编号：``), that set equals the heading's set.

The current ``#[test]`` count over ``crates/`` and ``xtask/`` is printed
as a live baseline. The count is informational output for the self-heal
guard to quote; the script deliberately asserts no expected number, so
the document can never become a second, stale copy of the truth.

Structural drift -- a missing matrix header, a wrong column count, an
unparsable or empty TC cell, a case without its label -- is a loud
failure, never a silent skip: a matrix that silently stops being
parseable would silently stop being checked.

Usage:

    python3 scripts/check-test-matrix.py                  # audit the tree
    python3 scripts/check-test-matrix.py --root /path     # audit another tree
    python3 scripts/check-test-matrix.py --self-test      # inject and assert

Exit status:

    0  every assertion holds (or the self-test passed)
    1  assertion failure(s); every violation is listed, not just the first
    2  usage or IO error (missing documents, unreadable files)
"""

from __future__ import annotations

import argparse
import re
import shutil
import sys
import tempfile
from collections import Counter
from dataclasses import dataclass
from pathlib import Path

HUB_RELATIVE = Path("docs/dev/tests.md")
SHARD_DIR = Path("docs/dev/tests")
SHARD_NAMES = ("core", "dict", "rt", "ui", "sec", "diag", "infra", "cfg")

# The three executability states a case may carry. The execution gate knows
# exactly these values, so anything else inside the labelled field is a
# structural defect rather than a new state.
EXECUTABLE = "[可执行]"
UNVERIFIABLE = "[不可验证]"
PENDING = "[待实现]"
ALLOWED_STATES = (EXECUTABLE, UNVERIFIABLE, PENDING)
STATE_SET = frozenset(ALLOWED_STATES)

# A row below this no longer carries the five coverage dimensions the suite
# promises, so dropping under it means coverage was silently lost.
MIN_MAIN_BINDINGS = 5

# Header cells locate the two columns the audit reads, so a cosmetic column
# reorder does not break the parser; a renamed header cell fails loudly.
REQ_HEADER_PHRASE = "功能条目编号"
TC_HEADER_PHRASE = "承接用例"

REQ_CELL_RE = re.compile(r"`(REQ-[A-Z]{2,}-[0-9]{2,})`")
REQ_TOKEN_RE = re.compile(r"`(REQ-[A-Z]{2,}-[0-9]{2,})`")
TC_ID_RE = re.compile(r"TC-([A-Z]+)-([0-9]+)")
BARE_NUMBER_RE = re.compile(r"[0-9]+")
CASE_HEADING_RE = re.compile(r"^###\s+(TC-[A-Z]+-[0-9]+)(?:\s+(.*))?$")
SECTION_HEADING_RE = re.compile(r"^#{1,2}\s")
FENCE_RE = re.compile(r"^\s*(```|~~~)")
CELL_TOKEN_RE = re.compile(r"`([^`]*)`|([~、/])|(\s+)|(.)", re.DOTALL)
EXEC_FIELD_RE = re.compile(r"可执行性：`([^`]*)`")
BIND_MARKER = "对应功能条目编号："
DEEPENING_OPEN = "（深化"
DEEPENING_CLOSE = "）"
UNIT_TEST_TOKEN = "#[test]"


class AuditIoError(Exception):
    """A document that must exist cannot be read or decoded."""


@dataclass
class Violation:
    """One failed assertion, located well enough to be fixed directly."""

    where: str
    message: str

    def __str__(self) -> str:
        return f"{self.where}: {self.message}"


@dataclass
class MatrixRow:
    """One REQ row: its non-deepening and deepening case bindings."""

    req: str
    line: int
    main: list
    deep: list


@dataclass
class Case:
    """One case heading plus the label facts collected from its block."""

    tc: str
    path: Path
    line: int
    reqs: list
    deepening: bool
    state: object = None
    state_line: object = None
    bind_reqs: object = None


def read_lines(path: Path) -> list:
    """Reads a document as (line number, text) pairs, 1-based."""
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as exc:
        raise AuditIoError(f"cannot read {path}: {exc}") from exc
    except UnicodeDecodeError as exc:
        raise AuditIoError(f"{path} is not valid UTF-8: {exc}") from exc
    return list(enumerate(text.splitlines(), start=1))


def split_table_row(line: str):
    """Splits one markdown table line into stripped cells, or None."""
    body = line.strip()
    if not (body.startswith("|") and body.endswith("|") and len(body) >= 2):
        return None
    return [cell.strip() for cell in body[1:-1].split("|")]


def tokenize_cell(text: str, violations: list, where: str):
    """Splits a TC cell into ('ref', s) / ('sep', ch) tokens.

    Anything the cell grammar does not name -- stray characters, an
    unterminated backtick -- is recorded and aborts this cell: half of a
    broken cell must never silently become a binding.
    """
    tokens = []
    for match in CELL_TOKEN_RE.finditer(text):
        ref, sep, _space, other = match.groups()
        if ref is not None:
            tokens.append(("ref", ref))
        elif sep is not None:
            tokens.append(("sep", sep))
        elif other is not None:
            violations.append(Violation(
                where, f"unexpected character {other!r} in TC cell {text!r}"))
            return None
    return tokens


def resolve_ref(raw: str, state: dict, violations: list, where: str):
    """Turns one cell atom into a full case id.

    A bare number continues the module (and the zero-padded digit width) of
    the last full id in the same cell, which is how abbreviated range tails
    such as 01~05 are written.
    """
    match = TC_ID_RE.fullmatch(raw)
    if match:
        state["module"] = match.group(1)
        state["width"] = len(match.group(2))
        return raw
    if BARE_NUMBER_RE.fullmatch(raw):
        module = state.get("module")
        if module is None:
            violations.append(Violation(
                where, f"bare number `{raw}` with no preceding full case id"))
            return None
        return f"TC-{module}-{raw.zfill(state['width'])}"
    violations.append(Violation(
        where, f"unparsable reference `{raw}` (expected TC-<MODULE>-<NN> "
               f"or a bare continuation number)"))
    return None


def expand_range(head: str, tail: str, violations: list, where: str) -> list:
    """Expands an inclusive ``a~b`` range into full case ids."""
    head_match = TC_ID_RE.fullmatch(head)
    tail_match = TC_ID_RE.fullmatch(tail)
    if tail_match.group(1) != head_match.group(1):
        violations.append(Violation(
            where, f"range {head}~{tail} crosses two modules"))
        return []
    start, end = int(head_match.group(2)), int(tail_match.group(2))
    if end < start:
        violations.append(Violation(
            where, f"descending range {head}~{tail}"))
        return []
    width = len(head_match.group(2))
    return [f"TC-{head_match.group(1)}-{n:0{width}d}"
            for n in range(start, end + 1)]


def parse_cell_part(tokens, violations: list, where: str, label: str):
    """Parses one cell part (main or deepening) into a list of case ids.

    The grammar is: item (``、`` or ``/`` item)*, where an item is one
    reference or a ``~`` range. Anything else in the token stream is a
    structural defect and fails the cell.
    """
    if tokens is None:
        return None
    ids = []
    state: dict = {}
    i, n = 0, len(tokens)
    need_ref = True
    while i < n:
        kind, value = tokens[i]
        if need_ref:
            if kind != "ref":
                violations.append(Violation(
                    where, f"expected a quoted case reference before "
                           f"{value!r} in {label} part"))
                return None
            head = resolve_ref(value, state, violations, where)
            if head is None:
                return None
            if i + 1 < n and tokens[i + 1] == ("sep", "~"):
                if i + 2 >= n or tokens[i + 2][0] != "ref":
                    violations.append(Violation(
                        where, f"range opened with `~` has no end in "
                               f"{label} part"))
                    return None
                tail = resolve_ref(tokens[i + 2][1], state, violations, where)
                if tail is None:
                    return None
                ids.extend(expand_range(head, tail, violations, where))
                i += 3
            else:
                ids.append(head)
                i += 1
            need_ref = False
        else:
            if kind != "sep" or value == "~":
                violations.append(Violation(
                    where, f"expected `、` or `/` between case references, "
                           f"found {value!r} in {label} part"))
                return None
            need_ref = True
            i += 1
    if need_ref and tokens:
        violations.append(Violation(
            where, f"trailing separator at the end of {label} part"))
        return None
    for tc in sorted({tc for tc in ids if ids.count(tc) > 1}):
        violations.append(Violation(
            where, f"case `{tc}` listed twice in the same TC cell"))
    return list(dict.fromkeys(ids))


def split_deepening(cell: str, violations: list, where: str):
    """Splits a TC cell into its main text and its deepening text."""
    idx = cell.find(DEEPENING_OPEN)
    if idx < 0:
        return cell, None
    rest = cell[idx + len(DEEPENING_OPEN):].rstrip()
    if not rest.endswith(DEEPENING_CLOSE):
        violations.append(Violation(
            where, f"deepening part opened with {DEEPENING_OPEN} is never "
                   f"closed with {DEEPENING_CLOSE}"))
        return None, None
    return cell[:idx], rest[:-1]


def parse_matrix(path: Path, lines: list, violations: list):
    """Parses the section-2 matrix into rows.

    Returns (rows, usable, rescued): ``usable`` is False when the table
    itself is missing or empty, in which case the set-based assertions are
    skipped (the structural violations already fail the run, and comparing
    against an empty matrix would only pile phantom-orphan noise on top).
    ``rescued`` holds case ids recovered by a lenient scan of cells that
    failed the strict grammar, so those cases do not double-report as
    orphans on top of the cell defect.
    """
    header = None
    req_col = tc_col = ncol = -1
    row_lines = []
    for no, text in lines:
        stripped = text.strip()
        if not stripped.startswith("|"):
            if header is not None:
                break
            continue
        cells = split_table_row(stripped)
        if cells is None:
            if header is not None:
                violations.append(Violation(
                    f"{path}:{no}", f"malformed table line: {text.strip()!r}"))
            continue
        if header is None:
            flat = [re.sub(r"\s+", "", cell) for cell in cells]
            if (any(REQ_HEADER_PHRASE in cell for cell in flat)
                    and any(TC_HEADER_PHRASE in cell for cell in flat)):
                header = cells
                ncol = len(cells)
                req_col = flat.index(next(
                    cell for cell in flat if REQ_HEADER_PHRASE in cell))
                tc_col = flat.index(next(
                    cell for cell in flat if TC_HEADER_PHRASE in cell))
            continue
        if all(set(cell) <= set("-: ") for cell in cells):
            continue
        row_lines.append((no, cells))
    if header is None:
        violations.append(Violation(
            str(path), "no traceability matrix found: expected a header row "
                       f"naming {REQ_HEADER_PHRASE!r} and {TC_HEADER_PHRASE!r}"))
        return [], False, set()

    rows = []
    seen_reqs = {}
    rescued = set()
    for no, cells in row_lines:
        where = f"{path}:{no}"
        if len(cells) != ncol:
            violations.append(Violation(
                where, f"matrix row has {len(cells)} cells, "
                       f"the header has {ncol}"))
            continue
        req_match = REQ_CELL_RE.fullmatch(cells[req_col])
        if req_match is None:
            violations.append(Violation(
                where, f"requirement column is not a backticked REQ id: "
                       f"{cells[req_col]!r}"))
            continue
        req = req_match.group(1)
        if req in seen_reqs:
            violations.append(Violation(
                where, f"duplicate matrix row for {req}, "
                       f"first defined at line {seen_reqs[req]}"))
        seen_reqs[req] = no
        tc_cell = cells[tc_col]
        if not tc_cell.strip():
            violations.append(Violation(where, "TC cell is empty"))
            continue
        main_text, deep_text = split_deepening(tc_cell, violations, where)
        if main_text is None:
            continue
        main = parse_cell_part(
            tokenize_cell(main_text, violations, where),
            violations, where, "main")
        deep = []
        if deep_text is not None:
            deep = parse_cell_part(
                tokenize_cell(deep_text, violations, where),
                violations, where, "deepening")
        if main is None or deep is None:
            rescued.update(re.findall(r"TC-[A-Z]+-[0-9]+", tc_cell))
            continue
        rows.append(MatrixRow(req, no, main, deep))
    if not rows:
        violations.append(Violation(
            str(path), "traceability matrix has no parsable rows"))
        return rows, False, rescued
    return rows, True, rescued


def parse_cases(path: Path, lines: list, violations: list, is_hub: bool) -> list:
    """Collects case headings and the label facts of each case block.

    A case block runs from its heading to the next case heading, the next
    top/section-level heading, or end of file. Fenced code is transparent:
    a ``### `` line inside an example must not split a block, and the
    template heading in the hub's fenced continuation section must not
    become a case.
    """
    cases = []
    current = None
    state_fields = []
    bind_hits = []

    def finish():
        """Attaches the collected fields to the open case, if any."""
        nonlocal current, state_fields, bind_hits
        if current is None:
            return
        if len(state_fields) != 1:
            violations.append(Violation(
                f"{path}:{current.line}",
                f"case {current.tc} carries {len(state_fields)} executability "
                f"fields, expected exactly 1"))
        else:
            value, field_line = state_fields[0]
            current.state_line = field_line
            label = value.strip()
            if label not in STATE_SET:
                violations.append(Violation(
                    f"{path}:{field_line}",
                    f"case {current.tc} has executability value {value!r}, "
                    f"not one of {', '.join(ALLOWED_STATES)}"))
            else:
                current.state = label
        if is_hub and bind_hits:
            reqs = set()
            for text, hit_line in bind_hits:
                found = REQ_TOKEN_RE.findall(text)
                if not found:
                    violations.append(Violation(
                        f"{path}:{hit_line}",
                        f"case {current.tc} has a {BIND_MARKER} field naming "
                        f"no requirement"))
                reqs.update(found)
            current.bind_reqs = reqs
        cases.append(current)
        current = None
        state_fields = []
        bind_hits = []

    fenced = False
    for no, text in lines:
        if FENCE_RE.match(text):
            fenced = not fenced
            continue
        if fenced:
            continue
        heading = CASE_HEADING_RE.match(text)
        if heading:
            finish()
            title = heading.group(2) or ""
            current = Case(
                tc=heading.group(1),
                path=path,
                line=no,
                reqs=REQ_TOKEN_RE.findall(title),
                deepening="深化" in title,
            )
            continue
        if current is None:
            continue
        if SECTION_HEADING_RE.match(text):
            finish()
            continue
        state_fields.extend(
            (value, no) for value in EXEC_FIELD_RE.findall(text))
        if is_hub and BIND_MARKER in text:
            bind_hits.append((text, no))
    finish()
    return cases


def count_unit_tests(root: Path) -> tuple:
    """Counts literal ``#[test]`` occurrences under crates/ and xtask/.

    Returns (count, complete): ``complete`` is False when neither tree
    exists, which is normal for the self-test's document-only sandbox.
    """
    total = 0
    seen_any = False
    for base in ("crates", "xtask"):
        directory = root / base
        if not directory.is_dir():
            continue
        seen_any = True
        for path in sorted(directory.rglob("*.rs")):
            try:
                text = path.read_text(encoding="utf-8")
            except (OSError, UnicodeDecodeError) as exc:
                raise AuditIoError(f"cannot read {path}: {exc}") from exc
            total += text.count(UNIT_TEST_TOKEN)
    return total, seen_any


def audit(root: Path) -> tuple:
    """Runs every assertion over the corpus under ``root``.

    Returns (violations, stats); the violations list is complete, never
    fail-fast, so one run shows everything that has to be fixed.
    """
    violations: list = []
    hub_path = root / HUB_RELATIVE
    if not hub_path.is_file():
        raise AuditIoError(f"missing hub document: {hub_path}")
    hub_lines = read_lines(hub_path)

    cases = []
    for name in SHARD_NAMES:
        shard_path = root / SHARD_DIR / f"{name}.md"
        if not shard_path.is_file():
            raise AuditIoError(f"missing shard document: {shard_path}")
        cases.extend(
            parse_cases(shard_path, read_lines(shard_path), violations, False))
    cases.extend(parse_cases(hub_path, hub_lines, violations, True))

    rows, matrix_usable, rescued = parse_matrix(hub_path, hub_lines, violations)

    matrix_reqs = set()
    matrix_tcs = set(rescued)
    main_total = 0
    min_main = None
    deep_marks = 0
    deep_ids = set()
    for row in rows:
        matrix_reqs.add(row.req)
        if len(row.main) < MIN_MAIN_BINDINGS:
            violations.append(Violation(
                f"{hub_path}:{row.line}",
                f"{row.req} binds only {len(row.main)} non-deepening cases, "
                f"the suite promises at least {MIN_MAIN_BINDINGS}"))
        main_total += len(row.main)
        min_main = (len(row.main) if min_main is None
                    else min(min_main, len(row.main)))
        deep_marks += len(row.deep)
        deep_ids.update(row.deep)
        matrix_tcs.update(row.main)
        matrix_tcs.update(row.deep)

    by_id = {}
    for case in cases:
        first = by_id.get(case.tc)
        if first is not None:
            violations.append(Violation(
                f"{case.path}:{case.line}",
                f"duplicate case heading {case.tc}, "
                f"first defined at {first.path}:{first.line}"))
        else:
            by_id[case.tc] = case

    if matrix_usable:
        for row in rows:
            for tc in row.main + row.deep:
                if tc not in by_id:
                    violations.append(Violation(
                        f"{hub_path}:{row.line}",
                        f"matrix binds {tc} under {row.req}, but no case "
                        f"heading defines it"))
        for tc, case in by_id.items():
            if tc not in matrix_tcs:
                violations.append(Violation(
                    f"{case.path}:{case.line}",
                    f"case {tc} is not bound by any matrix row"))
        for case in cases:
            where = f"{case.path}:{case.line}"
            for req in case.reqs:
                if req not in matrix_reqs:
                    violations.append(Violation(
                        where, f"case {case.tc} references {req}, "
                               f"which has no matrix row"))
            if case.bind_reqs:
                for req in sorted(case.bind_reqs):
                    if req not in matrix_reqs:
                        violations.append(Violation(
                            where, f"case {case.tc} body field references "
                                   f"{req}, which has no matrix row"))
                if set(case.reqs) != case.bind_reqs:
                    violations.append(Violation(
                        where,
                        f"case {case.tc} body field binds "
                        f"{sorted(case.bind_reqs)} but its heading binds "
                        f"{sorted(set(case.reqs))}"))

    for case in cases:
        if case.state == PENDING:
            violations.append(Violation(
                f"{case.path}:{case.state_line}",
                f"case {case.tc} is still labelled {PENDING}; the suite must "
                f"be release-clean"))

    unit_tests, trees_present = count_unit_tests(root)
    hub_cases = sum(1 for case in cases if case.path == hub_path)
    stats = {
        "rows": len(rows),
        "main_bindings": main_total,
        "min_main": 0 if min_main is None else min_main,
        "deep_marks": deep_marks,
        "deep_unique": len(deep_ids),
        "matrix_tcs": len(matrix_tcs),
        "hub_cases": hub_cases,
        "shard_cases": len(cases) - hub_cases,
        "cases": len(cases),
        "unique_ids": len(by_id),
        "deepening_titles": sum(1 for case in cases if case.deepening),
        "states": Counter(case.state for case in cases if case.state),
        "unit_tests": unit_tests,
        "trees_present": trees_present,
    }
    return violations, stats


def print_stats(stats: dict) -> None:
    """Prints the live counts the suite quotes in its own documents."""
    states = stats["states"]
    labelled = sum(states.values())
    print(f"check-test-matrix: matrix {stats['rows']} rows, "
          f"{stats['main_bindings']} main bindings "
          f"(min per row {stats['min_main']}), "
          f"{stats['deep_marks']} deepening marks "
          f"({stats['deep_unique']} distinct), "
          f"{stats['matrix_tcs']} distinct bound cases")
    print(f"check-test-matrix: cases {stats['cases']} "
          f"(hub {stats['hub_cases']}, shards {stats['shard_cases']}), "
          f"{stats['unique_ids']} distinct ids, "
          f"{stats['deepening_titles']} deepening-titled")
    print(f"check-test-matrix: executability "
          f"{EXECUTABLE} {states.get(EXECUTABLE, 0)}, "
          f"{UNVERIFIABLE} {states.get(UNVERIFIABLE, 0)}, "
          f"{PENDING} {states.get(PENDING, 0)} "
          f"(labelled {labelled} of {stats['cases']} cases)")
    note = "" if stats["trees_present"] else " (no crates/ or xtask/ tree)"
    print(f"check-test-matrix: #[test] occurrences under crates/ and "
          f"xtask/: {stats['unit_tests']}{note}")


def render(violation: Violation, root: Path) -> str:
    """Formats a violation with paths shown relative to the audited root."""
    return str(violation).replace(f"{root}/", "")


def self_test(default_root: Path) -> int:
    """Injects violations into a temporary copy and asserts detection.

    The kernel is the same ``audit`` the real run uses, pointed at the
    sandbox, so the self-test exercises exactly the shipped logic. The
    real documents are never modified.
    """
    problems: list = []
    with tempfile.TemporaryDirectory(
            prefix="check-test-matrix-selftest-") as tmp:
        sandbox = Path(tmp)
        # Creating the shard directory with parents also creates docs/dev,
        # the hub's parent, so one mkdir covers both copy targets.
        shard_dir = sandbox / SHARD_DIR
        shard_dir.mkdir(parents=True)
        hub = sandbox / HUB_RELATIVE
        try:
            shutil.copyfile(default_root / HUB_RELATIVE, hub)
            for name in SHARD_NAMES:
                shutil.copyfile(default_root / SHARD_DIR / f"{name}.md",
                                shard_dir / f"{name}.md")
        except OSError as exc:
            print(f"check-test-matrix: self-test FAILED - cannot build the "
                  f"sandbox copy: {exc}", file=sys.stderr)
            return 1

        def run():
            return audit(sandbox)

        def expect(condition: bool, message: str) -> None:
            if not condition:
                problems.append(message)

        clean_violations, _ = run()
        expect(not clean_violations,
               "the unmodified document copy did not pass:\n"
               + "\n".join(f"    {render(v, sandbox)}"
                           for v in clean_violations[:10]))

        original_hub = hub.read_text(encoding="utf-8")

        def inject(needle: str, replacement: str, description: str) -> bool:
            """Applies one mutation, failing loudly if its anchor drifted."""
            count = original_hub.count(needle)
            if count != 1:
                problems.append(
                    f"self-test anchor for {description} occurs {count} "
                    f"times, expected 1: {needle}")
                return False
            hub.write_text(original_hub.replace(needle, replacement),
                           encoding="utf-8")
            return True

        # A row pointing at a case id no heading ever defined.
        if inject("`TC-CORE-01`~`05`", "`TC-CORE-01`~`04`、`99`",
                  "wrong binding"):
            hits = [v for v in run()[0] if "TC-CORE-99" in str(v)]
            expect(hits, "a row pointing at nonexistent TC-CORE-99 "
                         "was not detected")
            hub.write_text(original_hub, encoding="utf-8")

        # A real case losing the only row that binds it.
        if inject("`TC-SEC-36`~`42`", "`TC-SEC-36`~`41`", "dropped binding"):
            hits = [v for v in run()[0] if "TC-SEC-42" in str(v)]
            expect(hits, "dropping TC-SEC-42 from its row, leaving the case "
                         "unbound, was not detected")
            hub.write_text(original_hub, encoding="utf-8")

        # A case heading defined twice.
        cfg = shard_dir / "cfg.md"
        original_cfg = cfg.read_text(encoding="utf-8")
        start = original_cfg.index("### TC-CFG-01 ")
        end = original_cfg.index("### TC-CFG-02 ")
        cfg.write_text(original_cfg + "\n" + original_cfg[start:end],
                       encoding="utf-8")
        hits = [v for v in run()[0]
                if "TC-CFG-01" in str(v) and "duplicate" in str(v)]
        expect(hits, "a duplicated TC-CFG-01 heading was not detected")
        cfg.write_text(original_cfg, encoding="utf-8")

        restored_violations, _ = run()
        expect(not restored_violations,
               "the restored document copy did not pass after the "
               "injections were removed:\n"
               + "\n".join(f"    {render(v, sandbox)}"
                           for v in restored_violations[:10]))

    if problems:
        for problem in problems:
            print(f"check-test-matrix: self-test FAILED - {problem}",
                  file=sys.stderr)
        return 1
    print("check-test-matrix: self-test PASS "
          "(3 injected violations detected, clean copy accepted)")
    return 0


def main(argv=None) -> int:
    """CLI entry point; see the module docstring for exit semantics."""
    parser = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument(
        "--root", type=Path,
        default=Path(__file__).resolve().parent.parent,
        help="repository root (default: the parent of scripts/)")
    parser.add_argument(
        "--self-test", action="store_true",
        help="inject violations into a temporary copy and assert detection")
    args = parser.parse_args(argv)

    if args.self_test:
        try:
            return self_test(args.root)
        except AuditIoError as exc:
            print(f"check-test-matrix: {exc}", file=sys.stderr)
            return 2

    try:
        violations, stats = audit(args.root)
    except AuditIoError as exc:
        print(f"check-test-matrix: {exc}", file=sys.stderr)
        return 2
    print_stats(stats)
    if violations:
        print(f"check-test-matrix: FAIL ({len(violations)} violations)",
              file=sys.stderr)
        for violation in violations:
            print(f"  - {render(violation, args.root)}", file=sys.stderr)
        return 1
    states = stats["states"]
    print(f"check-test-matrix: PASS ({stats['rows']} matrix rows, "
          f"{stats['cases']} cases, executability "
          f"{states.get(EXECUTABLE, 0)}/{states.get(UNVERIFIABLE, 0)}/"
          f"{states.get(PENDING, 0)}, #[test] {stats['unit_tests']})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
