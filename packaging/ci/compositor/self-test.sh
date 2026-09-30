#!/usr/bin/env bash
#
# self-test.sh - prove the compositor checks' parsers and verdicts are sensitive.
#
# The tier scripts cannot run without a compositor, but most of what they do is read text:
# Fcitx5's addon lines, the plugin's own diagnostics, and the globals a compositor
# publishes. A parser that silently matched nothing would let a tier report coverage it
# does not have, and that is the one failure this whole directory exists to prevent -- so
# the parsers are exercised here, against synthetic input, with no compositor, no fcitx5
# and no display server.
#
# Each case is a pair wherever a pair is possible: the input that must be accepted and the
# near-miss that must be rejected. `rspinyin` and `rspinyin-ui` are the reason -- one is a
# prefix of the other, and a check that confused them would call the engine loaded on a
# session where only the UI addon arrived.
#
# Run: bash packaging/ci/compositor/self-test.sh
#
# Exit status: 0 when every case passed, 1 when one did not.

set -euo pipefail

readonly SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=packaging/ci/compositor/common.sh
source "$SCRIPT_DIR/common.sh"

COVERAGE_TIER="self-test"
COMPOSITOR_SCRATCH=""
COMPOSITOR_SESSION_LOG=""
COMPOSITOR_COMPOSITOR_LOG=""

passed=0
failed=0
scratch="$(mktemp -d "${TMPDIR:-/tmp}/rspinyin-compositor-self-test-XXXXXX")"
trap 'rm -rf -- "$scratch"' EXIT

# Reports one case.
check() {
    local what="$1" expected="$2" actual="$3"
    if [ "$expected" = "$actual" ]; then
        passed=$((passed + 1))
        printf 'ok   %s\n' "$what"
    else
        failed=$((failed + 1))
        printf 'FAIL %s: expected %s, got %s\n' "$what" "$expected" "$actual" >&2
    fi
}

# Runs `command` in a subshell and reports the exit status it produced.
status_of() {
    local status=0
    ( "$@" ) >/dev/null 2>&1 || status=$?
    printf '%s' "$status"
}

# Whether a session log holding `$1` reports `$2` as loaded.
log_has_addon() {
    COMPOSITOR_SESSION_LOG="$scratch/session.log"
    printf '%s\n' "$1" >"$COMPOSITOR_SESSION_LOG"
    local status=0
    compositor_log_has_addon "$2" || status=$?
    printf '%s' "$status"
}

# ---------------------------------------------------------------------------
# The addon lines
# ---------------------------------------------------------------------------
#
# Fcitx5 writes `Loaded addon <id>` once the shared object's factory has been called. The
# engine's id is a prefix of the UI addon's, so the boundary is the whole test.

check "the engine's line is recognised" \
    0 "$(log_has_addon 'I addonmanager.cpp:195] Loaded addon rspinyin' rspinyin)"
check "the UI addon's line is not mistaken for the engine's" \
    1 "$(log_has_addon 'I addonmanager.cpp:195] Loaded addon rspinyin-ui' rspinyin)"
check "the UI addon's line is recognised" \
    0 "$(log_has_addon 'I addonmanager.cpp:195] Loaded addon rspinyin-ui' rspinyin-ui)"
check "a session with neither addon is reported as neither" \
    1 "$(log_has_addon 'I addonmanager.cpp:195] Loaded addon keyboard' rspinyin)"
check "the failure line is not read as a success" \
    1 "$(log_has_addon 'W addonloader.cpp:40] Could not load addon rspinyin' rspinyin)"

# ---------------------------------------------------------------------------
# The plugin's own diagnostics
# ---------------------------------------------------------------------------

pending_log() {
    COMPOSITOR_SESSION_LOG="$scratch/session.log"
    printf '%s\n' "$1" >"$COMPOSITOR_SESSION_LOG"
    local status=1
    compositor_window_backend_pending && status=0
    printf '%s' "$status"
}

check "the platform probe being pending is detected" \
    0 "$(pending_log 'rspinyin-ui: lifecycle/pending: platform awaits the X11 / Wayland backend probe')"
check "the UI start-up being pending is detected" \
    0 "$(pending_log 'rspinyin-ui: lifecycle/pending: ui-startup awaits the UI thread and the pre-created window')"
check "a session with no pending step is not reported as pending" \
    1 "$(pending_log 'rspinyin-ui: addon loaded')"

# ---------------------------------------------------------------------------
# The compositor's globals
# ---------------------------------------------------------------------------

# A stand-in for `wayland-info`, so the parser is exercised against a listing that is
# written here rather than against whatever the machine's compositor happens to publish.
mkdir -p -- "$scratch/bin"
cat >"$scratch/bin/wayland-info" <<'STUB'
#!/usr/bin/env bash
cat <<'LISTING'
interface: 'wl_display',                         version:  1, name:  1
interface: 'wl_shm',                             version:  1, name:  3
interface: 'xdg_wm_base',                        version:  6, name: 22
interface: 'zwlr_layer_shell_v1',                version:  4, name: 24
LISTING
STUB
chmod +x "$scratch/bin/wayland-info"
PATH="$scratch/bin:$PATH"

check "a published interface's version is read" \
    "4" "$(compositor_wayland_global_version zwlr_layer_shell_v1)"
check "an unpublished interface reads as nothing" \
    "" "$(compositor_wayland_global_version zwp_input_method_v2)"
check "a published interface is accepted" \
    "0" "$(status_of compositor_require_wayland_global xdg_wm_base why)"
check "an unpublished interface is refused" \
    "1" "$(status_of compositor_require_wayland_global zwp_input_method_v2 why)"
check "the rung above the popup tiers is refused when it is present" \
    "1" "$(status_of compositor_require_wayland_global_absent zwlr_layer_shell_v1 why)"
check "the rung above the popup tiers is accepted when it is absent" \
    "0" "$(status_of compositor_require_wayland_global_absent zwp_input_method_v2 why)"

# ---------------------------------------------------------------------------
# The session log assertions
# ---------------------------------------------------------------------------

absent_from_log() {
    COMPOSITOR_SESSION_LOG="$scratch/session.log"
    printf '%s\n' "$1" >"$COMPOSITOR_SESSION_LOG"
    local status=0
    compositor_assert_absent_from_log "$2" why || status=$?
    printf '%s' "$status"
}

check "a clean session passes the absence assertion" \
    0 "$(absent_from_log 'rspinyin: addon loaded' 'platform/wayland/focus-taken')"
check "a session that took the keyboard fails it" \
    1 "$(absent_from_log 'rspinyin-ui: platform/wayland/focus-taken' 'platform/wayland/focus-taken')"

# ---------------------------------------------------------------------------
# Process liveness
# ---------------------------------------------------------------------------
#
# `kill -0` alone would call an exited-but-unreaped child alive, and a wait loop built on
# it would sit out its whole timeout against a session that died immediately.

sleep 5 &
live_pid=$!
check "a running process is alive" "0" "$(status_of compositor_process_alive "$live_pid")"
kill "$live_pid" 2>/dev/null || true
wait "$live_pid" 2>/dev/null || true
check "a reaped process is not alive" "1" "$(status_of compositor_process_alive "$live_pid")"
check "an empty pid is not alive" "1" "$(status_of compositor_process_alive '')"

# ---------------------------------------------------------------------------
# Argument parsing
# ---------------------------------------------------------------------------

compositor_reset_options
compositor_common_option --scope window
check "the scope option is consumed with its value" "2" "$COMPOSITOR_CONSUMED"
check "the scope option sets the scope" "window" "$COMPOSITOR_SCOPE"
compositor_common_option --skip-build
check "a flag is consumed on its own" "1" "$COMPOSITOR_CONSUMED"
check "a flag sets its flag" "1" "$COMPOSITOR_SKIP_BUILD"
check "an unknown option is left to the caller" "1" "$(status_of compositor_common_option --display :0)"
check "an invalid scope is refused" "2" "$(status_of compositor_common_option --scope sideways)"

for scope in environment window all; do
    COMPOSITOR_SCOPE="$scope"
    case "$scope" in
        environment) expected="1" ;;
        *) expected="0" ;;
    esac
    check "the window half is $([ "$expected" = 0 ] && echo requested || echo skipped) under --scope $scope" \
        "$expected" "$(status_of compositor_wants_window)"
done

# ---------------------------------------------------------------------------
# The coverage block
# ---------------------------------------------------------------------------
#
# The block is the evidence a reader is given, so its shape is asserted rather than
# assumed: the machine-readable line has to name every field the workflow's `coverage`
# job matches on.

coverage_run() {
    local scope="$1" exit_status="$2"
    COMPOSITOR_SCRATCH="$scratch/coverage"
    mkdir -p -- "$COMPOSITOR_SCRATCH"
    COMPOSITOR_SESSION_LOG="$scratch/session.log"
    : >"$COMPOSITOR_SESSION_LOG"
    COVERAGE_TIER="x11"
    COVERAGE_SESSION="self-test"
    COVERAGE_SCOPE="$scope"
    COVERAGE_ENVIRONMENT="verified"
    COVERAGE_WINDOW="skipped"
    COVERAGE_VERIFIED=("something that held")
    COVERAGE_UNVERIFIED=("something that did not run")
    local status=0
    # The step summary is the job's, not this script's: a self-test that appended its
    # synthetic blocks to it would put made-up coverage in front of a reader.
    ( unset GITHUB_STEP_SUMMARY; compositor_finish "$exit_status" ) >/dev/null 2>&1 || status=$?
    printf '%s' "$status"
}

check "the coverage block exits with the status it was given" \
    "0" "$(coverage_run environment 0)"
check "a failing run still writes its block" \
    "1" "$(coverage_run environment 1)"
check "the block names every field the workflow matches on" \
    "1" "$(grep -c '^coverage tier=x11 session=self-test scope=environment environment=verified window=skipped exit=1$' "$scratch/coverage/coverage.md")"
check "the block lists what was verified" \
    "1" "$(grep -c '^- something that held$' "$scratch/coverage/coverage.md")"
check "the block lists what was not verified" \
    "1" "$(grep -c '^- something that did not run$' "$scratch/coverage/coverage.md")"

# ---------------------------------------------------------------------------
# The verdict
# ---------------------------------------------------------------------------

printf '\n%s passed, %s failed\n' "$passed" "$failed"
if [ "$failed" -ne 0 ]; then
    exit 1
fi
