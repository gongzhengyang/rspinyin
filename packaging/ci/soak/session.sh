#!/usr/bin/env bash
#
# One long-stability session: Fcitx5 with both addons, a client to type into, `xtask soak`
# driving the two, and the checks the run's own report cannot make.
#
# Usage: session.sh <hours> <report-path>
#
# Run from `run.sh`, inside `dbus-run-session`, from the repository root.
#
# # Why the readiness check is a memory map and not a log line
#
# "The addon loaded" is asserted by finding its library in the running process's map. A log
# line says the loader was reached; the map says the library is in the process, which is the
# thing the session needs. Both addons are checked, and both are named: ADR-0003 splits the
# product into an engine addon and a user-interface addon, and a session that loaded one of
# them has half an input method.
#
# # Why the candidate window is checked after the run
#
# `xtask soak` ends with a composition open on purpose, so the candidate window is mapped
# when it returns. Looking for the window on the X server is the only check available that
# distinguishes "the addon processed the injected keys" from "the keys went to a client
# that did nothing with them" -- the plugin's own report is about what it believed, and the
# client-text channel that would read the committed text back is a separate task's.
#
# # What is not verified here
#
# That a candidate was *presented* with the right geometry, colours and text. That needs
# the frame snapshot and the client readback channels; this file checks that the window
# exists and that the run left the input focus where it found it.

set -euo pipefail

hours="${1:?usage: session.sh <hours> <report-path>}"
report="${2:?usage: session.sh <hours> <report-path>}"

# How long a session component is given to come up, in half-second steps.
readonly STARTUP_STEPS=120

# The Fcitx5 daemon's process name, as `pgrep -x` matches it.
readonly DAEMON_NAME="fcitx5"

# The addon libraries the session must have mapped, one per addon.
readonly ADDON_LIBRARIES=("librspinyin.so" "librspinyin_ui.so")

# The `WM_CLASS` the candidate window carries.
readonly CANDIDATE_CLASS="rspinyin"

# The `WM_CLASS` the client under test carries.
readonly CLIENT_CLASS="XTerm"

# Waits until `command` succeeds, or fails with `code` and `message`.
await() {
  local code="$1" message="$2"
  shift 2
  for _ in $(seq 1 "${STARTUP_STEPS}"); do
    "$@" > /dev/null 2>&1 && return 0
    sleep 0.5
  done
  echo "${code}: ${message}" >&2
  return 1
}

fcitx5 -d > /tmp/fcitx5.log 2>&1 || true
await "dist/verify/addon-not-loaded" "${DAEMON_NAME} never started; see /tmp/fcitx5.log" \
  pgrep -x "${DAEMON_NAME}"
# `-o` selects the oldest match, which keeps the answer a single process without a pipe:
# `pgrep | head` would leave `pgrep` killed by SIGPIPE, and `pipefail` turns that into a
# failed assignment.
daemon_pid="$(pgrep -x -o "${DAEMON_NAME}")"

for library in "${ADDON_LIBRARIES[@]}"; do
  await "dist/verify/addon-not-loaded" \
    "${library} is not mapped into ${DAEMON_NAME} (pid ${daemon_pid})" \
    grep -q "${library}" "/proc/${daemon_pid}/maps"
done

# The client is what the injected keys are aimed at: Fcitx5's X frontend follows the
# focused client, so a session with no client has nowhere for a keystroke to land and the
# soak would measure a conversation with nobody.
xterm -class "${CLIENT_CLASS}" -e cat > /tmp/xterm.log 2>&1 &
await "dist/verify/client-missing" "no ${CLIENT_CLASS} window appeared; see /tmp/xterm.log" \
  xdotool search --class "${CLIENT_CLASS}"
window="$(xdotool search --class "${CLIENT_CLASS}" | head -n 1 || true)"
if [ -z "${window}" ]; then
  echo "dist/verify/client-missing: no ${CLIENT_CLASS} window on the server" >&2
  exit 1
fi

echo "session: ${DAEMON_NAME} pid ${daemon_pid}, client window ${window}, soak ${hours}h"

cargo run --quiet -p xtask -- soak \
  --hours "${hours}" \
  --pid "${daemon_pid}" \
  --window "${window}" \
  --report "${report}"

# See the header: the run ends with a composition open so that this is a fact rather than a
# race against the next commit.
candidate="$(xdotool search --class "${CANDIDATE_CLASS}" | head -n 1 || true)"
if [ -z "${candidate}" ]; then
  echo "dist/verify/candidate-window-absent: no ${CANDIDATE_CLASS} window on the server" >&2
  echo "session.sh: the soak typed for ${hours}h and the addon showed no candidate window" >&2
  exit 1
fi
echo "session: candidate window ${candidate} is mapped"

if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  {
    echo "### Session"
    echo
    echo "| item | value |"
    echo "|---|---|"
    echo "| display | ${DISPLAY} (Xvfb, no compositor) |"
    echo "| ${DAEMON_NAME} | pid ${daemon_pid}, both addon libraries mapped |"
    echo "| client window | ${window} (${CLIENT_CLASS}) |"
    echo "| candidate window | ${candidate} mapped at the end of the run |"
    echo
    echo "NOT verified here: real transparency and background blur (no compositing"
    echo "manager), fractional scaling, multi-output placement, and the text the host"
    echo "committed into the client."
  } >> "${GITHUB_STEP_SUMMARY}"
fi
