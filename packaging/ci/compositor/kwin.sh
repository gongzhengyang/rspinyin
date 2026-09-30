#!/usr/bin/env bash
#
# kwin.sh - the Wayland/KWin tier check.
#
# KWin implements xdg-shell but not `zwlr_layer_shell_v1`, so the ladder starts one rung
# below the wlroots tier: the candidate window is an `xdg_popup` anchored on the cursor
# through an `xdg_positioner`, and the compositor owns the flip and the slide when the
# window would cross a screen edge (`crates/ime-ui/src/platform/wayland/popup.rs`).
#
# # Why the missing layer-shell is asserted and not just noted
#
# The tier's claim is that the popup code path ran. That claim is only true while the
# rung above is unavailable: a session offering `zwlr_layer_shell_v1` would send the
# ladder to T1, the layer-shell path would run, and this job would report the KWin tier
# as covered while never having reached the code it exists to test. Asserting the global
# is absent is what keeps the tier's name honest.
#
# # The runner prerequisite
#
# `kwin_wayland --virtual` still allocates its buffers through the graphics stack, so
# this tier wants a machine with a working DRM/GBM path or a software renderer KWin
# accepts. A stock GitHub-hosted runner has neither, which is why the workflow gates this
# tier behind a repository variable rather than pretending a skipped job is a covered
# one. On a KDE machine, `--attach wayland-0` checks the real session instead of a nested
# stand-in, and that is the stronger reading of this tier.

set -euo pipefail

readonly SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=packaging/ci/compositor/common.sh
source "$SCRIPT_DIR/common.sh"

COVERAGE_TIER="wayland-kwin"
COVERAGE_SESSION="nested-kwin"

usage() {
    cat <<'USAGE'
usage: kwin.sh [options]

Checks the Wayland/KWin tier: starts a virtual KWin (or attaches to a running KDE
session), loads both rspinyin addons into an fcitx5 session on it, and asserts what the
xdg-popup code path promises. Exit status 3 means the compositor could not be started,
and 4 means the window scope was asked for but there is no window to check; see the
header of common.sh.
USAGE
    compositor_print_common_usage
}

compositor_require_session_bus "$@"

compositor_reset_options
while [ $# -gt 0 ]; do
    if compositor_common_option "$@"; then
        shift "$COMPOSITOR_CONSUMED"
        continue
    fi
    printf 'kwin.sh: unknown option %s\n' "$1" >&2
    usage >&2
    exit "$COMPOSITOR_USAGE"
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
        COVERAGE_SESSION="attached-kde-session"
        WAYLAND_DISPLAY="$COMPOSITOR_ATTACH"
        export WAYLAND_DISPLAY
        printf 'kwin: attaching to the session on %s\n' "$WAYLAND_DISPLAY" >&2
        return 0
    fi
    compositor_require_tool kwin_wayland "kwin-wayland"
    COMPOSITOR_COMPOSITOR_LOG="$COMPOSITOR_SCRATCH/kwin.log"
    # KWin is a Qt application as well as a compositor, and with no display of its own to
    # start from it needs to be told not to look for one.
    export QT_QPA_PLATFORM="${QT_QPA_PLATFORM:-offscreen}"
    printf 'kwin: starting a virtual KWin\n' >&2
    kwin_wayland --virtual --no-lockscreen --no-global-shortcuts \
        >"$COMPOSITOR_COMPOSITOR_LOG" 2>&1 &
    COMPOSITOR_COMPOSITOR_PID=$!
    compositor_wait_for_wayland_socket kwin_wayland 60
}

# ---------------------------------------------------------------------------
# The check
# ---------------------------------------------------------------------------

compositor_require_tool wayland-info "wayland-utils"
if [ -z "$COMPOSITOR_ATTACH" ]; then
    compositor_require_tool kwin_wayland "kwin-wayland"
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
compositor_verified "the compositor does not offer \`zwlr_layer_shell_v1\`, so the ladder starts at the popup rung and the KWin code path is the one that runs"

compositor_start_session
compositor_wait_for_addons
compositor_verified "both addons loaded into fcitx5 on $WAYLAND_DISPLAY, and the session mapped the staged builds"
COMPOSITOR_ENVIRONMENT="verified"

if compositor_wants_window; then
    compositor_wayland_window_scope
    # The virtual backend's output geometry is its own, not a monitor's, so the clamp
    # path is a limitation of a *started* session. An attached one is the user's real
    # session and carries no such caveat.
    if [ -z "$COMPOSITOR_ATTACH" ]; then
        compositor_unverified "KWin's own positioner clamping on a physical output: a virtual output's geometry differs from a real one, so the clamp path may not be reached here at all"
    fi
fi

compositor_finish "$COMPOSITOR_OK"
