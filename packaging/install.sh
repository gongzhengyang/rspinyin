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
#   DESTDIR=debian/rspinyin PREFIX=/usr bash packaging/install.sh --no-sudo
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
prefix="${PREFIX:-}"
destdir="${DESTDIR:-}"

usage() {
    cat <<'USAGE'
usage: install.sh [options]

  --dry-run        print the plan and change nothing
  --skip-build     install what is already in target/, without building it
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

if [ "$skip_build" -eq 0 ]; then
    echo "install: building the addon"
    cargo build --release -p ime-fcitx5 --features fcitx5-host
    echo "install: building the dictionary"
    cargo run --quiet -p xtask -- dictc
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
