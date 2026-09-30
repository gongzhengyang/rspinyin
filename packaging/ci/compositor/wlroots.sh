#!/usr/bin/env bash
#
# wlroots.sh - the Wayland/wlroots tier check.
#
# The wlroots family -- Sway, Hyprland, labwc, river, niri -- is the one Wayland tier
# whose candidate window is absolutely positioned: `zwlr_layer_shell_v1` puts it on the
# overlay layer and its margins carry the screen coordinates, so the window lands exactly
# where the backend computed rather than where a positioner negotiated. The backend is
# `crates/ime-ui/src/platform/wayland/`, and this tier is its T1 rung.
#
# # Headless sway is the real thing for the half it covers
#
# `WLR_BACKENDS=headless` gives sway a virtual output and no GPU. What that leaves intact
# is exactly what this tier's code path depends on: `zwlr_layer_shell_v1` is a global of
# the compositor, not of its backend, so a headless sway advertises it, accepts a layer
# surface on the overlay layer, and applies the margins. What it does not provide is a
# physical presentation path, so this is not evidence about the GPU, about fractional
# scaling, or about multi-output placement; those need a real wlroots session.
#
# # Why the protocol listing is the first assertion
#
# The wlroots tier is defined by a protocol, not by a brand. Asserting that the session
# publishes `zwlr_layer_shell_v1` -- read from the compositor itself, through an ordinary
# client connection -- is what makes "this is a wlroots tier" a fact about the session
# rather than a fact about the package name that was installed.

set -euo pipefail

readonly SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=packaging/ci/compositor/common.sh
source "$SCRIPT_DIR/common.sh"

COVERAGE_TIER="wayland-wlroots"
COVERAGE_SESSION="headless-sway"

usage() {
    cat <<'USAGE'
usage: wlroots.sh [options]

Checks the Wayland/wlroots tier: starts a headless sway (or attaches to a running
wlroots session), loads both rspinyin addons into an fcitx5 session on it, and asserts
what the layer-shell code path promises. Exit status 3 means the compositor could not be
started, and 4 means the window scope was asked for but there is no window to check; see
the header of common.sh.
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
    printf 'wlroots.sh: unknown option %s\n' "$1" >&2
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
        COVERAGE_SESSION="attached-wlroots-session"
        WAYLAND_DISPLAY="$COMPOSITOR_ATTACH"
        export WAYLAND_DISPLAY
        printf 'wlroots: attaching to the session on %s\n' "$WAYLAND_DISPLAY" >&2
        return 0
    fi
    compositor_require_tool sway "sway"
    COMPOSITOR_COMPOSITOR_LOG="$COMPOSITOR_SCRATCH/sway.log"
    # An empty configuration: the check is about the protocol the compositor implements,
    # not about a user's keybindings, and a machine-local config would make the tier
    # depend on whose home directory it ran in.
    : >"$COMPOSITOR_SCRATCH/sway.conf"
    # The headless backend has no input devices and no GPU. `pixman` is the software
    # renderer, so the check does not depend on a driver being present on the runner.
    export WLR_BACKENDS=headless
    export WLR_LIBINPUT_NO_DEVICES=1
    export WLR_RENDERER="${WLR_RENDERER:-pixman}"
    printf 'wlroots: starting headless sway\n' >&2
    sway --config "$COMPOSITOR_SCRATCH/sway.conf" >"$COMPOSITOR_COMPOSITOR_LOG" 2>&1 &
    COMPOSITOR_COMPOSITOR_PID=$!
    compositor_wait_for_wayland_socket sway 40
}

# ---------------------------------------------------------------------------
# The check
# ---------------------------------------------------------------------------

compositor_require_tool wayland-info "wayland-utils"
if [ -z "$COMPOSITOR_ATTACH" ]; then
    compositor_require_tool sway "sway"
fi

compositor_prepare
start_compositor

# T1 is the tier this session is supposed to reach, so the protocol it needs is asserted
# rather than assumed from the compositor's name.
compositor_require_wayland_global zwlr_layer_shell_v1 \
    "the wlroots tier positions the candidate window with a layer surface"
layer_shell_version="$WAYLAND_GLOBAL_VERSION"
compositor_verified "the compositor advertises \`zwlr_layer_shell_v1\` version $layer_shell_version, which is the T1 rung of the ladder"

compositor_require_wayland_global xdg_wm_base \
    "the ladder's lower rungs and every other Wayland tier are built on xdg-shell"
xdg_shell_version="$WAYLAND_GLOBAL_VERSION"
compositor_verified "the compositor advertises \`xdg_wm_base\` version $xdg_shell_version, so the ladder has a rung below T1 to fall to"

compositor_require_wayland_global wl_shm \
    "every tier draws through a shared-memory buffer"
compositor_verified "the compositor advertises \`wl_shm\` version $WAYLAND_GLOBAL_VERSION, so the buffers can be created at all"

compositor_start_session
compositor_wait_for_addons
compositor_verified "both addons loaded into fcitx5 on $WAYLAND_DISPLAY, and the session mapped the staged builds"
COMPOSITOR_ENVIRONMENT="verified"

if compositor_wants_window; then
    compositor_wayland_window_scope
    # Headless means one virtual output and no presentation path, so the scaling and
    # multi-output cases are out of reach. An attached session is the user's own and
    # carries no such caveat.
    if [ -z "$COMPOSITOR_ATTACH" ]; then
        compositor_unverified "fractional scaling, multi-output placement and real GPU presentation: the headless backend has one virtual output and no presentation path"
    fi
fi

compositor_finish "$COMPOSITOR_OK"
