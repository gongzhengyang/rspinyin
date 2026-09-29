# rspinyin quality gates.
#
# `just check` is the four commands fixed by features.md 0.3 and is this
# project's definition of "done"; `just ci` adds the architecture and licence
# audits of features.md 0.4 and ADR-0000.
#
# The audit scripts are invoked through `bash` so they work whether or not the
# executable bit survived checkout. They need python3, which every distro in the
# platform baseline ships; `cargo-public-api` is needed by `check-slint` only.

# List the available recipes.
default:
    @just --list

# Formatting, lints, unit tests and doctests (features.md 0.3, in this order).
check:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets --all-features -- -D warnings
    if ! cargo nextest --version >/dev/null 2>&1; then
        echo "check: cargo-nextest is not installed; run: cargo install cargo-nextest --locked" >&2
        exit 1
    fi
    cargo nextest run --workspace --all-features
    cargo test --workspace --doc

# Tests only, following the test-execution baseline (nextest, doctests on top).
test:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! cargo nextest --version >/dev/null 2>&1; then
        cargo install cargo-nextest --locked
    fi
    if cargo nextest --version >/dev/null 2>&1; then
        cargo nextest run --workspace --all-features
    else
        echo "test: cargo-nextest is unavailable, falling back to cargo test" >&2
        cargo test --workspace --all-features
    fi
    cargo test --workspace --doc

# Crate layering: one-way dependencies, UI isolation, diagnostics leaf (0.4 rules 1-2).
check-deps:
    bash scripts/check-deps.sh

# unsafe and extern "C" isolation plus SAFETY comments (0.4 rule 3).
check-unsafe:
    bash scripts/check-unsafe.sh

# Zero network capability in the dependency closure (0.4 rule 6, BUDGET-NET-01).
check-net:
    bash scripts/check-no-network.sh

# The runtime half of the same promise: a live fcitx5 session must hold no IP
# socket (BUDGET-NET-01). Needs a real session, so it is not part of `ci`.
check-net-runtime:
    bash scripts/runtime-socket-check.sh

# No Slint type in the public API of ime-ui (0.4 rule 11, ADR-0000 OB-4).
check-slint:
    bash scripts/check-slint-leak.sh

# Dictionary source allowlist: licences and hashes (ADR-0000 decision 2).
check-dict:
    bash scripts/check-dict-sources.sh

# Dependency licences and the OB-1..OB-6 review (ADR-0000 decision 1).
check-licenses:
    bash scripts/gen-licenses.sh --check

# Refresh the generated blocks in docs/dev/licenses.md and docs/dev/NOTICE.
gen-licenses:
    bash scripts/gen-licenses.sh --write

# OB-1's reachability assertion. Kept out of `ci` because it is the only network
# access in this repository's tooling and the CI job is offline by design.
check-licenses-links:
    bash scripts/gen-licenses.sh --check-links

# Performance budgets against the authoritative table (0.5.3).
check-budget:
    cargo run --quiet -p xtask -- budget --validate

# The packaged addon descriptor must advertise the workspace version; Fcitx5 reports the
# descriptor's value, so drift ships a mislabelled plugin.
check-versions:
    cargo run --quiet -p xtask -- check-versions

# Fuzz the decode engine. libFuzzer needs nightly, but the workspace pins a stable
# toolchain for reproducible builds, so the invocation has to override it explicitly.
#   just fuzz          the 60-second soak the acceptance criteria ask for
#   just fuzz 600      a longer run
fuzz seconds="60":
    cargo +nightly fuzz run dag_build -- -max_total_time={{seconds}}

# Self-tests of the seven audit scripts: each one injects a violation, asserts a
# non-zero exit, removes it and asserts a zero exit.
# Build and install the plugin into the system's Fcitx5 addon directories.
install *args:
    bash packaging/install.sh {{args}}

# Remove it again, restoring whatever it replaced.
uninstall *args:
    bash packaging/uninstall.sh {{args}}

check-self-tests:
    #!/usr/bin/env bash
    set -euo pipefail
    bash scripts/check-deps.sh --self-test
    bash scripts/check-unsafe.sh --self-test
    bash scripts/check-no-network.sh --self-test
    bash scripts/runtime-socket-check.sh --self-test
    bash scripts/check-slint-leak.sh --self-test
    bash scripts/check-dict-sources.sh --self-test
    bash scripts/gen-licenses.sh --self-test

# Benchmarks (budget assertions live with the benchmarks themselves).
bench:
    cargo bench --workspace

# Quick benchmark pass for CI. The guard keeps the job green while the workspace
# still has no criterion targets: libtest rejects `--quick`, and a bench job that
# cannot run is reported instead of failing for the wrong reason.
bench-quick:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! metadata="$(cargo metadata --format-version 1 --no-deps | tr -d ' \n')"; then
        echo "bench-quick: 'cargo metadata' failed; fix the workspace manifest" >&2
        exit 2
    fi
    case "$metadata" in
        *'"kind":["bench"]'*) cargo bench --workspace -- --quick ;;
        *) echo "bench-quick: no benchmark targets in the workspace yet, nothing to assert" ;;
    esac

# The full gate suite: quality commands plus every architecture and licence audit.
ci: check check-deps check-unsafe check-net check-slint check-dict check-licenses check-budget check-versions
