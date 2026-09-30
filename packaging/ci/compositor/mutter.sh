#!/usr/bin/env bash
#
# mutter.sh - the Wayland/Mutter tier check.
#
# Mutter is the third tier's compositor and the one the project's own risk register names
# as the most likely to reject the approach: it clamps an `xdg_popup` positioner harder
# than KWin does, and the fallback rung below the popup is a full-output parent surface
# with a positioned child -- a shape Mutter may refuse to map without taking the keyboard
# (`docs/dev/spikes/wayland-tiers.md`, section 1c). That open question is what this tier
# exists to answer, and it is answered by running the ladder on a real Mutter session.
#
# # Nested, and what that costs
#
# Mutter's nested mode runs it as a client of another display server, so this script
# brings up a virtual X server first and nests Mutter inside it. A nested Mutter is not a
# GNOME session: its output geometry comes from the X server it is nested in, its input
# path is the parent's, and the shell is absent. The assertions here are therefore about
# the protocol and the ladder, never about how the window looks on a GNOME desktop.
#
# # The runner prerequisite
#
# A nested Mutter renders through the graphics stack of the machine it runs on, which a
# stock GitHub-hosted runner does not usefully provide. The workflow gates this tier
# behind a repository variable rather than pretending a skipped job is a covered one. On
# a GNOME machine, `--attach wayland-0` checks the real session instead of a nested
# stand-in, and that is the stronger reading of this tier.

set -euo pipefail

readonly SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=packaging/ci/compositor/common.sh
source "$SCRIPT_DIR/common.sh"

COVERAGE_TIER="wayland-mutter"
COVERAGE_SESSION="nested-mutter"

readonly MUTTER_PARENT_SCREEN_DEFAULT="1920x1080x24"
readonly MUTTER_PARENT_DISPLAY_DEFAULT=":98"

usage() {
    cat <<'USAGE'
usage: mutter.sh [options]

Checks the Wayland/Mutter tier: starts a nested Mutter (or attaches to a running GNOME
session), loads both rspinyin addons into an fcitx5 session on it, and asserts what the
popup code path promises. Exit status 3 means the compositor could not be started, and 4
means the window scope was asked for but there is no window to check; see the header of
common.sh.

  --parent-display NAME   the X display the nested Mutter is a client of (default :98)
  --parent-screen WxHxD   that display's geometry (default 1920x1080x24)
USAGE
    compositor_print_common_usage
}

parent_display="$MUTTER_PARENT_DISPLAY_DEFAULT"
parent_screen="$MUTTER_PARENT_SCREEN_DEFAULT"

compositor_require_session_bus "$@"

compositor_reset_options
while [ $# -gt 0 ]; do
    if compositor_common_option "$@"; then
        shift "$COMPOSITOR_CONSUMED"
        continue
    fi
    case "$1" in
        --parent-display)
            [ $# -ge 2 ] || compositor_usage_error "--parent-display needs a value"
            parent_display="$2"
            shift 2
            ;;
        --parent-screen)
            [ $# -ge 2 ] || compositor_usage_error "--parent-screen needs a value"
            parent_screen="$2"
            shift 2
            ;;
        *)
            printf 'mutter.sh: unknown option %s\n' "$1" >&2
            usage >&2
            exit "$COMPOSITOR_USAGE"
            ;;
    esac
done

if [ "$COMPOSITOR_HELP" -eq 1 ]; then
    usage
    exit "$COMPOSITOR_OK"
fi

# ---------------------------------------------------------------------------
# The compositor
# ---------------------------------------------------------------------------

start_compositor() {
    if [ -n "$COMPOSITOR_ATTACH" ]; then
        COVERAGE_SESSION="attached-gnome-session"
        WAYLAND_DISPLAY="$COMPOSITOR_ATTACH"
        export WAYLAND_DISPLAY
        printf 'mutter: attaching to the session on %s\n' "$WAYLAND_DISPLAY" >&2
        return 0
    fi
    compositor_require_tool mutter "mutter"

    # The nested compositor is a client of a display server, so one has to exist first.
    # `--no-x11` below is about Xwayland inside the nested session, not about this.
    compositor_start_xvfb "$parent_display" "$parent_screen"
    export DISPLAY="$parent_display"
    COVERAGE_SESSION="nested-mutter on $parent_display"

    COMPOSITOR_COMPOSITOR_LOG="$COMPOSITOR_SCRATCH/mutter.log"
    # Mutter's nested path goes through the graphics stack of the machine it runs on.
    # Asking for the software rasteriser keeps the check about the compositor rather than
    # about whether the runner happens to have a driver.
    export LIBGL_ALWAYS_SOFTWARE="${LIBGL_ALWAYS_SOFTWARE:-1}"
    printf 'mutter: starting a nested Mutter inside %s\n' "$parent_display" >&2
    mutter --wayland --nested --no-x11 >"$COMPOSITOR_COMPOSITOR_LOG" 2>&1 &
    COMPOSITOR_COMPOSITOR_PID=$!
    compositor_wait_for_wayland_socket mutter 60
}

# ---------------------------------------------------------------------------
# The check
# ---------------------------------------------------------------------------

compositor_require_tool wayland-info "wayland-utils"
if [ -z "$COMPOSITOR_ATTACH" ]; then
    compositor_require_tool mutter "mutter"
    compositor_require_tool Xvfb "xvfb"
fi

compositor_prepare
start_compositor

compositor_require_wayland_global xdg_wm_base \
    "the popup tiers are built on xdg-shell and its positioner"
xdg_shell_version="$WAYLAND_GLOBAL_VERSION"
compositor_verified "the compositor advertises \`xdg_wm_base\` version $xdg_shell_version, which is what the T2 and T3 rungs need"

compositor_require_wayland_global wl_shm \
    "every tier draws through a shared-memory buffer"
compositor_verified "the compositor advertises \`wl_shm\` version $WAYLAND_GLOBAL_VERSION, so the buffers can be created at all"

compositor_require_wayland_global_absent zwlr_layer_shell_v1 \
    "its presence would send the ladder to T1 and leave the popup path this tier exists to test unexercised"
compositor_verified "the compositor does not offer \`zwlr_layer_shell_v1\`, so the ladder starts at the popup rung and the Mutter code path is the one that runs"

compositor_start_session
compositor_wait_for_addons
compositor_verified "both addons loaded into fcitx5 on $WAYLAND_DISPLAY, and the session mapped the staged builds"
COMPOSITOR_ENVIRONMENT="verified"

if compositor_wants_window; then
    compositor_wayland_window_scope
    # A nested Mutter's output is the parent X server's, so the edge cases a real GNOME
    # output produces are out of reach. An attached session is the user's own and carries
    # no such caveat.
    if [ -z "$COMPOSITOR_ATTACH" ]; then
        compositor_unverified "Mutter's clamping of a positioner that would cross a screen edge: reaching that path needs a window placed near an edge of a real GNOME output, and the nested session's geometry is the parent X server's"
    fi
fi

compositor_finish "$COMPOSITOR_OK"
