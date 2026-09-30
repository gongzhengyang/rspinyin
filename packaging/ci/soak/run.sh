#!/usr/bin/env bash
#
# Prepare a session for the long-stability soak and hand it to `session.sh`.
#
# Usage: run.sh <hours> <report-path>
#
# The two files are split because they live in different worlds. This one sets up the
# environment a session needs -- a virtual X server, a private configuration directory, a
# runtime directory, and the locale the X input-method protocol is only attempted under --
# and it needs no Fcitx5 running. `session.sh` runs inside `dbus-run-session` and owns the
# daemon, the client, the soak itself and the assertions made against what the run left
# behind.
#
# Why a virtual X server rather than a real session: a runner has no display, and the
# candidate window's X11 path is the one this project can exercise without a compositor.
# `+extension Composite` is requested because the window takes the opaque-background
# fallback when no compositing manager owns the selection, which is the path a headless
# server exercises.
#
# What this file cannot verify, and does not claim to: real transparency, background blur,
# fractional scaling and multi-output placement. Those need a compositor and a GPU.

set -euo pipefail

hours="${1:?usage: run.sh <hours> <report-path>}"
report="${2:?usage: run.sh <hours> <report-path>}"

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "${here}/../../.." && pwd)"

display=":99"
export DISPLAY="${display}"
# The X input-method protocol is only attempted when the locale is UTF-8, and Fcitx5 is
# only reached by a client that asks for `@im=fcitx`; without both, a client would receive
# the raw keystrokes and the soak would measure a session with no input method in it.
export LANG="C.UTF-8"
export LC_ALL="C.UTF-8"
export XMODIFIERS="@im=fcitx"
export GTK_IM_MODULE="fcitx"
export QT_IM_MODULE="fcitx"

runtime="$(mktemp -d)"
export XDG_RUNTIME_DIR="${runtime}"
chmod 700 "${runtime}"

# A private configuration directory, so the session reads the profile written below and
# not whatever the runner's home directory happens to contain. The profile lists one input
# method, which is what makes the session's active method the one under test: with the
# stock profile the keystrokes would go to `keyboard-us` and the soak would exercise a
# plugin that never saw them.
config="$(mktemp -d)"
export XDG_CONFIG_HOME="${config}"
mkdir -p "${config}/fcitx5"
cat > "${config}/fcitx5/profile" <<'PROFILE'
[Groups/0]
Name=Default
Default Layout=us
DefaultIM=rspinyin

[Groups/0/Items/0]
Name=rspinyin
Layout=

[GroupOrder]
0=Default
PROFILE

Xvfb "${display}" -screen 0 1920x1080x24 +extension Composite > /tmp/xvfb.log 2>&1 &
xvfb_pid=$!
# The server is asked whether it is there rather than slept for: a fixed wait is either
# too short on a loaded runner or wasted on an idle one.
for _ in $(seq 1 60); do
  xdpyinfo -display "${display}" > /dev/null 2>&1 && break
  sleep 0.5
done
if ! xdpyinfo -display "${display}" > /dev/null 2>&1; then
  echo "dist/verify/compositor-unavailable: Xvfb never answered on ${display}" >&2
  echo "run.sh: see /tmp/xvfb.log" >&2
  exit 1
fi

cd "${root}"
# The session's own status is carried out rather than checked here: this file's job is the
# environment, and a run that failed inside the session has already said why.
set +e
dbus-run-session -- bash "${here}/session.sh" "${hours}" "${report}"
status=$?
set -e

kill "${xvfb_pid}" 2> /dev/null || true
exit "${status}"
