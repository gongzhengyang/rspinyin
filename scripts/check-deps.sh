#!/usr/bin/env bash
#
# check-deps.sh - enforce the one-way crate layering of the workspace.
#
# Rules mechanised here (features.md 0.4 rules 1, 2 and the diagnostics-layer
# boundary of 2.1):
#
#   rule 1  ime-types <- ime-core <- ime-dict <- ime-config <- ime-ui <- ime-fcitx5
#           A crate may only depend on crates that sit strictly below it.
#   rule 2  ime-ui must never reach ime-core or ime-dict, directly or through
#           another crate; the UI consumes ime-types only.
#   rule 1  ime-types stays a dependency-free leaf.
#   leaf    ime-diag is a cross-cutting diagnostics leaf: its only internal
#           dependency may be ime-types, and no crate below ime-ui may pull it
#           in, so the pure engine never grows a diagnostics dependency.
#   dag     No cycle is allowed anywhere in the internal crate graph.
#
# The graph is read from `cargo metadata --format-version 1`, never from
# Cargo.lock, because the lockfile hides renamed dependencies and optional ones.
# `--all-features` makes the graph maximal and `--locked` keeps the audit from
# rewriting the committed lockfile.
#
# Exit codes: 0 = pass, 1 = violation, 2 = usage or environment error.
#
# Self-test: `--self-test` injects a violation into the real metadata document,
# asserts a non-zero exit, then removes it and asserts a zero exit.
#
# Requires python3 (present by default on every distro in the platform baseline).

set -euo pipefail

usage() {
    cat <<'USAGE'
usage: scripts/check-deps.sh [--self-test] [--metadata-file FILE] [--root DIR]

  --self-test            inject violations and assert they are detected
  --metadata-file FILE   analyse a captured `cargo metadata` document
  --root DIR             repository root (default: parent of this script)
USAGE
}

mode="check"
metadata_file=""
root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"

while [ $# -gt 0 ]; do
    case "$1" in
        --self-test) mode="self-test"; shift ;;
        --metadata-file)
            metadata_file="${2:?--metadata-file needs a path}"
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
            echo "check-deps: unknown argument: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

if ! command -v python3 >/dev/null 2>&1; then
    echo "check-deps: python3 is required to parse cargo metadata; install it and re-run" >&2
    exit 2
fi

metadata_tmp="$(mktemp "${TMPDIR:-/tmp}/rspinyin-deps.XXXXXX")"
trap 'rm -f -- "$metadata_tmp"' EXIT

if [ -n "$metadata_file" ]; then
    if ! cat -- "$metadata_file" >"$metadata_tmp"; then
        echo "check-deps: cannot read metadata file: $metadata_file" >&2
        exit 2
    fi
else
    if ! (cd -- "$root" && cargo metadata --format-version 1 --all-features --locked) >"$metadata_tmp"; then
        echo "check-deps: 'cargo metadata' failed under $root" >&2
        echo "check-deps: fix the workspace manifest (or the committed lockfile) and re-run" >&2
        exit 2
    fi
fi

# analyse MODE: run the layering audit over the captured metadata document.
# MODE is `none` for the real check; the self-test passes an injection mode that
# mutates the graph in memory before the very same audit runs.
analyse() {
    python3 - "$metadata_tmp" "$1" <<'PY'
import json
import sys
from pathlib import Path

metadata_path, mode = sys.argv[1], sys.argv[2]

# Layer order of features.md 0.4 rule 1: lower rank may be depended upon, never
# the other way round.
#
# `ime-fcitx5` and `ime-ui-addon` share rank 5. They are the two addon hosts: both sit
# directly above `ime-ui`, and neither may depend on the other. That is not a
# convenience — Fcitx5 `dlopen`s the two cdylibs independently as separate addons, and
# a dependency between them would be a link-time relationship the split exists to
# remove. Equal rank is what forbids it, since only strictly lower layers may be
# depended upon.
LAYERS = {
    "ime-types": 0,
    "ime-core": 1,
    "ime-dict": 2,
    "ime-config": 3,
    "ime-ui": 4,
    "ime-fcitx5": 5,
    "ime-ui-addon": 5,
}

# Cross-cutting crates with their own rule: the allowed internal dependencies.
LEAF_ONLY = {"ime-diag": {"ime-types"}}

# Crates of the pure engine path that must stay free of diagnostics IO.
NO_DIAGNOSTICS = ("ime-core", "ime-dict", "ime-config")

try:
    document = json.loads(Path(metadata_path).read_text(encoding="utf-8"))
except (OSError, ValueError) as error:
    sys.exit(f"check-deps: cannot parse cargo metadata: {error}")

names = {package["id"]: package["name"] for package in document["packages"]}
members = set(document["workspace_members"])
member_names = {names[member] for member in members}
by_name = {name: member for member, name in names.items() if member in members}
nodes = {node["id"]: node for node in document["resolve"]["nodes"]}


def add_edge(source, target):
    """Inject an internal dependency edge, for the self-test only."""
    if source not in by_name or target not in by_name:
        print(
            f"check-deps: self-test cannot inject {source} -> {target}: "
            f"crate is not in this workspace",
            file=sys.stderr,
        )
        sys.exit(2)
    node = nodes[by_name[source]]
    node.setdefault("deps", []).append(
        {
            "name": target.replace("-", "_"),
            "pkg": by_name[target],
            "dep_kinds": [{"kind": None, "target": None}],
        }
    )


if mode == "reverse-edge":
    add_edge("ime-core", "ime-dict")
elif mode == "ui-touches-engine":
    add_edge("ime-ui", "ime-core")
elif mode == "cycle":
    add_edge("ime-types", "ime-fcitx5")
elif mode == "diagnostics-in-engine":
    add_edge("ime-core", "ime-diag")
elif mode != "none":
    sys.exit(f"check-deps: unknown self-test mode: {mode}")

# Internal edges, keyed by crate name. Renames are handled because the package
# id, not the extern name, decides which crate an edge points at.
edges = {}
for member in sorted(members):
    edges[names[member]] = sorted(
        names[dependency["pkg"]]
        for dependency in nodes.get(member, {}).get("deps", [])
        if dependency["pkg"] in members
    )

problems = []

for crate in sorted(member_names):
    if crate.startswith("ime-") and crate not in LAYERS and crate not in LEAF_ONLY:
        problems.append(
            f"{crate}: internal crate is not classified here; add it to LAYERS "
            f"(or LEAF_ONLY) in scripts/check-deps.sh and document the intended direction"
        )

for crate, dependencies in edges.items():
    for dependency in dependencies:
        if crate in LAYERS and dependency in LAYERS and LAYERS[dependency] >= LAYERS[crate]:
            problems.append(
                f"{crate} depends on {dependency}, but rule 1 fixes the order "
                f"ime-types <- ime-core <- ime-dict <- ime-config <- ime-ui, with the "
                f"two addon hosts (ime-fcitx5, ime-ui-addon) at the same rank above it; "
                f"only strictly lower layers may be depended upon"
            )

for crate, allowed in LEAF_ONLY.items():
    for dependency in edges.get(crate, []):
        if dependency not in allowed:
            problems.append(
                f"{crate} depends on {dependency}, but it may only depend on "
                f"{', '.join(sorted(allowed))}; diagnostics is a cross-cutting leaf"
            )

for crate in NO_DIAGNOSTICS:
    if "ime-diag" in edges.get(crate, []):
        problems.append(
            f"{crate} depends on ime-diag; diagnostics belongs above the pure engine "
            f"(0.4 rule 4, 2.1 boundary 2). Route the probe through a trait instead"
        )

if edges.get("ime-types"):
    problems.append(
        f"ime-types depends on {', '.join(edges['ime-types'])}, but it is the "
        f"dependency-free contract leaf"
    )


def find_path(source, target):
    """Shortest internal dependency path source -> target, as crate names."""
    start, goal = by_name.get(source), by_name.get(target)
    if start is None or goal is None:
        return None
    queue = [[start]]
    seen = {start}
    while queue:
        path = queue.pop(0)
        for dependency in nodes.get(path[-1], {}).get("deps", []):
            if dependency["pkg"] not in members:
                continue
            if dependency["pkg"] == goal:
                return [names[step] for step in path] + [target]
            if dependency["pkg"] not in seen:
                seen.add(dependency["pkg"])
                queue.append(path + [dependency["pkg"]])
    return None


for target in ("ime-core", "ime-dict"):
    path = find_path("ime-ui", target)
    if path:
        problems.append(
            f"ime-ui reaches {target} via {' -> '.join(path)}; rule 2 forbids the UI "
            f"layer from touching the engine or the dictionary"
        )


def find_cycle():
    """Return one dependency cycle as crate names, or None."""
    grey, black = 1, 2
    colour = {}
    stack = []

    def visit(crate):
        colour[crate] = grey
        stack.append(crate)
        for dependency in edges.get(crate, []):
            if colour.get(dependency) == grey:
                return stack[stack.index(dependency):] + [dependency]
            if dependency not in colour:
                found = visit(dependency)
                if found:
                    return found
        colour[crate] = black
        stack.pop()
        return None

    for crate in sorted(edges):
        if crate not in colour:
            found = visit(crate)
            if found:
                return found
    return None


cycle = find_cycle()
if cycle:
    problems.append(f"dependency cycle: {' -> '.join(cycle)}")

if problems:
    print("check-deps: FAIL", file=sys.stderr)
    for problem in problems:
        print(f"  - {problem}", file=sys.stderr)
    sys.exit(1)

internal_edges = sum(len(dependencies) for dependencies in edges.values())
print(f"check-deps: PASS ({len(edges)} workspace crates, {internal_edges} internal edges)")
PY
}

expect_clean() {
    # expect_clean MODE DESCRIPTION
    local mode="$1" description="$2" status=0 output=""
    if output="$(analyse "$mode" 2>&1)"; then
        status=0
    else
        status=$?
    fi
    if [ "$status" -ne 0 ]; then
        echo "check-deps: self-test FAILED - $description exited $status, expected 0" >&2
        echo "$output" >&2
        return 1
    fi
    return 0
}

expect_violation() {
    # expect_violation MODE EXPECTED_TEXT DESCRIPTION
    local mode="$1" expected_text="$2" description="$3" status=0 output=""
    if output="$(analyse "$mode" 2>&1)"; then
        status=0
    else
        status=$?
    fi
    if [ "$status" -eq 0 ]; then
        echo "check-deps: self-test FAILED - $description was not detected" >&2
        echo "$output" >&2
        return 1
    fi
    case "$output" in
        *"$expected_text"*) ;;
        *)
            echo "check-deps: self-test FAILED - $description was reported for the wrong reason" >&2
            echo "check-deps: expected the report to mention: $expected_text" >&2
            echo "$output" >&2
            return 1
            ;;
    esac
    return 0
}

if [ "$mode" = "self-test" ]; then
    echo "check-deps: self-test (injecting violations into the real metadata document)"
    expect_clean none "the unmodified workspace"
    expect_violation reverse-edge "ime-core depends on ime-dict" "a reverse edge (ime-core -> ime-dict)"
    expect_violation ui-touches-engine "ime-ui reaches ime-core" "a rule 2 edge (ime-ui -> ime-core)"
    expect_violation cycle "dependency cycle:" "a dependency cycle (ime-types -> ime-fcitx5)"
    expect_violation diagnostics-in-engine "ime-core depends on ime-diag" \
        "a diagnostics edge (ime-core -> ime-diag)"
    expect_clean none "the workspace after removing the violations"
    echo "check-deps: self-test PASS (4 injected violations detected, clean graph accepted)"
    exit 0
fi

analyse none
