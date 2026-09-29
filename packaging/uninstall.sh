#!/usr/bin/env bash
#
# Remove rspinyin from the Fcitx5 addon directories.
#
# Everything the install put there is removed, everything it displaced is put back, and
# the Fcitx5 user interface the plugin took over is restored to what it was. Nothing the
# install did not create is touched, and the user's own dictionary, configuration and
# logs are left alone -- their location and the command that deletes them are printed.
# Run the script through `bash`; the executable bit is not required.
#
#   bash packaging/uninstall.sh
#   bash packaging/uninstall.sh --dry-run
#   DESTDIR=debian/rspinyin PREFIX=/usr bash packaging/uninstall.sh --no-sudo
#
# The list of files to remove is read from the manifest `xtask install` wrote, not from
# a copy of the payload table: that is what lets an uninstall tell a file it created
# from one a distribution package had already put there.

set -euo pipefail

dry_run=0
no_sudo=0
prefix="${PREFIX:-}"
destdir="${DESTDIR:-}"

usage() {
    cat <<'USAGE'
usage: uninstall.sh [options]

  --dry-run        print the plan and change nothing
  --prefix PATH    installation prefix the plugin was installed under
  --destdir PATH   staging directory the plugin was installed into
  --no-sudo        never elevate; every destination must be writable as it is
  -h, --help       print this message

PREFIX and DESTDIR are also read from the environment; an option wins over the variable.
USAGE
}

while [ $# -gt 0 ]; do
    case "$1" in
        --dry-run) dry_run=1 ;;
        --no-sudo) no_sudo=1 ;;
        --prefix)
            [ $# -ge 2 ] || { echo "uninstall.sh: --prefix needs a path" >&2; exit 2; }
            prefix="$2"
            shift
            ;;
        --destdir)
            [ $# -ge 2 ] || { echo "uninstall.sh: --destdir needs a path" >&2; exit 2; }
            destdir="$2"
            shift
            ;;
        -h | --help)
            usage
            exit 0
            ;;
        *)
            echo "uninstall.sh: unknown option '$1'" >&2
            usage >&2
            exit 2
            ;;
    esac
    shift
done

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
root="$(dirname -- "$script_dir")"
cd -- "$root"

# The destination directories come from the same development package the build uses, so
# a machine that cannot resolve them cannot resolve what to remove either.
if ! command -v pkg-config >/dev/null 2>&1 || ! pkg-config --exists Fcitx5Core; then
    cat >&2 <<'MISSING'
platform/fcitx5/dev-missing: could not find the Fcitx5 development package `Fcitx5Core` via pkg-config.
Install it with one of:
  Debian/Ubuntu: sudo apt install libfcitx5core-dev libfcitx5utils-dev libfcitx5config-dev
  Fedora:        sudo dnf install fcitx5-devel
  Arch:          sudo pacman -S fcitx5
MISSING
    exit 1
fi

if [ "$no_sudo" -eq 0 ] && [ "$(id -u)" -ne 0 ] && [ -z "$destdir" ]; then
    echo "uninstall: not running as root; the file removals will go through sudo" >&2
fi

uninstall_args=(--uninstall)
if [ "$dry_run" -eq 1 ]; then
    uninstall_args+=(--dry-run)
fi
if [ "$no_sudo" -eq 1 ]; then
    uninstall_args+=(--no-sudo)
fi
if [ -n "$prefix" ]; then
    uninstall_args+=(--prefix "$prefix")
fi
if [ -n "$destdir" ]; then
    uninstall_args+=(--destdir "$destdir")
fi

# Uninstalling reads the manifest `xtask install` wrote and undoes it, so it runs the
# same binary -- and that binary has to be built. A user who removed the Rust toolchain
# after installing is exactly the user who runs this, so the missing prerequisite is
# named here rather than surfacing as `command not found`.
if ! command -v cargo >/dev/null 2>&1; then
    cat >&2 <<'MISSING'
platform/toolchain/missing: `cargo` is not on PATH.
Uninstalling reads the install manifest and restores what the install displaced, and
both are done by the `xtask` binary, which has to be built. It needs the toolchain
pinned by rust-toolchain.toml (https://rustup.rs). Removing the plugin by hand is
possible but leaves the backups behind: the manifest under <datadir>/rspinyin names
every file this installer replaced.
MISSING
    exit 1
fi

cargo run --quiet -p xtask -- install ${uninstall_args[@]+"${uninstall_args[@]}"}

if [ "$dry_run" -eq 1 ]; then
    echo "uninstall: dry run complete; nothing was removed"
fi
