#!/usr/bin/env bash
#
# gen-licenses.sh - audit every dependency's licence and refresh the compliance
# documents.
#
# The project's own obligation: every dependency, direct or transitive, must carry
# a licence compatible with the `MIT OR Apache-2.0` rspinyin ships under; `GPL-*`,
# `AGPL-*` and unidentifiable licences are release blockers because no amount of
# attribution satisfies them. Slint's obligation: ADR-0000 elects the Royalty-free
# 2.0 branch of Slint's tri-licence, which carries `OB-1`..`OB-6` -- checked here
# against the licence text shipped inside the Slint package, never against a
# summary of it, because a summary is not the licence.
#
# SPDX expressions are evaluated, not string-matched: `A OR B` offers a choice, so
# one allowed branch is enough and the project elects it, while `A AND B` requires
# every operand. That is what makes `Unlicense OR MIT` acceptable, and what makes
# Slint's `GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR ...` acceptable --
# the elected branch is the royalty-free one. `LicenseRef-Slint-*` is honoured for
# the Slint stack only, and the REVIEWED table cannot cover a crate inside the
# Linux build closure, so an exemption cannot smuggle code into the shipped library.
#
# `--write` refreshes the generated blocks inside docs/dev/licenses.md and
# docs/dev/NOTICE; `--check` (the default) verifies that those documents still
# register every package, every dictionary source and all six obligations. Both
# read `cargo metadata`, never Cargo.lock, because the lockfile hides renamed and
# optional dependencies.
#
# The files the repository itself has to carry are asserted here rather than in a
# workflow: the two texts the workspace manifest publishes under (LICENSE-APACHE,
# LICENSE-MIT) and LICENSES/, whose copies of third-party licence texts are what
# docs/dev/NOTICE points a release's user at. A notice that names a path the
# repository does not have ships a dangling reference, which is what the LICENSES/
# half exists to make impossible.
#
# Exit codes: 0 = pass, 1 = violation, 2 = usage or environment error. The self-test
# injects one violating package at a time into the real resolve graph, asserts each
# is rejected, then exercises the source rules and the document pipeline in a
# scratch tree. Requires python3 3.11+ (tomllib) and cargo.

set -euo pipefail

usage() {
    cat <<'USAGE'
usage: scripts/gen-licenses.sh [--write | --check] [--self-test] [--check-links]
       [--metadata-file FILE] [--sources FILE] [--raw-dir DIR] [--root DIR]
  --check            audit and verify the committed documents (default)
  --write            audit and refresh the generated blocks in the documents
  --self-test        exercise the classifier and the document pipeline
  --check-links      also probe https://slint.dev for reachability (needs network)
  --metadata-file F  analyse a captured `cargo metadata` document
  --sources FILE     source allowlist (default: <root>/data/sources.toml)
  --raw-dir DIR      raw dictionary sources (default: <root>/data/raw)
  --root DIR         repository root (default: parent of this script)
USAGE
}

die() {
    echo "gen-licenses: $*" >&2
    exit 2
}

mode="check"
metadata_file=""
sources_file=""
raw_dir=""
check_links=0
script_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
root="$script_root"
metadata_tmp=""
scratch=""

cleanup() {
    [ -n "$metadata_tmp" ] && rm -f -- "$metadata_tmp"
    [ -n "$scratch" ] && rm -rf -- "$scratch"
    return 0
}
trap cleanup EXIT

while [ $# -gt 0 ]; do
    case "$1" in
        --check) mode="check"; shift ;;
        --write) mode="write"; shift ;;
        --self-test) mode="self-test"; shift ;;
        --check-links) check_links=1; shift ;;
        --metadata-file) metadata_file="${2:?--metadata-file needs a path}"; shift 2 ;;
        --sources) sources_file="${2:?--sources needs a path}"; shift 2 ;;
        --raw-dir) raw_dir="${2:?--raw-dir needs a directory}"; shift 2 ;;
        --root) root="${2:?--root needs a directory}"; shift 2 ;;
        -h | --help) usage; exit 0 ;;
        *) echo "gen-licenses: unknown argument: $1" >&2; usage >&2; exit 2 ;;
    esac
done

[ -n "$sources_file" ] || sources_file="$root/data/sources.toml"
[ -n "$raw_dir" ] || raw_dir="$root/data/raw"
command -v python3 >/dev/null 2>&1 || die "python3 is required to parse cargo metadata"
python3 -c 'import tomllib' >/dev/null 2>&1 || die "python3 3.11+ is required (tomllib)"

# capture_metadata: write the resolve graph to $metadata_tmp.
#
# `--filter-platform` makes cargo evaluate the target cfgs, so the "ships" set is
# the real Linux build closure rather than every crate the lockfile pins for
# Windows, macOS, Android and wasm -- which is how a Windows-only clipboard
# backend ends up inventoried but not treated as shipped code.
capture_metadata() {
    if [ -z "$metadata_tmp" ]; then
        metadata_tmp="$(mktemp "${TMPDIR:-/tmp}/rspinyin-licences.XXXXXX")"
    fi
    if [ -n "$metadata_file" ]; then
        cat -- "$metadata_file" >"$metadata_tmp" || die "cannot read metadata file: $metadata_file"
        return 0
    fi
    local triple
    triple="$(python3 -c 'import platform; print(platform.machine() + "-unknown-linux-gnu")')"
    if ! (cd -- "$root" && cargo metadata --format-version 1 --all-features --locked \
        --filter-platform "$triple") >"$metadata_tmp"; then
        die "'cargo metadata' failed under $root; fix the workspace manifest (or the committed lockfile) and re-run"
    fi
}

# run_audit MODE [CASE]: run the auditor.
#
# Positional contract of the python program, in order: MODE (`check`, `write` or
# `deps-only`), metadata file, sources.toml, raw dir, root, readme dir, and the
# self-test injection case (empty on a real run).
run_audit() {
    if [ -n "$metadata_file" ] || [ -z "$metadata_tmp" ]; then
        capture_metadata
    fi
    python3 - "$1" "$metadata_tmp" "$sources_file" "$raw_dir" "$root" "$root" "${@:2}" <<'PY'
import hashlib
import json
import re
import subprocess
import sys
import tomllib
from pathlib import Path

(mode, metadata_path, sources_path, raw_dir, root, readme_dir) = (
    sys.argv[1], Path(sys.argv[2]), Path(sys.argv[3]), Path(sys.argv[4]),
    Path(sys.argv[5]), Path(sys.argv[6]),
)
case = sys.argv[7] if len(sys.argv) > 7 else ""
LICENSES_DOC = root / "docs" / "dev" / "licenses.md"
NOTICE_DOC = root / "docs" / "dev" / "NOTICE"
# Licences the project accepts, from the task card's allowlist. `MPL-2.0` is on
# it with the caveat licenses.md records: it is file-level copyleft, so a
# modified MPL file has to stay MPL.
ALLOWED = frozenset({"MIT", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "ISC", "Zlib",
                     "Unicode-3.0", "MPL-2.0", "CC0-1.0", "0BSD"})
# The permissive subset the dictionary sources stay inside (ADR-0000 decision 2);
# `Unicode-DFS-2016` is the older spelling of the Unicode licence.
PERMISSIVE_SOURCES = ALLOWED | {"Unicode-DFS-2016"}
# Identifiers of the Slint tri-licence: acceptable for the Slint stack only, and
# only because ADR-0000 elects the royalty-free branch.
SLINT_IDENTIFIERS = frozenset({"LicenseRef-Slint-Royalty-free-2.0", "LicenseRef-Slint-Software-3.0"})
# Licences outside the allowlist reviewed and accepted for a named, non-shipping
# scope: (crates covered, why). An entry cannot cover a crate inside the Linux
# build closure -- code that ships needs an allowlisted licence, not a note.
REVIEWED = {
    "BSL-1.0": (frozenset({"clipboard-win", "error-code"}),
                "BSL-1.0 已获 OSI 认可且属宽松许可；两个 crate 均为 Windows 专用"
                "（Win32 剪贴板后端及其错误辅助库），不在 Linux 构建闭包内"),
    "NCSA": (frozenset({"libfuzzer-sys"}),
             "NCSA（伊利诺伊大学）是 MIT/BSD 系宽松许可；libfuzzer-sys 是 rav1e 的"
             "`fuzzing` 可选依赖（Cargo.lock 会钉住尚未启用的可选依赖），本 workspace 从不"
             "启用该 feature，因而不在 Linux 构建闭包内"),
}
# A crate whose licence field is missing is a blocker, not a warning: an
# unattributed dependency cannot be redistributed.
UNKNOWN = "（缺失）"
# The two licence texts the workspace manifest publishes the project under, each
# with a phrase that identifies it. A file that exists but holds the other licence
# is exactly the mistake a presence check misses, and a release archive, a `.deb`
# and an `.rpm` all carry both files.
PROJECT_LICENCES = (
    ("LICENSE-APACHE", ("Apache License", "Version 2.0, January 2004")),
    ("LICENSE-MIT", ("MIT License", "Permission is hereby granted, free of charge")),
)
TOKEN = re.compile(r"\(|\)|\bAND\b|\bOR\b|[^\s()]+")
# Generated blocks. licenses.md and NOTICE are hand-maintained documents and the
# script owns only what lies between these markers, so a reviewer reads the
# compliance prose in the file rather than in a heredoc.
BLOCKS = {
    "inventory": ("<!-- BEGIN GENERATED: dependency inventory -->",
                  "<!-- END GENERATED: dependency inventory -->"),
    "sources": ("<!-- BEGIN GENERATED: dictionary sources -->", "<!-- END GENERATED: dictionary sources -->"),
    "obligations": ("<!-- BEGIN GENERATED: slint obligations -->", "<!-- END GENERATED: slint obligations -->"),
    "notice": ("<!-- BEGIN GENERATED: slint licence and data sources -->",
               "<!-- END GENERATED: slint licence and data sources -->"),
}

# The six obligations of ADR-0000 with the sentence in the licence text that
# establishes each one. The excerpt written into licenses.md is located in the
# real licence file by these anchors; a missing anchor means the licence changed
# and the review has to be redone, so the gate fails instead of quoting stale
# text. The last field is how the obligation is checked here.
OBLIGATIONS = (
    ("OB-1", "归属展示", "Slint attribution badge",
     "断言 `README.md` 与 `README.zh.md` 含 Slint 归属徽章与 `https://slint.dev` 链接"
     "（`--check-links` 时另做可达性探测）"),
    ("OB-2", "不得单独分发 Slint", "publicly available alone and without integration",
     "扫描 `packaging/` 与构建产物，断言不存在独立的 Slint 库文件（只允许 `librspinyin.so`）"),
    ("OB-3", "不得用于嵌入式系统", "within Embedded Systems",
     "断言本文件第 6 节含显式的嵌入式/自助终端/车机排除声明"),
    ("OB-4", "不得暴露 Slint API", "exposes the APIs, in part or in total",
     "由 `scripts/check-slint-leak.sh` 解析 `cargo public-api -p ime-ui` 强制（0.4 规则 11）"),
    ("OB-5", "不得移除许可声明", "remove or alter any license notices",
     "断言 `LICENSES/` 存在且非空、其中的许可原文与 Slint 发行包内的同名原文逐字一致、"
     "`docs/dev/NOTICE` 引用的每个 `LICENSES/` 路径都真实存在，并断言 `git status` 无 "
     "`LICENSES/` 下的改动"),
    ("OB-6", "按现状提供、无担保", 'on an "as is" basis, without warranties',
     "断言本文件第 7 节与 `README` 许可段含“按现状提供、无担保”的转述"),
)

problems = []


def report(problem):
    problems.append(problem)


def finish(summary):
    if problems:
        print("gen-licenses: FAIL", file=sys.stderr)
        for problem in problems:
            print(f"  - {problem}", file=sys.stderr)
        print("gen-licenses: 合规问题必须修复或以书面豁免登记；`--write` 只刷新文档，不会掩盖问题",
              file=sys.stderr)
        return 1
    print(summary)
    return 0


# ── SPDX expressions ─────────────────────────────────────────────────────────


def alternatives(expression):
    """The licence sets an SPDX expression offers: OR branches, AND products.

    The legacy `A/B` spelling means OR, and `WITH <exception>` broadens the
    licence it is attached to rather than naming another one, so it is stripped
    and the base identifier decides.
    """
    tokens = TOKEN.findall(re.sub(r"\s+WITH\s+\S+", "", expression.replace("/", " OR ")))
    position = 0

    def operand():
        nonlocal position
        if tokens[position] != "(":
            licence = tokens[position]
            position += 1
            return [frozenset({licence})]
        position += 1
        result = either()
        position += 1  # the matching ")"
        return result

    def conjunction():
        nonlocal position
        result = operand()
        while position < len(tokens) and tokens[position] == "AND":
            position += 1
            # Evaluated once: inside a comprehension it would re-consume tokens
            # for every element of `result`, and `(A OR B) AND C` would break.
            other = operand()
            result = [left | right for left in result for right in other]
        return result

    def either():
        nonlocal position
        result = conjunction()
        while position < len(tokens) and tokens[position] == "OR":
            position += 1
            result += conjunction()
        return result

    return either()


def elect(name, expression):
    """(elected licence set, exemption reason), or (None, reason) when rejected."""
    slint_stack = name.startswith("slint") or name.startswith("i-slint")

    def allowed(licence):
        return licence in ALLOWED or (licence in SLINT_IDENTIFIERS and slint_stack)

    try:
        options = alternatives(expression)
    except IndexError:
        return None, f"无法解析的 SPDX 表达式 `{expression}`"
    acceptable = [option for option in options if all(allowed(licence) for licence in option)]
    if acceptable:
        # Fewest licences first, then expression order: deterministic, and it
        # prefers a single-licence branch over a dual-licence one.
        return min(acceptable, key=lambda option: (len(option), options.index(option))), None
    for option in options:
        excused = {l for l in option if l not in ALLOWED and l in REVIEWED and name in REVIEWED[l][0]}
        if excused and all(licence in ALLOWED or licence in excused for licence in option):
            return option, "书面豁免：" + "；".join(REVIEWED[licence][1] for licence in sorted(excused))
    flat = {licence for option in options for licence in option}
    if any(licence.startswith(("GPL-", "AGPL-")) for licence in flat):
        return None, f"copyleft（GPL/AGPL）具有传染性，禁止依赖：`{expression}`"
    if any(licence in SLINT_IDENTIFIERS for licence in flat):
        return None, f"`LicenseRef-Slint-*` 只适用于 Slint 自身的 crate：`{expression}`"
    return None, f"许可证不在允许清单内：`{expression}`"


# ── dependency graph ─────────────────────────────────────────────────────────


def shipping_closure(document, names):
    """Crates compiled into the shipped library: `ime-*` members, normal edges.

    Dev and build dependencies are not distributed with `librspinyin.so`, and
    `xtask` is a build tool, so neither is in the closure the allowlist governs.
    """
    roots = {member for member in document["workspace_members"]
             if names.get(member, "").startswith("ime-")}
    nodes = {node["id"]: node for node in document["resolve"]["nodes"]}
    closure, frontier = set(roots), list(roots)
    while frontier:
        for dependency in nodes.get(frontier.pop(), {}).get("deps", []):
            kinds = {kind.get("kind") for kind in dependency.get("dep_kinds", [])}
            if kinds <= {"dev", "build"} and kinds:
                continue
            if dependency["pkg"] not in closure:
                closure.add(dependency["pkg"])
                frontier.append(dependency["pkg"])
    return closure


def classify_dependencies(document):
    """(inventory rows, shipped ids). Every package is inventoried, none skipped."""
    packages = sorted(document["packages"], key=lambda p: (p["name"], p["version"]))
    members = set(document["workspace_members"])
    shipped = shipping_closure(document, {p["id"]: p["name"] for p in packages})
    inventory = []
    for package in packages:
        name, identifier = package["name"], package["id"]
        ships = identifier in shipped
        expression = package.get("license") or package.get("license_file") or ""
        if identifier in members:
            inventory.append((name, package, "MIT OR Apache-2.0", "MIT", "第一方", ships, ""))
            continue
        if not expression:
            report(f"{name} {package['version']}: no `license` field; an unattributed dependency "
                   f"cannot be redistributed (replace it or record the licence)")
            inventory.append((name, package, UNKNOWN, UNKNOWN, "禁止", ships, "缺少许可证字段"))
            continue
        elected, reason = elect(name, expression)
        licence = " AND ".join(sorted(elected)) if elected else UNKNOWN
        if elected is None:
            report(f"{name} {package['version']}: {reason}{'（随包发布）' if ships else ''}")
            inventory.append((name, package, expression, licence, "禁止", ships, reason))
        elif reason is not None and ships:
            report(f"{name} {package['version']}: 书面豁免的许可证不能覆盖随包发布的代码"
                   f"（`{expression}`）；替换该依赖或改用允许清单内的许可证")
            inventory.append((name, package, expression, licence, "禁止", ships,
                              "豁免范围不覆盖随包发布的代码"))
        else:
            inventory.append((name, package, expression, licence,
                              "豁免" if reason else "允许", ships, reason or ""))
    return inventory, shipped


# ── dictionary sources ───────────────────────────────────────────────────────


def load_sources():
    if not sources_path.is_file():
        report(f"{sources_path}: allowlist not found")
        return []
    try:
        entries = tomllib.loads(sources_path.read_text(encoding="utf-8")).get("source", [])
    except (OSError, tomllib.TOMLDecodeError) as error:
        report(f"{sources_path}: invalid TOML: {error}")
        return []
    if not entries:
        report(f"{sources_path}: declares no [[source]]")
    return entries


def check_sources(entries):
    """Register every source and count the non-permissive ones."""
    declared = {str(entry.get("id", "")): entry for entry in entries}
    copyleft = 0
    for identifier, entry in sorted(declared.items()):
        if entry.get("permissive") is not True:
            copyleft += 1
            report(f"{sources_path}: `{identifier}` is not marked permissive; ADR-0000 admits "
                   f"permissive sources only (add an ADR before admitting a copyleft source)")
        spdx = str(entry.get("spdx", ""))
        unknown = sorted(token for token in re.split(r"[\s()]+", spdx)
                         if token and token not in ("OR", "AND", "WITH", *PERMISSIVE_SOURCES))
        if unknown:
            report(f"{sources_path}: `{identifier}` declares `{spdx}`（{', '.join(unknown)}）；"
                   f"ADR-0000 要求宽松许可")
    if raw_dir.is_dir():
        for path in sorted(raw_dir.iterdir()):
            if path.is_file() and path.suffix == ".tsv" and path.stem not in declared:
                report(f"{path}: source `{path.stem}` is not registered in {sources_path}")
    return copyleft


# ── Slint obligations ────────────────────────────────────────────────────────


def slint_licence(document):
    """(path, text) of the licence shipped inside the Slint package."""
    for package in document["packages"]:
        if package["name"] == "slint":
            directory = Path(package["manifest_path"]).parent / "LICENSES"
            for name in ("LicenseRef-Slint-Royalty-free-2.0.md",
                         "LicenseRef-Slint-Royalty-free-2.0.txt"):
                if (directory / name).is_file():
                    return directory / name, (directory / name).read_text(encoding="utf-8")
    return None, None


def paragraph_with(text, anchor):
    for paragraph in re.split(r"\n\s*\n", text):
        if anchor.lower() in paragraph.lower():
            return " ".join(paragraph.split())
    return None


def public_page():
    """The project's public web page: the manifest's `repository` field, or ""."""
    try:
        manifest = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))
        return str(manifest["workspace"]["package"]["repository"])
    except (OSError, KeyError, TypeError, tomllib.TOMLDecodeError):
        return ""


def readme_conclusion():
    """OB-1: the attribution badge on a public page (ADR-0000 §2(b)).

    The badge has to sit above the first section heading. The licence asks for an
    attribution a visitor cannot miss, and a badge below three screens of prose is
    one a visitor has to look for -- so where it sits is part of the assertion, not
    a matter of taste.
    """
    failures = []
    for name in ("README.md", "README.zh.md"):
        if not (readme_dir / name).is_file():
            failures.append(f"{name} 不存在")
            continue
        text = (readme_dir / name).read_text(encoding="utf-8", errors="replace")
        if not re.search(r"https?://(www\.)?slint\.dev", text, re.IGNORECASE):
            failures.append(f"{name} 缺少 https://slint.dev 链接")
            continue
        preamble = text.split("\n## ", 1)[0]
        if not re.search(r"!\[[^\]]*\]\([^)]*slint[^)]*\)|<img[^>]*slint", preamble, re.I):
            failures.append(f"{name} 的第一个二级标题之前没有 Slint 归属徽章")
    for detail in failures:
        report(f"{detail}；OB-1 要求 README 首屏含 Slint 徽章与 https://slint.dev 链接"
               f"（ADR-0000 §2(b)）")
    if failures:
        return "未达成：" + "；".join(failures)
    page = public_page()
    where = f"；公开页面 {page}" if page else ""
    return ("已达成（README.md 与 README.zh.md 的第一个二级标题之前均含 Slint 归属徽章，"
            f"链接 https://slint.dev{where}）")


def slint_library_conclusion():
    """OB-2: no standalone Slint library may be produced or packaged."""
    pattern = re.compile(r"lib(i-)?slint[^\s/]*\.so", re.IGNORECASE)
    packaging = root / "packaging"
    found = [path for path in sorted(packaging.rglob("*")) if packaging.is_dir() and path.is_file()
             and (re.search(r"\.so(\.\d+)*$", path.name)
                  or pattern.search(path.read_text(encoding="utf-8", errors="ignore")))]
    built = False
    for directory in [root / "target" / "release", *sorted(root.glob("target/*/release"))]:
        if directory.is_dir():
            built = True
            found += [path for path in sorted(directory.glob("*.so*")) if "slint" in path.name.lower()]
    for path in found:
        report(f"{path}: 独立的 Slint 库文件；OB-2 只允许分发 `librspinyin.so`")
    if found:
        return "未达成：" + "、".join(str(path.relative_to(root)) for path in found)
    if built:
        return "已达成（`packaging/` 与构建产物中均无独立 Slint 库）"
    return "已达成（`packaging/` 中无独立 Slint 库；构建产物尚未生成，未核对 target/）"


def git_status(pathspec):
    """`git status --porcelain` lines for `pathspec`, or None when git cannot answer.

    None is not an empty list: a tree outside a git work tree, or a machine without
    git, has not been checked, and reporting that as "clean" would turn an unchecked
    claim into a passing one. The work tree has to be rooted exactly at `root`, so a
    scratch copy of the repository that happens to sit inside another checkout
    reports "not checked" rather than the enclosing repository's state.
    """
    try:
        toplevel = subprocess.run(["git", "rev-parse", "--show-toplevel"],
                                  cwd=root, capture_output=True, text=True, check=False)
        if toplevel.returncode != 0 or Path(toplevel.stdout.strip()).resolve() != root.resolve():
            return None
        completed = subprocess.run(["git", "status", "--porcelain", "--", pathspec],
                                   cwd=root, capture_output=True, text=True, check=False)
    except OSError:
        return None
    if completed.returncode != 0:
        return None
    return [line for line in completed.stdout.splitlines() if line.strip()]


def licences_directory_conclusion(licence_path, licence_text):
    """OB-5: the licence texts under `LICENSES/` are present and verbatim.

    The previous form of this check ran `git status --porcelain -- *LICENSES*` and
    asserted the output was empty. That is vacuously true when no `LICENSES/`
    directory is tracked at all -- which was the state the repository was in -- so
    the check could not report the dangling reference `docs/dev/NOTICE` carried, and
    it said nothing about the content of the files it named. Asserting presence and
    content is what turns it from a formality into a gate.

    The copies are compared against the licence texts shipped inside the Slint
    package, read through `cargo metadata`, rather than against a digest registered
    by hand: a hand-registered value has to be updated on every Slint upgrade and can
    drift unnoticed, while the package's own text is what the obligation is about.
    Only the names a release actually points at are compared -- the ones
    `docs/dev/NOTICE` references and the licence of the component whose terms this
    project satisfies by shipping its text -- so an extra file nothing references is
    left alone. Trailing newlines are ignored, because the universal-newline decoding
    used here already normalises line endings and a trailing blank line carries no
    licence content; every other character is compared exactly.
    """
    directory = root / "LICENSES"
    if not directory.is_dir():
        report("LICENSES/ 不存在；docs/dev/NOTICE 引用了其中的许可原文，"
               "发布产物会携带一个悬空引用")
        return "未达成：LICENSES/ 不存在"
    if not any(path.is_file() for path in directory.iterdir()):
        report("LICENSES/ 为空；许可原文必须随发布产物提供")
        return "未达成：LICENSES/ 为空"
    if licence_path is None or not licence_text:
        report("无法读取 Slint 发行包内的许可原文（先运行 `cargo fetch`）；"
               "LICENSES/ 下的副本无从比对")
        return "未达成：无法比对许可原文"

    notice = NOTICE_DOC.read_text(encoding="utf-8") if NOTICE_DOC.is_file() else ""
    required = set(re.findall(r"`LICENSES/([^\s`]+)`", notice))
    required.add(Path(licence_path).name)
    package_dir = Path(licence_path).parent
    failures = []
    for name in sorted(required):
        vendored, upstream = directory / name, package_dir / name
        if not vendored.is_file():
            failures.append(f"LICENSES/{name} 不存在；该许可原文必须随仓库提供，"
                            f"发布产物引用的是仓库内的副本")
        elif not upstream.is_file():
            failures.append(f"LICENSES/{name} 在 Slint 发行包内没有同名文件，无法核对原文")
        else:
            actual = vendored.read_text(encoding="utf-8", errors="replace")
            expected = upstream.read_text(encoding="utf-8", errors="replace")
            if actual.rstrip("\n") != expected.rstrip("\n"):
                failures.append(
                    f"LICENSES/{name} 与 Slint 发行包内的原文不一致（副本 SHA256 "
                    f"{hashlib.sha256(actual.encode('utf-8')).hexdigest()}，原文 SHA256 "
                    f"{hashlib.sha256(expected.encode('utf-8')).hexdigest()}）")
    for detail in failures:
        report(f"{detail}；许可原文必须逐字复制，不得删改或节选")
    if failures:
        return "未达成：" + "；".join(failures)

    checked = "、".join(f"`{name}`" for name in sorted(required))
    dirty = git_status("LICENSES/")
    if dirty is None:
        return (f"已达成（LICENSES/ 下 {checked} 与 Slint 发行包内的原文一致；"
                f"不在 git 工作树中，未核对本地改动）")
    if dirty:
        report("LICENSES/ 下有未提交的改动：" + "；".join(dirty))
        return "未达成：LICENSES/ 下有本地改动"
    return f"已达成（LICENSES/ 下 {checked} 与 Slint 发行包内的原文一致，且无本地改动）"


def check_project_licence_files():
    """The two licence texts the workspace manifest publishes under must be present.

    `Cargo.toml` declares `MIT OR Apache-2.0` for the workspace, and a release
    archive, a `.deb` and an `.rpm` all carry both texts. A manifest that declares a
    licence the repository does not ship is a release blocker, so presence is
    asserted -- and a phrase from each file is asserted too, because presence alone
    accepts the two having been swapped, which neither a reader nor a packager would
    notice.
    """
    for name, phrases in PROJECT_LICENCES:
        path = root / name
        if not path.is_file():
            report(f"{name} 不存在；`Cargo.toml` 声明的 `MIT OR Apache-2.0` 要求两份许可原文"
                   f"随仓库与发布产物提供")
            continue
        text = path.read_text(encoding="utf-8", errors="replace")
        for phrase in phrases:
            if phrase not in text:
                report(f"{name} 不含 `{phrase}`；该文件必须是它对应的那份许可原文，"
                       f"而不是另一份许可或一个空壳文件")


def obligation_rows(licence_path, licence_text, recorded_hash):
    """(rows, digest): the six obligations, each with its clause and its verdict."""
    if not (root / "scripts" / "check-slint-leak.sh").is_file():
        report("scripts/check-slint-leak.sh 不存在；OB-4 的强制门禁缺失")
    digest = hashlib.sha256(licence_text.encode("utf-8")).hexdigest() if licence_text else ""
    if not licence_text:
        report("无法读取 Slint 发行包内的许可原文（先运行 `cargo fetch`）；OB-1~OB-6 必须对照原文"
               "复核，不得引用二手解读")
    elif recorded_hash and digest != recorded_hash:
        report(f"Slint 许可原文的 SHA256 为 {digest}，与 licenses.md 登记的 {recorded_hash} 不一致；"
               f"复核后运行 `--write` 更新登记值")
    results = {"OB-1": readme_conclusion(), "OB-2": slint_library_conclusion(),
               "OB-3": "已达成（第 6 节声明）", "OB-4": "已达成（`scripts/check-slint-leak.sh` 强制）",
               "OB-5": licences_directory_conclusion(licence_path, licence_text),
               "OB-6": "已达成（第 7 节转述）"}
    rows = []
    for identifier, title, anchor, method in OBLIGATIONS:
        excerpt = paragraph_with(licence_text, anchor) if licence_text else None
        if not licence_text:
            results[identifier] = "未达成：无法读取许可原文"
        elif excerpt is None:
            report(f"Slint 许可原文中找不到 {identifier} 的依据句（`{anchor}`）；条款可能已变更，"
                   f"需重新复核")
            results[identifier] = "未达成：原文中找不到依据句"
        rows.append((identifier, title, excerpt or "—", method, results[identifier]))
    return rows, digest


# ── generated blocks ─────────────────────────────────────────────────────────


def render_inventory(inventory):
    first = [row for row in inventory if row[4] == "第一方"]
    third = [row for row in inventory if row[4] != "第一方"]
    groups = {}
    for row in third:
        groups.setdefault(row[2], []).append(row)
    key = lambda row: (row[0], row[1]["version"])
    lines = [
        BLOCKS["inventory"][0],
        "<!-- 由 `scripts/gen-licenses.sh --write` 生成；请勿手工编辑本块 -->",
        "",
        f"- 包总数：**{len(inventory)}**（与 `cargo metadata` 报告一致，含工作区自身 {len(first)} 个）",
        f"- 第三方依赖：**{len(third)}**；随 `librspinyin.so` 发布的 Linux 构建闭包："
        f"**{sum(1 for row in inventory if row[5])}** 个包（不含 dev / build 依赖与 `xtask`）",
        f"- 禁止的许可证：**{sum(1 for row in third if row[4] == '禁止')}**（`GPL-*`、`AGPL-*`、"
        f"未知/缺失一律禁止）；书面豁免：**{sum(1 for row in third if row[4] == '豁免')}**",
        "",
        "### 第一方（工作区成员，`MIT OR Apache-2.0`）", "",
        "| 包 | 版本 | 声明许可证 | 主张分支 | 结论 |", "|---|---|---|---|---|",
    ]
    lines += [f"| `{name}` | {package['version']} | `{expression}` | `{licence}` | 本项目自有 |"
              for name, package, expression, licence, _, _, _ in first]
    lines += ["", f"### 第三方（按声明表达式分组，共 {len(third)} 个包）", "",
              "| 声明表达式 | 数量 | 主张分支 | 结论 | 包 |", "|---|---|---|---|---|"]
    for expression in sorted(groups):
        rows = sorted(groups[expression], key=key)
        verdict = rows[0][4] + (f"（{rows[0][6]}）" if rows[0][6] and rows[0][4] != "允许" else "")
        lines.append(f"| `{expression}` | {len(rows)} | `{rows[0][3]}` | {verdict} | "
                     f"{'、'.join(f'`{row[0]}`' for row in rows)} |")
    lines += ["", BLOCKS["inventory"][1]]
    return "\n".join(lines)


def render_sources(entries, copyleft):
    rows = [f"| `{e.get('id', '')}` | {e.get('kind', 'upstream')} | {e.get('layer', '')} | "
            f"{e.get('license', '')} | `{e.get('spdx', '')}` | {e.get('retrieved', '') or '—'} | "
            f"{e.get('sha256', '') or '（衍生数据，不固定哈希）'} | {str(e.get('permissive')).lower()} |"
            for e in entries]
    return "\n".join([
        BLOCKS["sources"][0], "<!-- 由 `scripts/gen-licenses.sh --write` 生成；请勿手工编辑本块 -->",
        "", f"- 来源数：**{len(entries)}**；`permissive = false` 的来源数：**{copyleft}**"
        f"（ADR-0000 决策 2 要求为 0）", "",
        "| id | kind | layer | 许可证 | SPDX | 获取日期 | SHA256 | permissive |",
        "|---|---|---|---|---|---|---|---|", *rows, "", BLOCKS["sources"][1]])


def render_obligations(rows, licence_path, digest):
    return "\n".join([
        BLOCKS["obligations"][0], "<!-- 由 `scripts/gen-licenses.sh --write` 生成；请勿手工编辑本块 -->",
        "",
        (f"- 许可原文：`{licence_path}`（Slint 发行包内，随 `cargo metadata` 展开）"
         if licence_path else "- 许可原文：**未找到**（先运行 `cargo fetch`）"),
        f"- 原文 SHA256：`{digest or '—'}`（登记值变化即说明上游条款变动，必须重新复核）", "",
        "| 义务 | 条款依据（许可原文摘录） | 核对方式 | 复核结论 |", "|---|---|---|---|",
        *[f"| `{identifier}` {title} | {excerpt} | {method} | {conclusion} |"
          for identifier, title, excerpt, method, conclusion in rows],
        "", BLOCKS["obligations"][1]])


def render_notice(entries, licence_text, slint_version, digest):
    usage = {"pinyin-data": "L1 单字读音（主源）", "unihan": "L1 单字读音交叉校验",
             "jieba-dict": "L2 词条与 L4 词频", "base": "本项目生成的衍生词库",
             "polyphone": "本项目生成的多音字权重校正表"}
    rows = [f"| `{e.get('id', '')}`（{e.get('url', '')}） | {e.get('license', '')} "
            f"| {usage.get(str(e.get('id', '')), e.get('layer', ''))} |" for e in entries]
    return "\n".join([
        BLOCKS["notice"][0], "<!-- 由 `scripts/gen-licenses.sh --write` 生成；请勿手工编辑本块 -->",
        "",
        f"Slint {slint_version}（SixtyFPS GmbH，<https://slint.dev>）依据 **Slint Royalty-free",
        "Desktop, Mobile, and Web Applications License, Version 2.0**（SPDX：",
        "`LicenseRef-Slint-Royalty-free-2.0`）使用。归属声明：本程序使用 Slint 构建。", "",
        f"随本发布产物一同提供的许可原文（`LICENSES/LicenseRef-Slint-Royalty-free-2.0.md`，"
        f"SHA256 `{digest or '—'}`）：", "",
        (licence_text.strip() if licence_text
         else "（许可原文未找到：先运行 `cargo fetch` 再生成本块）"),
        "", "词库数据来源：", "| 来源 | 许可证 | 用途 |", "|---|---|---|", *rows,
        "", BLOCKS["notice"][1]])


def replace_block(text, block, body):
    start, end = block
    if start not in text or end not in text:
        print(f"gen-licenses: FAIL\n  - 文档缺少生成块标记 `{start}`；`docs/dev/licenses.md` 与"
              f"`docs/dev/NOTICE` 是手工维护的合规文档，生成块必须保留", file=sys.stderr)
        sys.exit(2)
    return text[: text.find(start)] + body + text[text.find(end) + len(end):]


def verify_documents(licences, notice, inventory, entries):
    """Check that the committed documents still register everything derived here."""
    if licences is None or notice is None:
        report("docs/dev/licenses.md 或 docs/dev/NOTICE 不存在；运行 `--write` 生成它们")
        return
    blocks = {}
    for name, (start, end) in BLOCKS.items():
        text = notice if name == "notice" else licences
        if start not in text or end not in text:
            report(f"docs/dev/{'NOTICE' if name == 'notice' else 'licenses.md'} "
                   f"缺少生成块标记 `{start}`")
        else:
            blocks[name] = text.split(start, 1)[1].split(end, 1)[0]
    absent = sorted({row[0] for row in inventory}
                    - set(re.findall(r"`([A-Za-z0-9_.+-]+)`", blocks.get("inventory", ""))))
    if absent:
        report(f"docs/dev/licenses.md 未登记 {len(absent)} 个包：{'、'.join(absent[:10])}"
               f"{' …' if len(absent) > 10 else ''}；运行 `bash scripts/gen-licenses.sh --write`")
    count = re.search(r"包总数：\*\*(\d+)\*\*", blocks.get("inventory", ""))
    if not count or int(count.group(1)) != len(inventory):
        report(f"docs/dev/licenses.md 的包总数与 `cargo metadata` 不一致（登记 "
               f"{count.group(1) if count else '缺失'}，实际 {len(inventory)}）")
    for entry in entries:
        identifier = str(entry.get("id", ""))
        for name, label in (("sources", "licenses.md"), ("notice", "NOTICE")):
            if identifier and f"`{identifier}`" not in blocks.get(name, ""):
                report(f"docs/dev/{label} 未登记词源 `{identifier}`")
    for identifier, _, _, _ in OBLIGATIONS:
        if f"`{identifier}`" not in blocks.get("obligations", ""):
            report(f"docs/dev/licenses.md 未登记义务 `{identifier}`")
    for text, label, statements in (
        (licences, "licenses.md", ("嵌入式", "GPL-3.0", "商业许可", "按现状", "无担保", "check-slint-leak.sh")),
        (notice, "NOTICE", ("fcitx5", "LGPL-2.1", "LicenseRef-Slint-Royalty-free-2.0")),
    ):
        for statement in statements:
            if statement.lower() not in text.lower():
                report(f"docs/dev/{label} 缺少声明文本：{statement}")


# ── self-test injection ──────────────────────────────────────────────────────


def inject(document, case):
    """Add one package to the real graph, for the self-test only.

    The crate becomes a dependency of a workspace member unless the case is about
    an exemption that has to stay outside the build closure.
    """
    name, licence = {
        "gpl": ("copyleft-lib", "GPL-3.0-only"),
        "agpl": ("network-lib", "AGPL-3.0-or-later"),
        "unknown": ("mystery-lib", "WTFPL"),
        "missing": ("no-licence", None),
        "slint-ref": ("fake-ui", "LicenseRef-Slint-Royalty-free-2.0"),
        "exempt-shipped": ("clipboard-win", "BSL-1.0"),
        "exempt-unnamed": ("other-win", "BSL-1.0"),
    }[case]
    identifier = f"registry+https://example.invalid/index#{name}@0.0.0"
    document["packages"].append({
        "id": identifier, "name": name, "version": "0.0.0", "license": licence,
        "source": "registry+https://example.invalid/index", "dependencies": [],
        "targets": [], "features": {}, "manifest_path": "/nonexistent/Cargo.toml",
    })
    nodes = {node["id"]: node for node in document["resolve"]["nodes"]}
    nodes[identifier] = {"id": identifier, "features": [], "deps": []}
    if case != "exempt-unnamed":
        nodes[sorted(document["workspace_members"])[0]].setdefault("deps", []).append({
            "name": name.replace("-", "_"), "pkg": identifier,
            "dep_kinds": [{"kind": None, "target": None}],
        })


def main():
    try:
        document = json.loads(metadata_path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as error:
        sys.exit(f"gen-licenses: cannot parse cargo metadata: {error}")
    if case:
        inject(document, case)
    inventory, _ = classify_dependencies(document)
    if mode == "deps-only":
        return finish(f"gen-licenses: PASS ({len(inventory)} packages classified)")

    entries = load_sources()
    copyleft = check_sources(entries)
    check_project_licence_files()
    licence_path, licence_text = slint_licence(document)
    licences = LICENSES_DOC.read_text(encoding="utf-8") if LICENSES_DOC.is_file() else None
    notice = NOTICE_DOC.read_text(encoding="utf-8") if NOTICE_DOC.is_file() else None
    recorded = re.search(r"原文 SHA256：`([0-9a-f]{64})`", licences or "")
    rows, digest = obligation_rows(licence_path, licence_text, recorded.group(1) if recorded else "")
    version = next((p["version"] for p in document["packages"] if p["name"] == "slint"), "unknown")
    if mode != "write":
        verify_documents(licences, notice, inventory, entries)
    elif licences is None or notice is None:
        print("gen-licenses: FAIL\n  - docs/dev/licenses.md 或 docs/dev/NOTICE 不存在；它们是手工"
              "维护的合规文档，本脚本只刷新其中的生成块", file=sys.stderr)
        return 2
    else:
        for block, body in ((BLOCKS["inventory"], render_inventory(inventory)),
                            (BLOCKS["sources"], render_sources(entries, copyleft)),
                            (BLOCKS["obligations"],
                             render_obligations(rows, licence_path, digest))):
            licences = replace_block(licences, block, body)
        LICENSES_DOC.write_text(licences, encoding="utf-8")
        NOTICE_DOC.write_text(replace_block(notice, BLOCKS["notice"], render_notice(
            entries, licence_text, version, digest)), encoding="utf-8")

    third = sum(1 for row in inventory if row[4] != "第一方")
    return finish(f"gen-licenses: PASS ({len(inventory)} packages: {third} third-party, "
                  f"{len(entries)} dictionary sources, OB-1..OB-6 recorded)")


sys.exit(main())
PY
}

# ── self-test ────────────────────────────────────────────────────────────────

expect_status() {
    # expect_status EXPECTED_STATUS EXPECTED_TEXT DESCRIPTION MODE [CASE]
    local expected="$1" text="$2" description="$3" status=0 output=""
    shift 3
    if output="$(run_audit "$@" 2>&1)"; then
        status=0
    else
        status=$?
    fi
    if [ "$status" -ne "$expected" ]; then
        echo "gen-licenses: self-test FAILED - $description exited $status, expected $expected" >&2
        echo "$output" >&2
        return 1
    fi
    case "$output" in
        *"$text"*) return 0 ;;
    esac
    echo "gen-licenses: self-test FAILED - $description reported the wrong reason; expected: $text" >&2
    echo "$output" >&2
    return 1
}

write_fixture_root() {
    # A scratch tree carrying copies of the committed documents plus what they
    # cannot supply themselves, so the document pipeline can be exercised
    # without writing to the repository. The licence copies come from the
    # repository because the check compares them against the text inside the
    # Slint package, which is read from the real metadata document.
    mkdir -p "$1/docs/dev" "$1/data/raw" "$1/scripts" "$1/LICENSES"
    cp -- "$script_root/docs/dev/licenses.md" "$script_root/docs/dev/NOTICE" "$1/docs/dev/"
    cp -- "$script_root/LICENSES/LicenseRef-Slint-Royalty-free-2.0.md" "$1/LICENSES/"
    : >"$1/scripts/check-slint-leak.sh"
    printf 'Apache License\nVersion 2.0, January 2004\n' >"$1/LICENSE-APACHE"
    printf 'MIT License\n\nPermission is hereby granted, free of charge\n' >"$1/LICENSE-MIT"
    printf '[[source]]\nid = "alpha"\nkind = "upstream"\nlayer = "L1"\nlicense = "MIT"\nspdx = "MIT"\npermissive = true\n' \
        >"$1/data/sources.toml"
    printf '# rspinyin\n\n[![Made with Slint](https://img.shields.io/badge/Made%%20with-Slint-blue)](https://slint.dev)\n' \
        >"$1/README.md"
    cp -- "$1/README.md" "$1/README.zh.md"
}

run_self_test() {
    scratch="$(mktemp -d "${TMPDIR:-/tmp}/rspinyin-licences.XXXXXX")"
    local real_root="$root" fixture="$scratch/metadata.json" triple
    root="$scratch/root"
    write_fixture_root "$root"
    # The two paths derived from `root` are recomputed here, and that is the whole point of
    # this pair of lines. They are normally filled in once, during argument parsing, from
    # the real repository root -- which happens *before* this function repoints `root` at the
    # fixture. Without the recomputation every case below would still read the real
    # `data/sources.toml`, so the injected allowlist entries would be invisible and the
    # non-permissive case would pass silently while asserting the opposite.
    sources_file="$root/data/sources.toml"
    raw_dir="$root/data/raw"
    metadata_file="$fixture"
    triple="$(python3 -c 'import platform; print(platform.machine() + "-unknown-linux-gnu")')"
    (cd -- "$real_root" && cargo metadata --format-version 1 --all-features --locked \
        --filter-platform "$triple") >"$fixture" ||
        die "cannot capture the workspace metadata for the self-test"

    echo "gen-licenses: self-test (injecting packages into the real resolve graph)"
    expect_status 0 "PASS" "the real dependency graph" deps-only
    expect_status 1 "copyleft（GPL/AGPL）" "an injected GPL dependency" deps-only gpl
    expect_status 1 "copyleft（GPL/AGPL）" "an injected AGPL dependency" deps-only agpl
    expect_status 1 "许可证不在允许清单内" "an injected unknown licence" deps-only unknown
    expect_status 1 "no \`license\` field" "a crate without a licence" deps-only missing
    expect_status 1 "只适用于 Slint 自身的 crate" "a Slint ref outside the stack" deps-only slint-ref
    expect_status 1 "书面豁免的许可证不能覆盖随包发布的代码" \
        "an exemption reaching the shipped closure" deps-only exempt-shipped
    expect_status 1 "许可证不在允许清单内" "an unnamed reviewed licence" deps-only exempt-unnamed

    echo "gen-licenses: self-test (dictionary source rules)"
    printf '[[source]]\nid = "alpha"\npermissive = false\n' >"$root/data/sources.toml"
    expect_status 1 "not marked permissive" "a non-permissive allowlist entry" write
    printf '[[source]]\nid = "alpha"\npermissive = true\nspdx = "GPL-3.0-only"\n' \
        >"$root/data/sources.toml"
    expect_status 1 "ADR-0000 要求宽松许可" "a copyleft source licence" write
    printf '[[source]]\nid = "alpha"\npermissive = true\nspdx = "MIT"\n' >"$root/data/sources.toml"
    printf '词\t拼音\n未登记\twei4ji4lu4\n' >"$root/data/raw/rogue.tsv"
    expect_status 1 "not registered" "an unregistered raw source" write
    rm -f -- "$root/data/raw/rogue.tsv"

    echo "gen-licenses: self-test (document pipeline)"
    printf '# rspinyin\n\nrspinyin 是一个离线优先的 Linux 中文拼音输入法。\n' >"$root/README.zh.md"
    expect_status 1 "README.zh.md" "a README without the attribution badge" write
    cp -- "$root/README.md" "$root/README.zh.md"
    expect_status 0 "PASS" "the compliant scratch tree" write
    expect_status 0 "PASS" "the documents just written" check
    printf '# rspinyin\n' >"$root/README.md"
    expect_status 1 "README.md" "a README whose badge was removed" check
    cp -- "$root/README.zh.md" "$root/README.md"
    printf '\n[[source]]\nid = "gamma"\nkind = "upstream"\nlicense = "MIT"\nspdx = "MIT"\npermissive = true\n' \
        >>"$root/data/sources.toml"
    expect_status 1 "未登记词源" "a source added without regenerating the document" check
    expect_status 0 "PASS" "the tree after regeneration" write

    echo "gen-licenses: self-test (the project's licence files and the LICENSES/ guard)"
    mv -- "$root/LICENSE-MIT" "$root/LICENSE-MIT.absent"
    expect_status 1 "LICENSE-MIT 不存在" "a tree without the MIT licence text" check
    mv -- "$root/LICENSE-MIT.absent" "$root/LICENSE-MIT"
    mv -- "$root/LICENSES" "$root/LICENSES.absent"
    expect_status 1 "LICENSES/ 不存在" "a tree without LICENSES/" check
    mv -- "$root/LICENSES.absent" "$root/LICENSES"
    expect_status 0 "PASS" "the restored LICENSES/" check
    cp -- "$root/docs/dev/NOTICE" "$root/NOTICE.saved"
    printf '\n见 `LICENSES/LicenseRef-Absent-1.0.md`。\n' >>"$root/docs/dev/NOTICE"
    expect_status 1 "LICENSES/LicenseRef-Absent-1.0.md 不存在" \
        "a NOTICE pointing at a licence text the repository does not carry" check
    mv -- "$root/NOTICE.saved" "$root/docs/dev/NOTICE"
    printf '\n## Tampered\n' >>"$root/LICENSES/LicenseRef-Slint-Royalty-free-2.0.md"
    expect_status 1 "与 Slint 发行包内的原文不一致" \
        "a LICENSES/ copy whose text was altered" check
    echo "gen-licenses: self-test PASS (8 classifier cases, 3 source cases, licence files and LICENSES/ guard, document pipeline)"
}

# --check-links: OB-1 also asks that the attribution link answer HTTP 200. The
# probe is opt-in because it is the only network access in this repository's
# tooling, and the CI job is offline by design.
[ "$check_links" -eq 0 ] || python3 -c 'import sys, urllib.request
sys.exit(0 if urllib.request.urlopen("https://slint.dev", timeout=10).status == 200 else 1)' \
    || die "https://slint.dev did not answer HTTP 200; OB-1 needs the attribution link to work"

if [ "$mode" = "self-test" ]; then
    run_self_test
    exit 0
fi

capture_metadata
run_audit "$mode"
