#!/usr/bin/env bash
#
# common.sh - the shared half of the compositor tier checks.
#
# Sourced by every script in this directory, never executed on its own. Each tier script
# calls `compositor_prepare`, starts its own compositor, calls `compositor_start_session`
# and `compositor_wait_for_addons`, makes its own assertions, and ends with
# `compositor_finish`.
#
# # What a tier check is
#
# The candidate window positions itself differently on each display server: an
# override-redirect window on X11, a `zwlr_layer_surface_v1` on wlroots, and an
# `xdg_popup` with a positioner on KWin and Mutter. Those are four code paths in
# `crates/ime-ui/src/platform/`, and each one needs its own session to be exercised.
#
# A tier check therefore has two halves, and they are not equally available:
#
#   environment  the compositor starts, it offers the protocols that tier's code path
#                needs, and both addons load into it. Nothing about the plugin can make
#                this half pass or fail; it says whether the session is the session the
#                tier claims to be.
#
#   window       the candidate window exists, it is positioned, and it never holds the
#                keyboard. This half needs the plugin to actually present a window, and
#                it is the half that carries the project's highest-severity defect class
#                (AGENTS.md section 8 rule 20: taking keyboard focus).
#
# `--scope` names the half a caller wants, and the exit status describes exactly that
# half. Nothing here reports a half it did not run: a half that could not be exercised is
# named as unverified in the coverage block and is never counted as a pass. That is the
# point of the whole directory -- a green tier job that quietly asserted nothing is worse
# than a red one, because it reads as coverage the project does not have.
#
# # Exit status
#
#   0  every assertion in the requested scope ran and passed
#   1  an assertion failed: a defect in the plugin or in that tier's code path
#   2  usage or environment error in the check itself
#   3  the compositor could not be started, so no assertion could run at all;
#      reported under `dist/verify/compositor-unavailable`
#   4  the window scope was requested but the plugin presents no window to check;
#      reported under `platform/compositor/unsupported`
#
# 3 and 4 are the two ways a tier can be uncovered and they are deliberately distinct.
# 3 is the machine's problem and is fixed by giving the tier a compositor; 4 is the
# plugin's problem and is fixed in the plugin. A single "it failed" status would make the
# two indistinguishable in a job log, which is what the codes exist to prevent.
#
# # The coverage block
#
# Every run ends with a block naming what was asserted and what was not, followed by one
# machine-readable line:
#
#   coverage tier=<id> session=<kind> scope=<scope> environment=<state> window=<state>
#
# The states are `verified`, `failed` and `skipped`, and only `verified` counts as
# coverage. A half that could not be exercised is named in the block's "NOT verified
# here" list with its reason, so a reader never has to infer the boundary from what is
# absent. The workflow's `coverage` job collects the lines.
#
# # Nothing here needs the network
#
# The checks build the addons from the working tree and stage them into a private
# directory; no package is fetched and no service is contacted. The one thing that may
# need the network is `cargo` itself, on a machine whose registry cache is cold, and that
# is the build's business rather than the check's -- the addon under test has no network
# capability at all (BUDGET-NET-01).

# Exit statuses, named so a reader does not have to hold the table above in their head.
readonly COMPOSITOR_OK=0
readonly COMPOSITOR_DEFECT=1
readonly COMPOSITOR_USAGE=2
readonly COMPOSITOR_UNAVAILABLE=3
readonly COMPOSITOR_NO_WINDOW=4

# The diagnostic codes this directory reports. `dist/verify/compositor-unavailable` is
# the delivery channel's own code for an environment that could not be brought up;
# `platform/compositor/unsupported` is the plugin's own code for a session that can host
# no candidate window, and is spelled the same way here so one grep finds both halves.
readonly COMPOSITOR_UNAVAILABLE_CODE="dist/verify/compositor-unavailable"
readonly COMPOSITOR_ADDON_CODE="dist/verify/addon-not-loaded"
readonly COMPOSITOR_NO_WINDOW_CODE="platform/compositor/unsupported"

# The two addons a session must have loaded before anything about it can be asserted.
# The engine decodes; the UI addon owns the candidate window. A tier check that loaded
# only the engine has verified nothing about the window path it exists to test.
readonly COMPOSITOR_ADDON_IDS=("rspinyin" "rspinyin-ui")

# The plugin's own statement that it is not drawing a window yet: the UI addon's
# initialisation sequence reports every step that has not landed, and the platform probe
# and the UI start-up are two of them. Both are matched, because a session that has the
# probe but no window is in the same position as one that has neither.
readonly COMPOSITOR_PENDING_PLATFORM="lifecycle/pending: platform awaits"
readonly COMPOSITOR_PENDING_UI="lifecycle/pending: ui-startup awaits"

# Coverage state. A tier script fills these in before calling `compositor_finish`.
COVERAGE_TIER=""
COVERAGE_SESSION=""
COVERAGE_SCOPE=""
COVERAGE_ENVIRONMENT="skipped"
COVERAGE_WINDOW="skipped"
COVERAGE_VERIFIED=()
COVERAGE_UNVERIFIED=()

# ---------------------------------------------------------------------------
# Diagnostics
# ---------------------------------------------------------------------------

# Prints `message` to standard error and exits with `status`.
#
# A failure after the check has started still writes its coverage block, with the half
# that died marked `failed`. Without that, the tier that failed would be the one tier
# with no record of what it did and did not assert -- which is exactly the record a
# reader needs to tell "the compositor would not start" from "the plugin misbehaved".
compositor_die() {
    local status="$1"
    shift
    printf '%s: %s\n' "${COVERAGE_TIER:-compositor}" "$*" >&2
    if [ -n "${COMPOSITOR_SCRATCH:-}" ] && [ -d "${COMPOSITOR_SCRATCH:-}" ]; then
        if [ "$COVERAGE_ENVIRONMENT" != "verified" ]; then
            COVERAGE_ENVIRONMENT="failed"
        fi
        if compositor_wants_window && [ "$COVERAGE_WINDOW" != "verified" ]; then
            COVERAGE_WINDOW="failed"
        fi
        compositor_finish "$status"
    fi
    exit "$status"
}

# Prints a usage error and exits 2.
compositor_usage_error() {
    compositor_die "$COMPOSITOR_USAGE" "$*"
}

# Fails unless `tool` is on PATH, naming the package that provides it.
compositor_require_tool() {
    local tool="$1" package="$2"
    if ! command -v "$tool" >/dev/null 2>&1; then
        compositor_die "$COMPOSITOR_UNAVAILABLE" \
            "$COMPOSITOR_UNAVAILABLE_CODE: '$tool' is not on PATH; install $package"
    fi
}

# Prints the last `count` lines of `log`, for a failure message.
compositor_log_tail() {
    local log="$1" count="${2:-25}"
    printf -- '--- %s (last %s lines) ---\n' "$log" "$count" >&2
    tail -n "$count" "$log" >&2 || true
    printf -- '--- end of %s ---\n' "$log" >&2
}

# ---------------------------------------------------------------------------
# The scratch root
# ---------------------------------------------------------------------------

# The repository root, resolved from this file's own location.
compositor_repo_root() {
    local here
    here="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
    cd -- "$here/../../.." && pwd
}

# Creates the private directory the check works in and registers its cleanup.
#
# Everything the session needs lives under here: the XDG bases Fcitx5 is pointed at, the
# staged addons, and the logs. Staging rather than installing is what makes the check
# about the working tree: a machine with a copy of the addon already installed would
# otherwise let a session load that copy and pass for code the check never ran.
#
# `COMPOSITOR_SCRATCH_DIR` puts the directory somewhere a caller can find afterwards,
# which is what lets a workflow upload the session log as evidence. Without it the
# directory goes to the usual temporary location and is removed when the check ends.
compositor_make_scratch() {
    local base="${COMPOSITOR_SCRATCH_DIR:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}}"
    mkdir -p -- "$base" || compositor_die "$COMPOSITOR_USAGE" \
        "cannot create the scratch parent $base"
    COMPOSITOR_SCRATCH="$(mktemp -d "$base/rspinyin-compositor-XXXXXX")"
    COMPOSITOR_SESSION_LOG="$COMPOSITOR_SCRATCH/session.log"
    COMPOSITOR_SESSION_PID=""
    COMPOSITOR_COMPOSITOR_PID=""
    COMPOSITOR_COMPOSITOR_LOG=""
    trap 'compositor_cleanup' EXIT
}

# Stops whatever the check started and removes the scratch root.
#
# Registered as an EXIT trap, so it runs on a failure as well: a check that leaves a
# headless compositor behind would leave the next one on the same runner fighting it for
# the socket name.
compositor_cleanup() {
    local status=$?
    compositor_stop_process "$COMPOSITOR_SESSION_PID"
    compositor_stop_process "$COMPOSITOR_COMPOSITOR_PID"
    compositor_stop_process "${COMPOSITOR_XVFB_PID:-}"
    if [ -n "${COMPOSITOR_EXTRA_PIDS:-}" ]; then
        local pid
        for pid in $COMPOSITOR_EXTRA_PIDS; do
            compositor_stop_process "$pid"
        done
    fi
    if [ -n "${COMPOSITOR_SCRATCH:-}" ]; then
        if [ "${COMPOSITOR_KEEP_SCRATCH:-0}" -eq 1 ]; then
            printf 'compositor: keeping %s\n' "$COMPOSITOR_SCRATCH" >&2
        else
            rm -rf -- "$COMPOSITOR_SCRATCH"
        fi
    fi
    return "$status"
}

# Terminates `pid` if it names a live process, and reaps it.
compositor_stop_process() {
    local pid="${1:-}"
    [ -n "$pid" ] || return 0
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
}

# Whether `pid` is still running, rather than merely still present.
#
# `kill -0` answers "may I signal it", which a zombie still satisfies: a child that has
# exited but not yet been reaped is signalled happily. A wait loop built on `kill -0`
# alone would therefore sit out its whole timeout against a session that died on its
# first line, and report the timeout rather than the death. The process state is read
# instead.
compositor_process_alive() {
    local pid="${1:-}" state
    [ -n "$pid" ] || return 1
    kill -0 "$pid" 2>/dev/null || return 1
    state="$(awk '{print $3}' "/proc/$pid/stat" 2>/dev/null || true)"
    [ -n "$state" ] && [ "$state" != "Z" ]
}

# ---------------------------------------------------------------------------
# The addons under test
# ---------------------------------------------------------------------------

# Builds the two addons, unless the caller staged a build already.
compositor_build_addons() {
    if [ "${COMPOSITOR_SKIP_BUILD:-0}" -eq 1 ]; then
        return 0
    fi
    local root
    root="$(compositor_repo_root)"
    printf 'compositor: building the addons\n' >&2
    (
        cd -- "$root" || exit "$COMPOSITOR_USAGE"
        cargo build --release -p ime-fcitx5 --features fcitx5-host
        cargo build --release -p ime-ui-addon --features fcitx5-host
    ) || compositor_die "$COMPOSITOR_USAGE" \
        "the addon build failed; the host-ABI build needs libfcitx5core-dev, libfcitx5utils-dev and libfcitx5config-dev"
}

# Stages the built addons and their descriptors into the scratch root.
#
# The layout mirrors the one the test harness builds for itself in
# `xtask/src/testd/sandbox.rs`, for the same reason: Fcitx5 resolves `Library=` through
# its own StandardPath, which reads `FCITX_ADDON_DIRS`, and a session pointed at the
# system addon directory would load whichever build was installed there last.
compositor_stage_addons() {
    local root library
    root="$(compositor_repo_root)"
    mkdir -p -- "$COMPOSITOR_SCRATCH/addon" \
        "$COMPOSITOR_SCRATCH/share/fcitx5/addon" \
        "$COMPOSITOR_SCRATCH/share/fcitx5/inputmethod" \
        "$COMPOSITOR_SCRATCH/xdg/data" \
        "$COMPOSITOR_SCRATCH/xdg/config" \
        "$COMPOSITOR_SCRATCH/xdg/cache" \
        "$COMPOSITOR_SCRATCH/xdg/runtime"
    chmod 700 -- "$COMPOSITOR_SCRATCH/xdg/runtime"

    for library in librspinyin.so librspinyin_ui.so; do
        if [ ! -f "$root/target/release/$library" ]; then
            compositor_die "$COMPOSITOR_USAGE" \
                "target/release/$library is missing; build it or drop --skip-build"
        fi
        cp -- "$root/target/release/$library" "$COMPOSITOR_SCRATCH/addon/$library"
    done
    cp -- "$root/packaging/fcitx5/rspinyin.conf" \
        "$root/packaging/fcitx5/rspinyin-ui.conf" \
        "$COMPOSITOR_SCRATCH/share/fcitx5/addon/"
    cp -- "$root/packaging/fcitx5/rspinyin-im.conf" \
        "$COMPOSITOR_SCRATCH/share/fcitx5/inputmethod/"
}

# Points the session at the staged addons, and writes the same values to a file.
#
# The variables are exported rather than passed to one child, because the compositor
# needs `XDG_RUNTIME_DIR` too: it is where the compositor puts its socket and where the
# session looks for it, and the two only meet if both read the same value.
#
# `--attach` leaves `XDG_RUNTIME_DIR` alone. The socket of a session that is already
# running is in that session's runtime directory, and overriding it would hide the
# display server the caller asked to be checked against.
compositor_export_session_env() {
    local file="$COMPOSITOR_SCRATCH/session.env"
    export XDG_DATA_HOME="$COMPOSITOR_SCRATCH/xdg/data"
    export XDG_CONFIG_HOME="$COMPOSITOR_SCRATCH/xdg/config"
    export XDG_CACHE_HOME="$COMPOSITOR_SCRATCH/xdg/cache"
    if [ -z "${COMPOSITOR_ATTACH:-}" ]; then
        export XDG_RUNTIME_DIR="$COMPOSITOR_SCRATCH/xdg/runtime"
    fi
    export FCITX_ADDON_DIRS="$COMPOSITOR_SCRATCH/addon"
    export FCITX_DATA_DIRS="$COMPOSITOR_SCRATCH/share:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"
    export FCITX_CONFIG_DIRS="$COMPOSITOR_SCRATCH/xdg/config:${XDG_CONFIG_DIRS:-/etc/xdg}"
    {
        printf 'XDG_DATA_HOME=%s\n' "$XDG_DATA_HOME"
        printf 'XDG_CONFIG_HOME=%s\n' "$XDG_CONFIG_HOME"
        printf 'XDG_CACHE_HOME=%s\n' "$XDG_CACHE_HOME"
        printf 'XDG_RUNTIME_DIR=%s\n' "${XDG_RUNTIME_DIR:-<inherited>}"
        printf 'FCITX_ADDON_DIRS=%s\n' "$FCITX_ADDON_DIRS"
        printf 'FCITX_DATA_DIRS=%s\n' "$FCITX_DATA_DIRS"
        printf 'FCITX_CONFIG_DIRS=%s\n' "$FCITX_CONFIG_DIRS"
    } >"$file"
}

# Everything a tier script needs before it starts its own compositor.
compositor_prepare() {
    compositor_make_scratch
    compositor_build_addons
    compositor_stage_addons
    compositor_export_session_env
}

# Re-runs the calling script inside a private session bus, once.
#
# Fcitx5 is a D-Bus client: its own addon activation, the input-method portal and the
# config tool all go through the session bus, and a session started without one logs a
# warning and behaves differently from the one a user has. Rather than requiring every
# caller to remember `dbus-run-session`, the check provides its own bus.
#
# It is the *first* thing a tier script calls, before the scratch directory exists: the
# re-exec replaces the process, so anything created before it would belong to a process
# that no longer runs the trap that cleans it up.
#
# Called as `compositor_require_session_bus "$@"` from the top level of a tier script,
# so that `$@` is the script's own arguments and the re-exec passes them on unchanged.
compositor_require_session_bus() {
    if [ -n "${DBUS_SESSION_BUS_ADDRESS:-}" ] || [ "${COMPOSITOR_UNDER_BUS:-0}" -eq 1 ]; then
        return 0
    fi
    if ! command -v dbus-run-session >/dev/null 2>&1; then
        printf 'compositor: dbus-run-session is not installed; the session will start without a bus\n' >&2
        return 0
    fi
    export COMPOSITOR_UNDER_BUS=1
    exec dbus-run-session -- bash "${BASH_SOURCE[1]}" "$@"
}

# ---------------------------------------------------------------------------
# The virtual X server
# ---------------------------------------------------------------------------

# Starts a virtual X server on `display` and waits for it to answer.
#
# Two tiers need one: the X11 tier, where the virtual server *is* the session under
# test, and the Mutter tier, where the nested compositor needs an X display to nest
# inside. It lives here rather than in either of them because a second copy would be a
# second definition of what "the server is up" means, and the two would drift.
#
# `-nolisten tcp` keeps the server on its Unix socket: the checks have no use for a
# listening TCP port, and a server that opened one would be a network capability of the
# kind the project's own promise forbids (BUDGET-NET-01).
compositor_start_xvfb() {
    local display="$1" screen="$2"
    compositor_require_tool Xvfb "xvfb"
    compositor_require_tool xdpyinfo "x11-utils"
    COMPOSITOR_XVFB_LOG="$COMPOSITOR_SCRATCH/xvfb.log"
    printf 'compositor: starting Xvfb on %s (%s)\n' "$display" "$screen" >&2
    Xvfb "$display" -screen 0 "$screen" -nolisten tcp \
        +extension SHAPE +extension XTEST +extension RANDR +extension Composite \
        >"$COMPOSITOR_XVFB_LOG" 2>&1 &
    COMPOSITOR_XVFB_PID=$!

    local deadline=$((SECONDS + 30))
    while [ "$SECONDS" -lt "$deadline" ]; do
        if DISPLAY="$display" xdpyinfo >/dev/null 2>&1; then
            return 0
        fi
        if ! compositor_process_alive "$COMPOSITOR_XVFB_PID"; then
            compositor_log_tail "$COMPOSITOR_XVFB_LOG"
            compositor_die "$COMPOSITOR_UNAVAILABLE" \
                "$COMPOSITOR_UNAVAILABLE_CODE: Xvfb exited before serving $display"
        fi
        sleep 0.5
    done
    compositor_log_tail "$COMPOSITOR_XVFB_LOG"
    compositor_die "$COMPOSITOR_UNAVAILABLE" \
        "$COMPOSITOR_UNAVAILABLE_CODE: no X server answered on $display within 30s"
}

# ---------------------------------------------------------------------------
# The Wayland session
# ---------------------------------------------------------------------------

# Waits for a compositor to create its Wayland socket and points the session at it.
#
# The socket name is discovered rather than assumed. A compositor takes `wayland-0` when
# that name is free and the next number when it is not, and a check that hardcoded
# `wayland-0` would silently attach to whatever else on the machine already owned it --
# which is the one failure mode that makes a tier check test the wrong compositor.
compositor_wait_for_wayland_socket() {
    local compositor="$1" timeout="${2:-40}" deadline socket
    deadline=$((SECONDS + timeout))
    while [ "$SECONDS" -lt "$deadline" ]; do
        socket="$(find "${XDG_RUNTIME_DIR:?}" -maxdepth 1 -type s -name 'wayland-*' -print -quit 2>/dev/null || true)"
        if [ -n "$socket" ]; then
            WAYLAND_DISPLAY="$(basename -- "$socket")"
            export WAYLAND_DISPLAY
            printf 'compositor: %s is listening on %s\n' "$compositor" "$WAYLAND_DISPLAY" >&2
            return 0
        fi
        if [ -n "${COMPOSITOR_COMPOSITOR_PID:-}" ] \
            && ! compositor_process_alive "$COMPOSITOR_COMPOSITOR_PID"; then
            compositor_log_tail "$COMPOSITOR_COMPOSITOR_LOG"
            compositor_die "$COMPOSITOR_UNAVAILABLE" \
                "$COMPOSITOR_UNAVAILABLE_CODE: $compositor exited before creating a Wayland socket"
        fi
        sleep 0.5
    done
    compositor_log_tail "$COMPOSITOR_COMPOSITOR_LOG"
    compositor_die "$COMPOSITOR_UNAVAILABLE" \
        "$COMPOSITOR_UNAVAILABLE_CODE: $compositor created no Wayland socket in $XDG_RUNTIME_DIR within ${timeout}s"
}

# The version a compositor offers for `interface`, or nothing when it offers none.
#
# Wayland has no request that names the compositor, so what a session *is* has to be
# read off the globals it publishes. `wayland-info` connects as an ordinary client and
# prints them; that listing is the only first-hand evidence available about which
# protocols the session actually implements, as opposed to which ones its documentation
# says it does.
# A compositor that cannot be reached at all, and one that simply lacks the interface,
# both answer with nothing. The two are told apart earlier, by the socket wait and by the
# tool check, so this function stays a lookup rather than becoming a second diagnosis.
compositor_wayland_global_version() {
    local interface="$1" listing version
    listing="$(wayland-info 2>/dev/null || true)"
    version="$(printf '%s\n' "$listing" \
        | sed -n "s/^interface: '${interface}',[[:space:]]*version:[[:space:]]*\([0-9]*\).*/\1/p")"
    # The first line only: a compositor announces one global per interface name, and a
    # stray second match would be a second object rather than a second version.
    printf '%s\n' "${version%%$'\n'*}"
}

# Fails unless the compositor offers `interface`, naming what needs it.
#
# The version is left in `WAYLAND_GLOBAL_VERSION` for the caller to record. On failure the
# whole listing is printed: "the compositor does not offer X" and "the listing could not
# be read" produce the same empty answer, and only the listing tells them apart.
compositor_require_wayland_global() {
    local interface="$1" why="$2"
    WAYLAND_GLOBAL_VERSION="$(compositor_wayland_global_version "$interface")"
    if [ -z "$WAYLAND_GLOBAL_VERSION" ]; then
        compositor_dump_wayland_globals
        compositor_die "$COMPOSITOR_DEFECT" \
            "the compositor on ${WAYLAND_DISPLAY:-the current session} does not offer $interface: $why"
    fi
}

# Prints the globals the session publishes, for a failure message.
compositor_dump_wayland_globals() {
    printf -- '--- wayland-info on %s ---\n' "${WAYLAND_DISPLAY:-the current session}" >&2
    # stdout is folded onto stderr so the listing lands with the message that needed it;
    # stderr already points there.
    wayland-info 1>&2 || true
    printf -- '--- end of wayland-info ---\n' >&2
}

# Fails when the compositor offers `interface`, naming what its presence would mean.
#
# This is not symmetry for its own sake. A tier check claims that a particular code path
# was exercised, and on the popup tiers that claim is only true while the rung above is
# unavailable: `zwlr_layer_shell_v1` in a KWin or Mutter session would send the ladder to
# T1, the layer-shell path would run, and the popup path the tier exists to test would
# never be reached -- while the job reported the tier as covered.
compositor_require_wayland_global_absent() {
    local interface="$1" why="$2"
    WAYLAND_GLOBAL_VERSION="$(compositor_wayland_global_version "$interface")"
    if [ -n "$WAYLAND_GLOBAL_VERSION" ]; then
        compositor_die "$COMPOSITOR_DEFECT" \
            "the compositor on ${WAYLAND_DISPLAY:-the current session} offers $interface version $WAYLAND_GLOBAL_VERSION: $why"
    fi
}

# ---------------------------------------------------------------------------
# The session
# ---------------------------------------------------------------------------

# Starts Fcitx5 against the staged addons, with its output going to the session log.
#
# No `--daemon`: the check wants the process as a child it can stop, and it wants the log
# as a file it can read while the process is still running. Readiness is decided by that
# log rather than by a fixed sleep, because a sleep is a race with the machine.
compositor_start_session() {
    : >"$COMPOSITOR_SESSION_LOG"
    fcitx5 >"$COMPOSITOR_SESSION_LOG" 2>&1 &
    COMPOSITOR_SESSION_PID=$!
}

# Whether the session log carries Fcitx5's own `Loaded addon <id>` line for `id`.
#
# The trailing boundary is load-bearing: `rspinyin` is a prefix of `rspinyin-ui`, so a
# plain substring match would report the engine as loaded on a session where only the UI
# addon arrived.
compositor_log_has_addon() {
    local id="$1"
    grep -qE "Loaded addon ${id}([^-A-Za-z0-9_]|\$)" "$COMPOSITOR_SESSION_LOG"
}

# Waits for both addons to be live, or fails saying which half of which one did not come.
#
# Two lines per addon are required, and they say different things. Fcitx5 writes
# `Loaded addon <id>` once the shared object's factory has been called; the plugin's own
# glue writes `<id>: addon loaded` only after the C ABI handshake and the whole
# initialisation sequence reached their end. Waiting for the host's line alone would call
# a session ready whose plugin had declined on its first step, which is the case this
# check exists to catch.
compositor_wait_for_addons() {
    local timeout="${1:-40}" deadline addon missing
    deadline=$((SECONDS + timeout))
    while :; do
        if ! compositor_process_alive "$COMPOSITOR_SESSION_PID"; then
            compositor_log_tail "$COMPOSITOR_SESSION_LOG"
            compositor_die "$COMPOSITOR_DEFECT" \
                "fcitx5 exited before loading its addons"
        fi
        missing=""
        for addon in "${COMPOSITOR_ADDON_IDS[@]}"; do
            if ! compositor_log_has_addon "$addon"; then
                missing="the host never reported 'Loaded addon $addon'"
            elif ! grep -qF "$addon: addon loaded" "$COMPOSITOR_SESSION_LOG"; then
                missing="the plugin never reported '$addon: addon loaded'"
            fi
        done
        if [ -z "$missing" ]; then
            return 0
        fi
        if [ "$SECONDS" -ge "$deadline" ]; then
            compositor_log_tail "$COMPOSITOR_SESSION_LOG"
            compositor_die "$COMPOSITOR_DEFECT" \
                "$COMPOSITOR_ADDON_CODE: within ${timeout}s, $missing"
        fi
        sleep 0.5
    done
}

# Fails unless the running session mapped the staged libraries rather than others.
#
# `/proc/<pid>/maps` is the only offline proof that the session is running the build
# under test; without it a machine with the addon installed system-wide could pass this
# check for code it never loaded.
compositor_assert_own_libraries() {
    local maps="/proc/$COMPOSITOR_SESSION_PID/maps" library
    for library in librspinyin.so librspinyin_ui.so; do
        if ! grep -qF "$COMPOSITOR_SCRATCH/addon/$library" "$maps"; then
            compositor_die "$COMPOSITOR_DEFECT" \
                "the session did not map the staged $library; it loaded another build"
        fi
    done
}

# Whether the plugin reports that its window backend has not been wired up.
#
# This is the plugin's own statement, read from its own diagnostics, and it is what makes
# the window half's unavailability a fact rather than an assumption: the check does not
# decide that no window can exist, it reads the reason the plugin gives.
compositor_window_backend_pending() {
    grep -qF "$COMPOSITOR_PENDING_PLATFORM" "$COMPOSITOR_SESSION_LOG" \
        || grep -qF "$COMPOSITOR_PENDING_UI" "$COMPOSITOR_SESSION_LOG"
}

# Fails unless no line of the session log carries `pattern`.
compositor_assert_absent_from_log() {
    local pattern="$1" what="$2"
    if grep -qF -- "$pattern" "$COMPOSITOR_SESSION_LOG"; then
        grep -F -- "$pattern" "$COMPOSITOR_SESSION_LOG" >&2 || true
        compositor_die "$COMPOSITOR_DEFECT" "$what"
    fi
}

# ---------------------------------------------------------------------------
# Coverage
# ---------------------------------------------------------------------------

# Records an assertion that ran and held.
compositor_verified() {
    COVERAGE_VERIFIED+=("$1")
}

# Records an assertion that did not run, and why.
compositor_unverified() {
    COVERAGE_UNVERIFIED+=("$1")
}

# Writes the coverage block and exits with `status`.
#
# The block is written whatever the status: a run that failed still has to say what it
# did and did not reach, or the failure is read as covering everything it did not.
#
# It goes to three places on purpose. `$GITHUB_STEP_SUMMARY` is what a person reads;
# stderr is what a developer running the script by hand reads; and a file under the
# scratch root is what the workflow's `coverage` job reads, because a step summary is
# per-job and the job that decides whether a tier counts as covered is a different one.
compositor_finish() {
    local status="$1" line block
    block="$(
        {
            printf '\n### %s tier (%s)\n\n' "$COVERAGE_TIER" "$COVERAGE_SESSION"
            printf 'Scope requested: `%s` | environment: **%s** | window: **%s**\n\n' \
                "$COVERAGE_SCOPE" "$COVERAGE_ENVIRONMENT" "$COVERAGE_WINDOW"
            if [ "${#COVERAGE_VERIFIED[@]}" -gt 0 ]; then
                printf 'Verified:\n\n'
                for line in "${COVERAGE_VERIFIED[@]}"; do
                    printf -- '- %s\n' "$line"
                done
                printf '\n'
            fi
            if [ "${#COVERAGE_UNVERIFIED[@]}" -gt 0 ]; then
                printf 'NOT verified here:\n\n'
                for line in "${COVERAGE_UNVERIFIED[@]}"; do
                    printf -- '- %s\n' "$line"
                done
                printf '\n'
            fi
            printf '```\ncoverage tier=%s session=%s scope=%s environment=%s window=%s exit=%s\n```\n' \
                "$COVERAGE_TIER" "$COVERAGE_SESSION" "$COVERAGE_SCOPE" \
                "$COVERAGE_ENVIRONMENT" "$COVERAGE_WINDOW" "$status"
        }
    )"
    printf '%s\n' "$block" >&2
    if [ -n "${COMPOSITOR_SCRATCH:-}" ] && [ -d "${COMPOSITOR_SCRATCH:-}" ]; then
        printf '%s\n' "$block" >"$COMPOSITOR_SCRATCH/coverage.md"
    fi
    if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
        printf '%s\n' "$block" >>"$GITHUB_STEP_SUMMARY"
    fi
    exit "$status"
}

# ---------------------------------------------------------------------------
# Argument parsing
# ---------------------------------------------------------------------------

# Handles one of the options every tier script shares.
#
# `$1` is the option and `$2` its value when the option takes one. Returns 0 when the
# option was one of ours, having set `COMPOSITOR_CONSUMED` to the number of arguments to
# shift -- 1 for a flag, 2 for one with a value -- and 1 when the caller has to handle it
# itself. `--help` sets `COMPOSITOR_HELP=1` and is reported as consumed, so a caller
# prints its own usage and stops.
#
# This is a case rather than a loop of its own so that a tier script keeps a single
# argument loop: an option that belongs to the tier and one that belongs to every tier
# are then parsed in one place, and a tier cannot end up with an option that only works
# when it is the first argument.
compositor_common_option() {
    case "$1" in
        --scope)
            [ $# -ge 2 ] || compositor_usage_error "--scope needs a value"
            case "$2" in
                environment | window | all) COMPOSITOR_SCOPE="$2" ;;
                *) compositor_usage_error "--scope must be environment, window or all" ;;
            esac
            COMPOSITOR_CONSUMED=2
            ;;
        --skip-build)
            COMPOSITOR_SKIP_BUILD=1
            COMPOSITOR_CONSUMED=1
            ;;
        --keep-scratch)
            COMPOSITOR_KEEP_SCRATCH=1
            COMPOSITOR_CONSUMED=1
            ;;
        --attach)
            [ $# -ge 2 ] || compositor_usage_error "--attach needs a display name"
            COMPOSITOR_ATTACH="$2"
            COMPOSITOR_CONSUMED=2
            ;;
        -h | --help)
            COMPOSITOR_HELP=1
            COMPOSITOR_CONSUMED=1
            ;;
        *)
            return 1
            ;;
    esac
    return 0
}

# The state `compositor_common_option` writes, reset for a fresh parse.
compositor_reset_options() {
    COMPOSITOR_SKIP_BUILD=0
    COMPOSITOR_KEEP_SCRATCH=0
    COMPOSITOR_SCOPE="environment"
    COMPOSITOR_ATTACH=""
    COMPOSITOR_HELP=0
    COMPOSITOR_CONSUMED=1
}

# Whether the window half is part of the requested scope.
compositor_wants_window() {
    [ "${COMPOSITOR_SCOPE:-}" = "window" ] || [ "${COMPOSITOR_SCOPE:-}" = "all" ]
}

# The options every tier script shares, for its own `usage` to print.
compositor_print_common_usage() {
    cat <<'USAGE'

  --scope WHICH     what to assert: `environment` (default), `window` or `all`.
                    `environment` is the session itself -- the compositor, the
                    protocols that tier's code path needs, and the two addons
                    loading. `window` additionally asserts the candidate window and
                    that it never takes the keyboard. The exit status describes
                    exactly the scope asked for and never a scope that did not run.
  --attach NAME     use a display server that is already running (`:0`, `wayland-0`,
                    ...) instead of starting one. Nothing is started and nothing is
                    stopped, and the session is a real one rather than a nested or
                    headless stand-in.
  --skip-build      use whatever is in `target/release/` instead of building.
  --keep-scratch    leave the staging directory and the session log behind.
  -h, --help        print this message
USAGE
}

# The one thing the window half needs and cannot get: a window.
#
# Called by a tier whose session cannot present one. The reason is read from the
# plugin's own diagnostics rather than assumed, so the message says which initialisation
# step is outstanding instead of guessing.
compositor_fail_without_window() {
    local why
    if compositor_window_backend_pending; then
        why="the plugin reports its window backend is not wired up yet"
        grep -E "$COMPOSITOR_PENDING_PLATFORM|$COMPOSITOR_PENDING_UI" \
            "$COMPOSITOR_SESSION_LOG" >&2 || true
    else
        why="the session reported no candidate window and no reason for its absence"
    fi
    compositor_log_tail "$COMPOSITOR_SESSION_LOG"
    compositor_die "$COMPOSITOR_NO_WINDOW" \
        "$COMPOSITOR_NO_WINDOW_CODE: the window scope was requested but there is no window to check: $why"
}

# The failure codes the tier ladder records, in the contract's own vocabulary.
#
# Each names a way a compositor can refuse a tier: no `zwlr_layer_shell_v1` at all, no
# configure inside the timeout, the compositor dismissing the popup, closing the layer
# surface, a protocol error, or -- the one that matters most -- handing our surface the
# keyboard. See `crates/ime-ui/src/platform/wayland/probe.rs`.
readonly WAYLAND_TIER_FAILURE_CODES=(
    "platform/wayland/protocol-missing"
    "platform/wayland/configure-timeout"
    "platform/wayland/popup-done"
    "platform/wayland/layer-closed"
    "platform/wayland/protocol-error"
    "platform/wayland/focus-taken"
)

# The window half for a Wayland tier.
#
# Wayland has no request that enumerates another client's surfaces, so there is no
# `xdotool search` equivalent here: a window a client created is invisible to everything
# outside that client and the compositor. What is observable is the plugin's own
# diagnostics, and that is what this asserts. The tier ladder records one of
# [`WAYLAND_TIER_FAILURE_CODES`] when a compositor refuses a tier or hands the surface
# the keyboard, so any of those codes in the session log means the window path did not
# work even though the session itself stayed up.
#
# What this half therefore cannot say is where the window ended up. Placement accuracy --
# the offset the geometry work is accepted against -- needs a measurement from inside
# the session, and nothing in this directory claims it.
compositor_wayland_window_scope() {
    local code
    if compositor_window_backend_pending; then
        compositor_fail_without_window
    fi
    for code in "${WAYLAND_TIER_FAILURE_CODES[@]}"; do
        compositor_assert_absent_from_log "$code" \
            "the tier ladder recorded $code: the compositor refused the tier or took the keyboard"
    done
    compositor_assert_absent_from_log "$COMPOSITOR_NO_WINDOW_CODE" \
        "the plugin reports that no backend on this compositor can host the window"
    compositor_verified "the plugin's own diagnostics record no tier failure and no fallback on this compositor"
    compositor_unverified "the window's geometry and its distance from the cursor: Wayland exposes no way to enumerate another client's surfaces, so placement is not observable from outside the session"
    COMPOSITOR_WINDOW="verified"
}
