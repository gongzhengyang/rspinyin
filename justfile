# rspinyin quality gates.
#
# `just ci` is this repository's single definition of "done": the four commands
# fixed by features.md 0.3, every architecture and licence audit of features.md
# 0.4 and ADR-0000, and the host-ABI half. The CI workflow never writes that set
# down again -- `quality` runs `just ci`, and the jobs that run what the suite
# cannot (a scan that needs the network, the host-ABI half) call the single recipe
# that owns it -- so a gate cannot be added on one side and forgotten on the other.
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
# set instead, while the real C ABI link is exercised by `just check-host` -- part of
# `just ci` -- on a machine that has the development packages installed.
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

# The same gates with the real Fcitx5 C ABI linked in. Part of `just ci`, so the
# suite covers the host-ABI path on every machine that can build it.
#
# The three libraries the addon build scripts probe are checked with pkg-config
# before cargo is invoked. Where they are present the gates run; where they are not,
# the recipe reports a skip instead of letting build.rs abort with
# `platform/fcitx5/dev-missing`. The distinction matters: an abort reads as a failing
# gate, and the dependency-free path -- the reason the pure-Rust half of the suite
# exists at all -- has to stay green without the development packages.
#
# `RSPINYIN_REQUIRE_HOST_ABI=1` turns the skip into a failure. The CI `host-abi` job
# sets it, so a runner whose package install silently failed cannot pass by skipping
# the gate it exists to run. `just ci-host` is the same thing locally.
check-host:
    #!/usr/bin/env bash
    set -euo pipefail
    # Kept in step with REQUIRED_LIBS in crates/ime-fcitx5/build.rs and
    # crates/ime-ui-addon/build.rs: a library missing from this list would let the
    # probe answer "available" and then let the build abort anyway.
    required_libs=(Fcitx5Core Fcitx5Utils Fcitx5Config)
    host_abi_available() {
        command -v pkg-config >/dev/null 2>&1 || return 1
        local lib
        for lib in "${required_libs[@]}"; do
            pkg-config --exists "$lib" || return 1
        done
    }
    if ! host_abi_available; then
        if [ "${RSPINYIN_REQUIRE_HOST_ABI:-0}" = "1" ]; then
            printf '%s\n' \
                "check-host: platform/fcitx5/dev-missing: pkg-config cannot find Fcitx5Core," \
                "check-host: Fcitx5Utils or Fcitx5Config, and RSPINYIN_REQUIRE_HOST_ABI=1 makes" \
                "check-host: that a failure. Install them with one of:" \
                "check-host:   Debian/Ubuntu: sudo apt install libfcitx5core-dev libfcitx5utils-dev libfcitx5config-dev" \
                "check-host:   Fedora:        sudo dnf install fcitx5-devel" \
                "check-host:   Arch:          sudo pacman -S fcitx5" >&2
            exit 1
        fi
        printf '%s\n' \
            "check-host: skipped: pkg-config cannot find Fcitx5Core, Fcitx5Utils or" \
            "check-host: Fcitx5Config, so the host-ABI half of this suite cannot run here." \
            "check-host: the pure-Rust half did run; the C ABI half is what the CI job" \
            "check-host: 'host-abi' covers. Set RSPINYIN_REQUIRE_HOST_ABI=1 to make the" \
            "check-host: absence a failure instead of a report." >&2
        exit 0
    fi
    # The addon's lifecycle tests wait out the font warm-up once per test process, and
    # under a parallel build that warm-up can stretch past the default wait the tests
    # fall back to. Raising it here keeps this gate green on a loaded machine while
    # every path that does not come through this recipe keeps the default; a window
    # that genuinely fails to draw still fails, because the gate is the painted
    # pixels, never the clock.
    export RSPINYIN_TEST_FRAME_TIMEOUT_SECS=240
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
check-no-grab:
    bash scripts/check-no-grab.sh

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
    bash scripts/check-no-grab.sh --self-test
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

# Quick benchmark pass for CI.
#
# A workspace with no benchmark target cannot run this gate at all, and a job that
# reported success there would be claiming a measurement it never made -- so the
# absence is a failure, not a note. The check is on `cargo metadata` rather than on
# libtest's exit code because libtest rejects `--quick` outright, which would report
# the wrong reason.
bench-quick:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! metadata="$(cargo metadata --format-version 1 --no-deps | tr -d ' \n')"; then
        echo "bench-quick: 'cargo metadata' failed; fix the workspace manifest" >&2
        exit 2
    fi
    case "$metadata" in
        *'"kind":["bench"]'*) cargo bench --workspace -- --quick ;;
        *)
            echo "bench-quick: no benchmark target exists in this workspace, so this gate" >&2
            echo "bench-quick: would pass without measuring anything. Add the criterion" >&2
            echo "bench-quick: targets the budgeted paths need before relying on it." >&2
            exit 1
            ;;
    esac

# Every architecture, licence, budget and self-test audit, in one list. `just ci` is
# `check audits check-host`, so this is the only place the audit set is written down:
# the CI workflow runs `just ci` and never names an audit of its own.
audits: check-deps check-unsafe check-no-grab check-net check-slint check-ui check-dict check-licenses check-budget check-versions check-self-tests

# The full gate suite: the four commands of features.md 0.3, every architecture and
# licence audit, and the host-ABI half. This is the single definition of "done", and
# the CI `quality` job runs this recipe and nothing else.
#
# `check-host` is part of the suite and reports a skip rather than failing when the
# Fcitx5 development packages are absent; see its own comment. That is what keeps the
# dependency-free path dependency-free while still covering the host-ABI gate
# everywhere it can be covered.
#
# `check-advisories` is deliberately absent: `cargo audit` reaches the registry to
# check whether a version has been yanked, so putting it here would make the whole
# gate fail on a machine with no network. It is its own recipe and its own CI job
# (`audit`), run where the network is available. `AGENTS.md` section 3.9 makes it a
# pre-commit step for exactly that reason.
ci: check audits check-host

# `just ci` with the host-ABI half required rather than optional: the same suite, but
# a missing Fcitx5 development package is a failure instead of a reported skip.
ci-host:
    #!/usr/bin/env bash
    set -euo pipefail
    RSPINYIN_REQUIRE_HOST_ABI=1 just ci

# The aarch64 gate: build both addons and run the host-ABI half of the suite on an
# arm64 machine. This is what the CI job `cross-arch` runs, and the evidence the
# delivery matrix cites when it claims the architecture.
#
# It refuses to run anywhere else. A cross build on an x86_64 host would produce
# artifacts whose ELF headers say AArch64 while nothing on that machine could load
# them: both addons are cdylibs that link against arm64 `libFcitx5Core.so.7` and
# `libFcitx5Utils.so.2`, which no x86_64 installation has. That path exists -- it is
# `cross-arm64` below -- but it is a developer aid, not this gate.
#
# Three assertions, each of which a successful build does not imply:
#   * the ELF machine: a mis-set linker happily produces an x86_64 object and cargo
#     still reports success;
#   * the exported factory symbol: rustc's generated version script lists only the
#     symbols Rust marks for export, so the C++ factory can disappear without a
#     warning, and a library that loads but contains no addon is the failure mode
#     that reaches a user;
#   * the test suite: the architecture claim has to rest on the code running rather
#     than on the file existing.
#
# What this cannot cover is a real fcitx5 session. A CI runner has no display server
# and no compositor, so "the addon loads into a live fcitx5 on aarch64" stays
# unverified here and needs a machine with a session to close.
check-arm64:
    #!/usr/bin/env bash
    set -euo pipefail
    arch="$(uname -m)"
    if [ "$arch" != "aarch64" ]; then
        printf '%s\n' \
            "check-arm64: dist/verify/not-arm64: this gate runs on arm64 and this machine is ${arch}." \
            "check-arm64: the cross-compilation path is 'just cross-arm64', which builds and" \
            "check-arm64: inspects the artifacts but cannot execute them on this machine." >&2
        exit 1
    fi
    cargo build --release -p ime-fcitx5 --features fcitx5-host
    cargo build --release -p ime-ui-addon --features fcitx5-host
    for library in target/release/librspinyin.so target/release/librspinyin_ui.so; do
        test -f "$library" \
            || { echo "check-arm64: dist/verify/artifact-missing: ${library} was not built" >&2; exit 1; }
        readelf -h "$library" | grep -q 'AArch64' \
            || { echo "check-arm64: dist/verify/not-arm64: ${library} is not an aarch64 object" >&2; exit 1; }
        nm -D --defined-only "$library" | grep -q 'fcitx_addon_factory_instance' \
            || { echo "check-arm64: dist/verify/factory-symbol-missing: ${library} exports no addon factory" >&2; exit 1; }
        size "$library"
    done
    # Required, not optional: the development packages were just used to build the
    # addons, so a skip here would mean the aarch64 claim rests on a build that never
    # ran a test. The release artifacts above are the ones a package would ship; this
    # half is what proves they run.
    RSPINYIN_REQUIRE_HOST_ABI=1 just check-host

# Cross-compile both addons for aarch64 from an x86_64 host and inspect what came out.
#
# The aarch64 linker and the emulator runner are opt-in rather than part of
# `.cargo/config.toml`, and `packaging/cross/aarch64-unknown-linux-gnu.toml` records
# why: cargo applies `[target.<triple>]` to host artifacts whenever the host triple is
# the key, so a repository-wide `linker` entry would also be picked up by a plain
# `cargo build` on an arm64 machine, where the build is native and no cross linker
# exists. This recipe is the x86_64 side of that split.
#
# The prerequisites cannot be installed by this repository, so the recipe checks for
# them and stops with the command that fixes each one. The sysroot is the interesting
# one: the addons link against arm64 Fcitx5 libraries, and until an arm64 sysroot
# holds the development packages there is nothing to link against -- `cargo build
# --target` on its own cannot produce a loadable addon.
#
#   AARCH64_SYSROOT   arm64 sysroot holding the Fcitx5 development packages
#                     (default: /usr/aarch64-linux-gnu)
cross-arm64:
    #!/usr/bin/env bash
    set -euo pipefail
    config="packaging/cross/aarch64-unknown-linux-gnu.toml"
    sysroot="${AARCH64_SYSROOT:-/usr/aarch64-linux-gnu}"
    command -v aarch64-linux-gnu-gcc >/dev/null 2>&1 || {
        echo "cross-arm64: platform/cross/toolchain-missing: aarch64-linux-gnu-gcc is not on PATH." >&2
        echo "cross-arm64: install gcc-aarch64-linux-gnu and g++-aarch64-linux-gnu." >&2
        exit 1
    }
    if command -v rustup >/dev/null 2>&1; then
        rustup target list --installed | grep -qx 'aarch64-unknown-linux-gnu' || {
            echo "cross-arm64: platform/cross/target-missing: run 'rustup target add aarch64-unknown-linux-gnu'." >&2
            exit 1
        }
    fi
    test -d "${sysroot}/usr/lib/aarch64-linux-gnu/pkgconfig" || {
        echo "cross-arm64: platform/cross/sysroot-missing: ${sysroot} holds no arm64 Fcitx5 development packages." >&2
        echo "cross-arm64: unpack libfcitx5core-dev, libfcitx5utils-dev and libfcitx5config-dev" >&2
        echo "cross-arm64: for arm64 into it, or point AARCH64_SYSROOT at one that has them;" >&2
        echo "cross-arm64: see the header of ${config}." >&2
        exit 1
    }
    # Without PKG_CONFIG_ALLOW_CROSS the build scripts refuse to probe a foreign
    # target at all; without the two paths, pkg-config answers with the x86_64 include
    # and library directories and the link fails on undefined fcitx:: symbols.
    export PKG_CONFIG_ALLOW_CROSS=1
    export PKG_CONFIG_SYSROOT_DIR="${sysroot}"
    export PKG_CONFIG_LIBDIR="${sysroot}/usr/lib/aarch64-linux-gnu/pkgconfig:${sysroot}/usr/share/pkgconfig"
    cargo build --release --target aarch64-unknown-linux-gnu --config "${config}" \
        -p ime-fcitx5 --features fcitx5-host
    cargo build --release --target aarch64-unknown-linux-gnu --config "${config}" \
        -p ime-ui-addon --features fcitx5-host
    out="target/aarch64-unknown-linux-gnu/release"
    for library in "${out}/librspinyin.so" "${out}/librspinyin_ui.so"; do
        test -f "$library" \
            || { echo "cross-arm64: dist/verify/artifact-missing: ${library} was not built" >&2; exit 1; }
        readelf -h "$library" | grep -q 'AArch64' \
            || { echo "cross-arm64: dist/verify/not-arm64: ${library} is not an aarch64 object" >&2; exit 1; }
        nm -D --defined-only "$library" | grep -q 'fcitx_addon_factory_instance' \
            || { echo "cross-arm64: dist/verify/factory-symbol-missing: ${library} exports no addon factory" >&2; exit 1; }
        size "$library"
    done
    echo "cross-arm64: the artifacts above are aarch64; nothing on this host can load"
    echo "cross-arm64: them. Run 'just check-arm64' on an arm64 machine to execute the"
    echo "cross-arm64: test suite against this architecture."
