#!/usr/bin/env bash
#
# Build rspinyin and install it as a Fcitx5 addon.
#
# The build runs as the invoking user and only the file copies are escalated, so the
# build tree never ends up owned by root. Run the script through `bash`; the executable
# bit is not required.
#
#   bash packaging/install.sh
#   bash packaging/install.sh --dry-run
#   bash packaging/install.sh --dict /path/to/base.dict --skip-build
#   DESTDIR=debian/rspinyin PREFIX=/usr bash packaging/install.sh --no-sudo
#
# `--dict` installs a dictionary that was compiled elsewhere instead of compiling one
# here. Compiling needs the raw sources under `data/raw/`, which are fetched rather than
# committed, so without this option an install on a machine that has never run
# `data/fetch.sh` either downloads them or fails. A release archive ships the compiled
# `base.dict`, and this is the option that installs it.
#
# Where the files go is resolved from the target system's own Fcitx5 installation by
# `xtask install`, not hardcoded here: Fcitx5's addon directory is
# `/usr/lib/x86_64-linux-gnu/fcitx5` on Debian and Ubuntu, `/usr/lib64/fcitx5` on Fedora
# and `/usr/lib/fcitx5` on Arch, and a hardcoded path installs the plugin where Fcitx5
# never looks for it. See xtask/src/install/layout.rs.

set -euo pipefail

dry_run=0
skip_build=0
no_sudo=0
dict_source=""
prefix="${PREFIX:-}"
destdir="${DESTDIR:-}"

usage() {
    cat <<'USAGE'
usage: install.sh [options]

  --dry-run        print the plan and change nothing
  --skip-build     install what is already in target/, without building it
  --dict PATH      install a prebuilt base.dict instead of compiling one
  --prefix PATH    installation prefix (default: the one pkg-config reports)
  --destdir PATH   staging directory prepended to every destination
  --no-sudo        never elevate; every destination must be writable as it is
  -h, --help       print this message

PREFIX and DESTDIR are also read from the environment, as the options above are the
same two values; an option wins over the variable.
USAGE
}

while [ $# -gt 0 ]; do
    case "$1" in
        --dry-run) dry_run=1 ;;
        --skip-build) skip_build=1 ;;
        --no-sudo) no_sudo=1 ;;
        --dict)
            [ $# -ge 2 ] || { echo "install.sh: --dict needs a path" >&2; exit 2; }
            dict_source="$2"
            shift
            ;;
        --prefix)
            [ $# -ge 2 ] || { echo "install.sh: --prefix needs a path" >&2; exit 2; }
            prefix="$2"
            shift
            ;;
        --destdir)
            [ $# -ge 2 ] || { echo "install.sh: --destdir needs a path" >&2; exit 2; }
            destdir="$2"
            shift
            ;;
        -h | --help)
            usage
            exit 0
            ;;
        *)
            echo "install.sh: unknown option '$1'" >&2
            usage >&2
            exit 2
            ;;
    esac
    shift
done

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
root="$(dirname -- "$script_dir")"
cd -- "$root"

# The build needs the Fcitx5 headers and the destination paths come from the same
# development package, so both stop here rather than at a linker error.
if ! command -v pkg-config >/dev/null 2>&1 || ! pkg-config --exists Fcitx5Core; then
    cat >&2 <<'MISSING'
platform/fcitx5/dev-missing: could not find the Fcitx5 development package `Fcitx5Core` via pkg-config.
Install it with one of:
  Debian/Ubuntu: sudo apt install libfcitx5core-dev libfcitx5utils-dev libfcitx5config-dev
  Fedora:        sudo dnf install fcitx5-devel
  Arch:          sudo pacman -S fcitx5
Alternatively, build without the `fcitx5-host` feature to compile only the pure-Rust side.
MISSING
    exit 1
fi

if [ "$no_sudo" -eq 0 ] && [ "$(id -u)" -ne 0 ] && [ -z "$destdir" ]; then
    echo "install: not running as root; the file copies will go through sudo" >&2
fi

# Unconditional, and before anything is built. Even with `--dict` and `--skip-build` the
# file copies run through `xtask install`, so a machine without cargo fails a few lines
# later regardless; naming the missing prerequisite here is what turns that into a
# diagnosis instead of `command not found`.
if ! command -v cargo >/dev/null 2>&1; then
    cat >&2 <<'MISSING'
platform/toolchain/missing: `cargo` is not on PATH.
This script installs from a build tree. `--dict` removes the need to compile the
dictionary, but the file copies still run through `xtask install`, and building that
needs the toolchain pinned by rust-toolchain.toml (https://rustup.rs).
Installing from a release archive without a Rust toolchain is not supported here yet.
MISSING
    exit 1
fi

if [ "$skip_build" -eq 0 ]; then
    echo "install: building the two addons"
    # Both cdylibs: the engine and the candidate-window addon (ADR-0003's split).
    # Building only the first would leave the install plan's second payload missing,
    # which the installer reports as a missing artifact rather than a failed build.
    cargo build --release -p ime-fcitx5 -p ime-ui-addon --features fcitx5-host
fi

# The dictionary is either compiled here or staged from `--dict`. `--skip-build` skips
# only the compilation: staging a prebuilt dictionary is a file copy, and refusing it
# would make the option useless in the environment it exists for.
if [ "$skip_build" -eq 0 ] && [ -z "$dict_source" ]; then
    echo "install: building the dictionary"
    cargo run --quiet -p xtask -- dictc
elif [ -n "$dict_source" ]; then
    # A directory satisfies a bare `-s` on Linux, and `cp` would then fail with a message
    # about omitting it rather than about the option naming the wrong thing.
    if [ ! -f "$dict_source" ] || [ ! -s "$dict_source" ]; then
        echo "install.sh: --dict ${dict_source}: not a non-empty regular file" >&2
        exit 2
    fi
    if [ "$dry_run" -eq 1 ]; then
        echo "install: would stage the prebuilt dictionary from ${dict_source}"
    # `xtask install` reads `data/compiled/base.dict`, so a caller who names that very
    # file is asking for a copy onto itself; it is already where it belongs.
    elif [ "$dict_source" -ef data/compiled/base.dict ]; then
        echo "install: the prebuilt dictionary is already at data/compiled/base.dict"
    else
        echo "install: using the prebuilt dictionary at ${dict_source}"
        mkdir -p data/compiled
        cp -- "$dict_source" data/compiled/base.dict
    fi
fi

install_args=()
if [ "$dry_run" -eq 1 ]; then
    install_args+=(--dry-run)
fi
if [ "$no_sudo" -eq 1 ]; then
    install_args+=(--no-sudo)
fi
if [ -n "$prefix" ]; then
    install_args+=(--prefix "$prefix")
fi
if [ -n "$destdir" ]; then
    install_args+=(--destdir "$destdir")
fi

cargo run --quiet -p xtask -- install ${install_args[@]+"${install_args[@]}"}

if [ "$dry_run" -eq 1 ]; then
    echo "install: dry run complete; nothing was written"
fi
