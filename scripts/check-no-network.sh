#!/usr/bin/env bash
#
# check-no-network.sh - assert that the dependency closure carries no network
# capability.
#
# "Zero network" is a product promise (features.md 0.4 rule 6, BUDGET-NET-01),
# not a preference: the plugin must behave byte-identically online and offline,
# so a network crate anywhere in the closure is a release blocker.
#
# Banned crates, verbatim from the architecture spec:
#
#   reqwest, hyper, ureq, curl, curl-sys, isahc, surf, awc, openssl (network
#   part), rustls, native-tls, tungstenite, quinn, zmq
#
# and, as feature-scoped bans, tokio and async-std when their network features
# are enabled.
#
# Matching is per dash/underscore-separated segment of the crate name, so
# variants such as `hyper-util`, `curl-sys`, `tokio-rustls`, `quinn-proto` and
# `native-tls` are all caught, while unrelated names (`surface-nets`) are not.
# `tls` is the single exception: it is matched only as a leading or trailing
# segment, because it abbreviates thread-local storage as well as transport
# security and would otherwise flag `scoped-tls-hkt`. See POSITIONAL_SEGMENTS.
# The OpenSSL stack is banned wholesale rather than by "network part": an
# offline input method has no legitimate use for it.
#
# The graph is read from `cargo metadata --format-version 1` with
# `--all-features`, never from Cargo.lock: the lockfile hides renamed and
# optional dependencies, and a crate enabled only by a feature is exactly the
# case this gate must catch.
#
# Exit codes: 0 = pass, 1 = violation, 2 = usage or environment error.
#
# Self-test: `--self-test` injects banned packages, a banned feature and a
# benign look-alike into the real metadata document and asserts the outcomes.
# It also snapshots the working tree before the injections and compares it
# afterwards, which is the assertion behind "the self-test has no side effects":
# the original design added a banned dependency to a real Cargo.toml and had to
# put it back, and this script injects into an in-memory copy instead precisely
# so that there is nothing to restore. The guard proves that rather than
# asserting it, and it compares before against after instead of demanding a
# clean tree, because a developer's tree is expected to be dirty while they
# work -- what must not change is the tree *because of this run*.
#
# Requires python3 (present by default on every distro in the platform baseline).

set -euo pipefail

usage() {
    cat <<'USAGE'
usage: scripts/check-no-network.sh [--self-test] [--metadata-file FILE] [--root DIR]

  --self-test            inject banned and benign packages and assert the outcomes
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
            echo "check-no-network: unknown argument: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

if ! command -v python3 >/dev/null 2>&1; then
    echo "check-no-network: python3 is required to parse cargo metadata; install it and re-run" >&2
    exit 2
fi

metadata_tmp="$(mktemp "${TMPDIR:-/tmp}/rspinyin-net.XXXXXX")"
trap 'rm -f -- "$metadata_tmp"' EXIT

if [ -n "$metadata_file" ]; then
    if ! cat -- "$metadata_file" >"$metadata_tmp"; then
        echo "check-no-network: cannot read metadata file: $metadata_file" >&2
        exit 2
    fi
else
    if ! (cd -- "$root" && cargo metadata --format-version 1 --all-features --locked) >"$metadata_tmp"; then
        echo "check-no-network: 'cargo metadata' failed under $root" >&2
        echo "check-no-network: fix the workspace manifest (or the committed lockfile) and re-run" >&2
        exit 2
    fi
fi

# analyse MODE: audit the captured metadata document for network capability.
# MODE is `none` for the real check; the self-test passes an injection mode that
# mutates the graph in memory before the very same audit runs.
analyse() {
    python3 - "$metadata_tmp" "$1" <<'PY'
import json
import sys
from pathlib import Path

metadata_path, mode = sys.argv[1], sys.argv[2]

# features.md 0.4 rule 6 / BUDGET-NET-01, matched against every dash or
# underscore separated segment of a crate name.
BANNED_SEGMENTS = frozenset(
    {
        "reqwest",
        "hyper",
        "ureq",
        "curl",
        "isahc",
        "surf",
        "awc",
        "openssl",
        "rustls",
        "tls",
        "tungstenite",
        "quinn",
        "zmq",
    }
)

# `tls` is the one name that is only banned in a leading or trailing segment.
# It abbreviates both Transport Layer Security and thread-local storage, and the
# crates this gate exists to stop -- `native-tls`, `tokio-native-tls`, `tls-api` --
# all carry it at one end. Matched in a medial position it would flag
# `scoped-tls-hkt`, a zero-dependency scoped thread-local helper that Slint pulls
# in and that cannot open a socket. Every other banned name is position-independent.
POSITIONAL_SEGMENTS = frozenset({"tls"})


def banned_segments(name):
    """Segments of `name` that put it in the banned set."""
    segments = name.replace("_", "-").split("-")
    hits = {segment for segment in segments if segment in BANNED_SEGMENTS}
    for segment in segments:
        if segment in POSITIONAL_SEGMENTS and segment not in (segments[0], segments[-1]):
            hits.discard(segment)
    return hits

# Crates that are only banned while their network features are enabled.
NET_FEATURE_CRATES = ("tokio", "async-std")
NET_FEATURE_PREFIX = "net"

try:
    document = json.loads(Path(metadata_path).read_text(encoding="utf-8"))
except (OSError, ValueError) as error:
    sys.exit(f"check-no-network: cannot parse cargo metadata: {error}")

packages = document["packages"]
names = {package["id"]: package["name"] for package in packages}
members = set(document["workspace_members"])
nodes = {node["id"]: node for node in document["resolve"]["nodes"]}
by_name = {package["name"]: package["id"] for package in packages}


def inject_package(name, features=()):
    """Add a package, its resolve node and an edge from a workspace member."""
    if not members:
        print("check-no-network: self-test cannot inject packages: no workspace members", file=sys.stderr)
        sys.exit(2)
    origin = sorted(members)[0]
    package_id = f"registry+https://example.invalid/index#{name}@0.0.0"
    packages.append(
        {
            "id": package_id,
            "name": name,
            "version": "0.0.0",
            "license": "MIT",
            "source": "registry+https://example.invalid/index",
            "dependencies": [],
            "targets": [],
            "features": {},
            "manifest_path": "/nonexistent/Cargo.toml",
        }
    )
    nodes[package_id] = {"id": package_id, "features": list(features), "deps": []}
    names[package_id] = name
    nodes[origin].setdefault("deps", []).append(
        {
            "name": name.replace("-", "_"),
            "pkg": package_id,
            "dep_kinds": [{"kind": None, "target": None}],
        }
    )
    return package_id


def enable_feature(crate, feature):
    """Turn on a feature of an existing (or injected) crate."""
    package_id = by_name.get(crate)
    if package_id is None:
        package_id = inject_package(crate)
        by_name[crate] = package_id
    node = nodes.setdefault(package_id, {"id": package_id, "features": [], "deps": []})
    node.setdefault("features", []).append(feature)


if mode == "banned-crate":
    inject_package("reqwest")
elif mode == "banned-variant":
    inject_package("openssl-sys")
elif mode == "banned-tls-variant":
    inject_package("native-tls")
elif mode == "net-feature":
    enable_feature("tokio", "net")
elif mode == "benign-lookalike":
    inject_package("surface-nets")
elif mode == "benign-tls-lookalike":
    inject_package("scoped-tls-hkt")
elif mode != "none":
    sys.exit(f"check-no-network: unknown self-test mode: {mode}")


def path_from_workspace(target):
    """Shortest dependency path from any workspace member to `target`."""
    for member in sorted(members):
        queue = [[member]]
        seen = {member}
        while queue:
            path = queue.pop(0)
            for dependency in nodes.get(path[-1], {}).get("deps", []):
                step = dependency["pkg"]
                if step == target:
                    return [names.get(entry, entry) for entry in path] + [
                        names.get(target, target)
                    ]
                if step not in seen:
                    seen.add(step)
                    queue.append(path + [step])
    return None


problems = []
scanned = 0

for package_id, name in sorted(names.items(), key=lambda item: item[1]):
    scanned += 1
    banned = sorted(banned_segments(name))
    if banned:
        path = path_from_workspace(package_id)
        where = f" (dependency path: {' -> '.join(path)})" if path else ""
        problems.append(
            f"{name}: banned network crate `{banned[0]}` is in the dependency closure{where}"
        )
        continue

    if name in NET_FEATURE_CRATES:
        features = nodes.get(package_id, {}).get("features", [])
        enabled = sorted(
            feature
            for feature in features
            if feature == "full"
            or feature == NET_FEATURE_PREFIX
            or feature.startswith(f"{NET_FEATURE_PREFIX}-")
        )
        if enabled:
            path = path_from_workspace(package_id)
            where = f" (dependency path: {' -> '.join(path)})" if path else ""
            problems.append(
                f"{name}: network feature enabled: {', '.join(enabled)}{where}"
            )

if problems:
    print("check-no-network: FAIL", file=sys.stderr)
    for problem in problems:
        print(f"  - {problem}", file=sys.stderr)
    print(
        "check-no-network: the plugin must stay byte-identically offline; "
        "remove the dependency or the feature",
        file=sys.stderr,
    )
    sys.exit(1)

print(f"check-no-network: PASS ({scanned} packages scanned, no network capability)")
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
        echo "check-no-network: self-test FAILED - $description exited $status, expected 0" >&2
        echo "$output" >&2
        return 1
    fi
    return 0
}

# Repository-state guard. See the note at the top of this file: the pair proves
# the self-test left the tree as it found it. `side_effect_tracked` distinguishes
# "the tree is clean" from "the tree could not be observed", which an empty
# snapshot cannot do on its own.
side_effect_snapshot=""
side_effect_tracked=0

snapshot_repository() {
    if ! command -v git >/dev/null 2>&1; then
        echo "check-no-network: git is unavailable; the self-test cannot observe the working tree"
        return 0
    fi
    if ! side_effect_snapshot="$(git -C "$root" status --porcelain 2>/dev/null)"; then
        side_effect_snapshot=""
        echo "check-no-network: $root is not a git work tree; the self-test cannot observe the working tree"
        return 0
    fi
    side_effect_tracked=1
    return 0
}

assert_repository_unchanged() {
    [ "$side_effect_tracked" -eq 1 ] || return 0
    local after
    if ! after="$(git -C "$root" status --porcelain 2>/dev/null)"; then
        return 0
    fi
    if [ "$after" = "$side_effect_snapshot" ]; then
        return 0
    fi
    echo "check-no-network: self-test FAILED - the self-test modified the working tree" >&2
    echo "check-no-network: git status --porcelain before:" >&2
    printf '%s\n' "$side_effect_snapshot" >&2
    echo "check-no-network: git status --porcelain after:" >&2
    printf '%s\n' "$after" >&2
    return 1
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
        echo "check-no-network: self-test FAILED - $description was not detected" >&2
        echo "$output" >&2
        return 1
    fi
    case "$output" in
        *"$expected_text"*) ;;
        *)
            echo "check-no-network: self-test FAILED - $description was reported for the wrong reason" >&2
            echo "check-no-network: expected the report to mention: $expected_text" >&2
            echo "$output" >&2
            return 1
            ;;
    esac
    return 0
}

if [ "$mode" = "self-test" ]; then
    echo "check-no-network: self-test (injecting packages into the real metadata document)"
    snapshot_repository
    expect_clean none "the unmodified workspace"
    expect_violation banned-crate "reqwest" "a banned crate (reqwest)"
    expect_violation banned-variant "openssl-sys" "a banned variant (openssl-sys)"
    expect_violation banned-tls-variant "native-tls" "a trailing-segment TLS crate (native-tls)"
    expect_violation net-feature "tokio: network feature enabled" "tokio with the net feature"
    expect_clean benign-lookalike "an unrelated name (surface-nets)"
    expect_clean benign-tls-lookalike "a medial tls that means thread-local storage (scoped-tls-hkt)"
    expect_clean none "the workspace after removing the violations"
    assert_repository_unchanged
    echo "check-no-network: self-test PASS (4 injected violations detected, clean graph accepted, no side effects)"
    exit 0
fi

analyse none
