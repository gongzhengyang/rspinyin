#!/usr/bin/env bash
#
# x11.sh - the X11 tier check.
#
# On X11 the candidate window is one override-redirect window positioned by screen
# coordinates, drawn into with `PutImage` and shaped through the SHAPE extension; the
# backend is `crates/ime-ui/src/platform/x11.rs`. This script brings up an X server,
# loads both addons into an fcitx5 session on it, and asserts what can be asserted from
# outside the process.
#
# # Why a virtual server answers half of this and not the other half
#
# `Xvfb` is a complete X server. It implements the extensions the backend uses, and it
# will map an override-redirect window, so the window half of the check is real. What it
# is not is a *session*: it has no compositing manager. The consequences are specific,
# and they are recorded rather than papered over:
#
#   * the ARGB transparency path is not exercised. The backend detects the missing
#     compositor through the `_NET_WM_CM_S<n>` selection owner and takes the documented
#     opaque-background fallback, so what this tier verifies here is the fallback.
#     `docs/dev/features.md` 0.5.5 registers the same limitation for the development
#     machine, which is also X11 without a compositor.
#   * background blur is a compositor feature and cannot be negotiated with one that is
#     not running.
#
# # The focus half
#
# The project's highest-severity defect is the candidate window taking the keyboard
# (AGENTS.md section 8 rule 20). On X11 that is observable rather than inferred: the
# window carries `WM_HINTS` with `input = False` and the override-redirect bit, and the
# server reports the input focus. Both are read here, and the project's own injector
# (`xtask testd --probe`) prints the server's answer as evidence beside them.

set -euo pipefail

readonly SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=packaging/ci/compositor/common.sh
source "$SCRIPT_DIR/common.sh"

COVERAGE_TIER="x11"
COVERAGE_SESSION="virtual-x-server"

readonly XVFB_SCREEN_DEFAULT="1920x1080x24"
readonly X11_DISPLAY_DEFAULT=":99"

# The extensions the X11 code path and the checks around it need, each with the reason
# it is required, so that a server lacking one produces a diagnosis rather than a failed
# assertion with no explanation.
readonly X11_REQUIRED_EXTENSIONS=(
    "SHAPE:the candidate window shapes its interactive region with XShapeCombineRegion"
    "XTEST:the project's own injector drives a real session through XTEST"
    "RANDR:output geometry and scale changes arrive through RandR"
    "Composite:the ARGB transparency path needs a compositing-capable server"
)

usage() {
    cat <<'USAGE'
usage: x11.sh [options]

Checks the X11 tier: starts an X server (or attaches to a running one), loads both
rspinyin addons into an fcitx5 session on it, and asserts what the X11 code path
promises. Exit status 3 means the server could not be started, and 4 means the window
scope was asked for but there is no window to check; see the header of common.sh.

  --display NAME    the X display to start or attach to (default :99)
  --screen WxHxD    geometry of the server this script starts (default 1920x1080x24)
USAGE
    compositor_print_common_usage
}

display="$X11_DISPLAY_DEFAULT"
screen="$XVFB_SCREEN_DEFAULT"

compositor_require_session_bus "$@"

compositor_reset_options
while [ $# -gt 0 ]; do
    if compositor_common_option "$@"; then
        shift "$COMPOSITOR_CONSUMED"
        continue
    fi
    case "$1" in
        --display)
            [ $# -ge 2 ] || compositor_usage_error "--display needs a value"
            display="$2"
            shift 2
            ;;
        --screen)
            [ $# -ge 2 ] || compositor_usage_error "--screen needs a value"
            screen="$2"
            shift 2
            ;;
        *)
            printf 'x11.sh: unknown option %s\n' "$1" >&2
            usage >&2
            exit "$COMPOSITOR_USAGE"
            ;;
    esac
done

if [ "$COMPOSITOR_HELP" -eq 1 ]; then
    usage
    exit "$COMPOSITOR_OK"
fi

# Attaching means the caller named the display, and the session is a real one rather
# than a virtual server this script brought up.
if [ -n "$COMPOSITOR_ATTACH" ]; then
    display="$COMPOSITOR_ATTACH"
    COVERAGE_SESSION="attached-x-server"
fi
export DISPLAY="$display"

# ---------------------------------------------------------------------------
# The X server
# ---------------------------------------------------------------------------

# Starts a virtual X server unless the caller named one that is already running.
start_x_server() {
    if [ -n "$COMPOSITOR_ATTACH" ]; then
        printf 'x11: attaching to the X server on %s\n' "$display" >&2
        return 0
    fi
    compositor_start_xvfb "$display" "$screen"
}

# Fails unless the server offers `extension`, naming what needs it.
require_extension() {
    local extension="${1%%:*}" why="${1#*:}"
    if ! xdpyinfo | grep -qE "(^|[[:space:]])${extension}([[:space:]]|\$)"; then
        compositor_die "$COMPOSITOR_DEFECT" \
            "the X server on $display does not offer the $extension extension: $why"
    fi
}

# ---------------------------------------------------------------------------
# The window half
# ---------------------------------------------------------------------------

# The candidate windows the server is showing, one id per line.
#
# The backend sets `WM_CLASS` to `rspinyin` for both the instance and the class name
# (`crates/ime-ui/src/platform/x11.rs`), which is what makes the window findable from
# outside without the plugin having to announce itself.
candidate_windows() {
    xdotool search --class rspinyin 2>/dev/null || true
}

# Asserts everything the X11 code path promises about the window it draws.
check_window_scope() {
    local -a windows=()
    mapfile -t windows < <(candidate_windows)

    if [ "${#windows[@]}" -eq 0 ]; then
        xwininfo -root -tree >&2 || true
        compositor_fail_without_window
    fi

    local window="${windows[0]}"
    printf 'x11: candidate window %s\n' "$window" >&2

    # Override-redirect is what keeps a window manager out of the placement: the backend
    # decides where the window goes, and no manager repositions or reparents it.
    if ! xwininfo -id "$window" | grep -q 'Override Redirect State: yes'; then
        xwininfo -id "$window" >&2 || true
        compositor_die "$COMPOSITOR_DEFECT" \
            "candidate window $window is not override-redirect; a window manager would place it"
    fi

    # `input = False` in WM_HINTS is what tells a window manager this window must not get
    # the keyboard. It is the X11 half of the same structural guarantee the Wayland tiers
    # make with `keyboard_interactivity = none`.
    if ! xprop -id "$window" WM_HINTS | grep -q 'Client accepts input or input focus: False'; then
        xprop -id "$window" WM_HINTS >&2 || true
        compositor_die "$COMPOSITOR_DEFECT" \
            "candidate window $window accepts input focus; it must carry input = False"
    fi

    # And the server's own answer, which is the observable form of the same claim.
    local focused
    focused="$(xdotool getwindowfocus 2>/dev/null || echo none)"
    local candidate
    for candidate in "${windows[@]}"; do
        if [ "$focused" = "$candidate" ]; then
            compositor_die "$COMPOSITOR_DEFECT" \
                "the candidate window $candidate holds the input focus"
        fi
    done
    printf 'x11: the server reports the input focus on %s, not on a candidate window\n' \
        "$focused" >&2

    compositor_verified "the candidate window exists as an override-redirect window (\`$window\`)"
    compositor_verified "it carries \`WM_HINTS\` with \`input = False\`"
    compositor_verified "the server's input focus is $focused, which is not the candidate window"
    COMPOSITOR_WINDOW="verified"
}

# ---------------------------------------------------------------------------
# The check
# ---------------------------------------------------------------------------

for tool in xdpyinfo xprop xwininfo; do
    compositor_require_tool "$tool" "x11-utils"
done
compositor_require_tool xdotool "xdotool"
if [ -z "$COMPOSITOR_ATTACH" ]; then
    compositor_require_tool Xvfb "xvfb"
fi

compositor_prepare
start_x_server

for entry in "${X11_REQUIRED_EXTENSIONS[@]}"; do
    require_extension "$entry"
done
compositor_verified "the X server on $display offers SHAPE, XTEST, RANDR and Composite"

# A compositor is what makes the ARGB path reachable. Its absence is not a failure --
# the documented fallback is a correct product behaviour -- but it does bound what this
# run covered, so it is read and recorded rather than assumed.
#
# What is read is the root window's `_NET_WM_CM_S0` property, the EWMH form of the claim.
# The backend's own probe reads the selection owner of the atom of the same name
# (`crates/ime-ui/src/platform/x11.rs`), which is the authoritative half; a compositor
# that took the selection and did not publish the property would be reported here as
# absent, and that is the direction to err in -- it understates the coverage rather than
# overstating it.
if xprop -root _NET_WM_CM_S0 2>/dev/null | grep -q 'window id'; then
    COVERAGE_SESSION="$COVERAGE_SESSION, compositor present"
    compositor_verified "the root window carries \`_NET_WM_CM_S0\`, so a compositing manager is running"
else
    COVERAGE_SESSION="$COVERAGE_SESSION, no compositor"
    compositor_unverified "real ARGB transparency and background blur: no compositing manager is running, so the backend takes its documented opaque fallback and the ARGB path is not exercised"
fi

compositor_start_session
compositor_wait_for_addons
compositor_verified "both addons loaded into fcitx5 on $display, and the session mapped the staged builds"
COMPOSITOR_ENVIRONMENT="verified"

if compositor_wants_window; then
    check_window_scope
    # The project's own injector, reporting the server's state in its own vocabulary.
    # Evidence rather than an assertion: a server without XTEST is an environment this
    # half cannot drive, and that is worth recording rather than failing over.
    if probe_output="$(cd -- "$(compositor_repo_root)" && cargo run --quiet -p xtask -- testd --probe 2>&1)"; then
        printf '%s\n' "$probe_output" >>"$COMPOSITOR_SCRATCH/xtask-testd-probe.txt"
        compositor_verified "the project's own injector connected to $display and reported: $(printf '%s' "$probe_output" | tr '\n' ' ')"
    else
        compositor_unverified "the project's own injector could not probe $display; key injection into this session is unproven"
    fi
fi

compositor_finish "$COMPOSITOR_OK"
