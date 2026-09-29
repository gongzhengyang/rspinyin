# rspinyin quality gates.
#
# `just check` is the four commands fixed by features.md 0.3 and is this
# project's definition of "done"; `just ci` adds the architecture and licence
# audits of features.md 0.4 and ADR-0000.
#
# `--all-features` cannot be passed to the whole workspace. It turns on the
# `fcitx5-host` feature of the two addon crates -- `ime-fcitx5` and
# `ime-ui-addon`, the pair of cdylibs ADR-0003 and ADR-0004 split the plugin
# into -- and each of their build scripts aborts when pkg-config cannot find the
# Fcitx5 development packages. That would make the dependency-free job depend on
# those packages, which is the exact opposite of what that job exists to prove.
# AGENTS.md section 3.1 names `fcitx5-host` as the one deliberate exception to
# `--all-features`: both crates are excluded from every `--all-features` command
# below and are linted, tested and doctested on their default (empty) feature
# set instead, while the real C ABI link is exercised by `just check-host` on a
# machine that has the development packages installed.
#
# The audit scripts are invoked through `bash` so they work whether or not the
# executable bit survived checkout. They need python3, which every distro in the
# platform baseline ships; `cargo-public-api` is needed by `check-slint` only.

# List the available recipes.
default:
    @just --list

# Formatting, lints, unit tests and doctests (features.md 0.3, in this order).
#
# The two addon crates carry the `fcitx5-host` feature and are therefore split
# out of the `--all-features` commands; see the note at the top of this file.
check:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo fmt --all -- --check
    cargo clippy --workspace --exclude ime-fcitx5 --exclude ime-ui-addon --all-targets --all-features -- -D warnings
    cargo clippy -p ime-fcitx5 -p ime-ui-addon --all-targets -- -D warnings
    if ! cargo nextest --version >/dev/null 2>&1; then
        echo "check: cargo-nextest is not installed; run: cargo install cargo-nextest --locked" >&2
        exit 1
    fi
    cargo nextest run --workspace --exclude ime-fcitx5 --exclude ime-ui-addon --all-features
    cargo nextest run -p ime-fcitx5 -p ime-ui-addon
    cargo test --workspace --exclude ime-fcitx5 --exclude ime-ui-addon --doc --all-features
    cargo test -p ime-fcitx5 -p ime-ui-addon --doc

# The same gates with the real Fcitx5 C ABI linked in. Requires
# libfcitx5core-dev, libfcitx5utils-dev and libfcitx5config-dev; without them
# the build aborts with the `platform/fcitx5/dev-missing` diagnostic from
# either addon's build.rs.
check-host:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo clippy -p ime-fcitx5 --all-targets --features fcitx5-host -- -D warnings
    cargo clippy -p ime-ui-addon --all-targets --features fcitx5-host -- -D warnings
    if ! cargo nextest --version >/dev/null 2>&1; then
        echo "check-host: cargo-nextest is not installed; run: cargo install cargo-nextest --locked" >&2
        exit 1
    fi
    cargo nextest run -p ime-fcitx5 --features fcitx5-host
    cargo nextest run -p ime-ui-addon --features fcitx5-host
    cargo test -p ime-fcitx5 --doc --features fcitx5-host
    cargo test -p ime-ui-addon --doc --features fcitx5-host

# Tests only, following the test-execution baseline (nextest, doctests on top).
test:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! cargo nextest --version >/dev/null 2>&1; then
        cargo install cargo-nextest --locked
    fi
    if cargo nextest --version >/dev/null 2>&1; then
        cargo nextest run --workspace --exclude ime-fcitx5 --exclude ime-ui-addon --all-features
        cargo nextest run -p ime-fcitx5 -p ime-ui-addon
    else
        echo "test: cargo-nextest is unavailable, falling back to cargo test" >&2
        cargo test --workspace --exclude ime-fcitx5 --exclude ime-ui-addon --all-features
        cargo test -p ime-fcitx5 -p ime-ui-addon
    fi
    cargo test --workspace --exclude ime-fcitx5 --exclude ime-ui-addon --doc --all-features
    cargo test -p ime-fcitx5 -p ime-ui-addon --doc

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

# Candidate-window spec conformance: the geometry, grid and colour tables of
# features.md 3.1 and 3.2 against the .slint sources and src/theme.rs, plus the
# assertion that no token and no constant is declared and never drawn.
check-ui:
    bash scripts/check-ui-spec.sh

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

# Build the release addons and turn them into a release under dist/: the archive, the
# release manifest and SHA256SUMS. The version comes from the workspace manifest and the
# architecture from the host, so a release cannot be mislabelled by a hand-typed argument.
#
# The build is part of this recipe because the packager does not build: like `xtask
# install`, it reports a missing artifact as a missing artifact rather than as a failed
# build. `--features fcitx5-host` is required -- without it the cdylibs are built without
# the C ABI glue and export no addon factory.
package:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo build --release -p ime-fcitx5 --features fcitx5-host
    cargo build --release -p ime-ui-addon --features fcitx5-host
    arch="$(uname -m)"
    cargo run --quiet -p xtask -- package --out "dist" --arch "$arch"

# The same, labelled with an architecture the caller names. Used by the release jobs that
# build somewhere other than they package: the caller is responsible for having built that
# architecture's artifacts, because the packager labels the release with what it is told.
package-arch arch:
    cargo run --quiet -p xtask -- package --out "dist" --arch "{{arch}}"

# Measure the release artifacts under dist/ against the size budgets in
# docs/dev/budgets.json. Kept separate from `check-budget` (which only cross-checks the
# document against the specification table) because measuring needs a release build:
# run `just package` first.
check-size:
    cargo run --quiet -p xtask -- budget --measure

# Vulnerability and licence-bans gate (AGENTS.md section 3.9).
check-advisories:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! cargo audit --version >/dev/null 2>&1; then
        echo "check-advisories: cargo-audit is not installed; run: cargo install cargo-audit --locked" >&2
        exit 1
    fi
    if ! cargo deny --version >/dev/null 2>&1; then
        echo "check-advisories: cargo-deny is not installed; run: cargo install cargo-deny --locked" >&2
        exit 1
    fi
    cargo audit
    cargo deny check

# Fuzz the decode engine. libFuzzer needs nightly, but the workspace pins a stable
# toolchain for reproducible builds, so the invocation has to override it explicitly.
#   just fuzz          the 60-second soak the acceptance criteria ask for
#   just fuzz 600      a longer run
fuzz seconds="60":
    cargo +nightly fuzz run dag_build -- -max_total_time={{seconds}}

# Build and install the plugin into the system's Fcitx5 addon directories.
install *args:
    bash packaging/install.sh {{args}}

# Remove it again, restoring whatever it replaced.
uninstall *args:
    bash packaging/uninstall.sh {{args}}

# Self-tests of the eight audit scripts: each one injects a violation, asserts a
# non-zero exit, removes it and asserts a zero exit.
check-self-tests:
    #!/usr/bin/env bash
    set -euo pipefail
    bash scripts/check-deps.sh --self-test
    bash scripts/check-unsafe.sh --self-test
    bash scripts/check-no-network.sh --self-test
    bash scripts/runtime-socket-check.sh --self-test
    bash scripts/check-slint-leak.sh --self-test
    bash scripts/check-ui-spec.sh --self-test
    bash scripts/idle-cpu-check.sh --self-test
    bash scripts/check-dict-sources.sh --self-test
    bash scripts/gen-licenses.sh --self-test

# Benchmarks, then the budget gate over what they measured. The gate reads the criterion
# output this recipe just produced and fails on a case that is past its threshold, so a
# performance regression is caught by the same command that measures it.
bench:
    cargo bench --workspace
    cargo run --quiet -p xtask -- budget --check

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
# `check-host` is deliberately absent -- it needs the Fcitx5 development packages,
# which the dependency-free job must not require. Run it separately (CI job
# `host-abi`) or via `just ci-host` on a machine that has them.
#
# `check-advisories` is absent for a different reason: `cargo audit` reaches the
# registry to check whether a version has been yanked, so putting it here would make
# the whole gate fail on a machine with no network -- and the CI `quality` job is
# offline by design. The advisory scan is its own job (`audit`) and its own recipe,
# run where the network is available. `AGENTS.md` section 3.9 makes it a pre-commit
# step for exactly that reason.
ci: check check-deps check-unsafe check-net check-slint check-ui check-dict check-licenses check-budget check-versions check-self-tests

# `just ci` plus the host-ABI half. Needs the Fcitx5 development packages.
ci-host: ci check-host
