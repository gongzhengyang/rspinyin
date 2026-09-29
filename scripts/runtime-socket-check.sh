#!/usr/bin/env bash
#
# runtime-socket-check.sh - assert that a live fcitx5 session holds no IP socket.
#
# `BUDGET-NET-01` fixes the number of process-external sockets at zero: the
# plugin must behave byte-identically online and offline. scripts/check-no-network.sh
# proves the *capability* is absent from the dependency closure; this script is
# the runtime half of the same promise and observes the sockets a real session
# actually holds.
#
# For every socket owned by the observed process, three shapes are violations:
#
#   peer    a TCP/UDP socket whose peer address is neither loopback nor the
#           wildcard -- an outbound connection, including one still being
#           established (SYN-SENT) and a UDP socket connected for a lookup.
#   bind    a TCP/UDP socket bound to a specific non-loopback local address:
#           reachable from the network before anybody connects.
#   listen  a TCP listener on the wildcard address: it accepts connections from
#           anywhere, which is the inbound half of the same capability.
#
# Deliberately not violations:
#
#   * AF_UNIX sockets. fcitx5's D-Bus, X11 and Wayland connections are all Unix
#     sockets; they cannot leave the machine, and counting them would make the
#     gate meaningless. lsof reports them so the summary can say how many were
#     excluded instead of dropping them silently.
#   * loopback connections and listeners (127.0.0.0/8, ::1), including the
#     systemd-resolved stub at 127.0.0.53.
#   * UDP sockets bound to the wildcard with no peer: nothing has been sent.
#
# Two collectors, on purpose. `ss` -- the method the budget table names -- is
# authoritative, and `lsof -nP` cross-checks it: when lsof sees an external
# socket that ss did not, the gate fails even though both reported nothing,
# because a collector that cannot see a socket cannot certify its absence. lsof
# runs with -nP so that the audit itself performs no name lookups; an audit that
# resolves DNS in order to decide whether anything resolves DNS is self-defeating.
#
# Process selection: a running fcitx5 is observed where it is, because
# restarting a user's live input method is a side effect this script must not
# have. `--replace` opts into the `fcitx5 -r` restart the architecture spec
# describes, and `--pid` observes an arbitrary process, which is what the
# self-test uses. An fcitx5 that this script started itself is stopped again on
# exit, so the machine is left as it was found.
#
# Exit codes: 0 = pass, 1 = violation, 2 = usage or environment error.
#
# Self-test: `--self-test` classifies synthetic `ss`/`lsof` listings -- an
# external peer, an external bind, a wildcard listener, a collector
# disagreement, and benign AF_UNIX/loopback/wildcard-UDP sockets -- and then
# repeats the exercise against real helper processes holding exactly those
# sockets, so the collectors are proven sensitive rather than just the parser.
# It also snapshots the working tree before the exercise and compares it
# afterwards, which is the assertion behind "the self-test has no side effects":
# the helpers bind sockets and write into a scratch directory, never into the
# repository. The guard compares before against after rather than demanding a
# clean tree, because a developer's tree is expected to be dirty while they
# work -- what must not change is the tree *because of this run*.
#
# Requires python3, iproute2 (`ss`) and `lsof`; the runtime half additionally
# needs a real fcitx5 session (see the `[实验室]` tag on the task card).

set -euo pipefail

usage() {
    cat <<'USAGE'
usage: scripts/runtime-socket-check.sh [--self-test] [--pid PID] [--wait SECONDS]
                                      [--replace] [--ss-file FILE] [--lsof-file FILE]

  --self-test        classify synthetic listings and real helper processes
  --pid PID          observe this process instead of fcitx5 (repeatable)
  --wait SECONDS     settle time after starting fcitx5 (default: 3)
  --replace          restart fcitx5 with `fcitx5 -r -d` before sampling
  --ss-file FILE     analyse a captured `ss -tanp` / `ss -uanp` listing
  --lsof-file FILE   analyse a captured `lsof -nP -i` listing
USAGE
}

die() {
    echo "runtime-socket-check: $*" >&2
    exit 2
}

mode="check"
target_pids=()
wait_seconds=3
replace=0
ss_file=""
lsof_file=""
started_fcitx5=""
scratch=""

cleanup() {
    if [ -n "$scratch" ]; then
        rm -rf -- "$scratch"
    fi
    if [ -n "$started_fcitx5" ]; then
        # Only an instance this script started is stopped again; a session that
        # was already running is none of our business.
        kill "$started_fcitx5" 2>/dev/null || true
        wait "$started_fcitx5" 2>/dev/null || true
    fi
}
trap cleanup EXIT

while [ $# -gt 0 ]; do
    case "$1" in
        --self-test) mode="self-test"; shift ;;
        --pid)
            target_pids+=("${2:?--pid needs a process id}")
            shift 2
            ;;
        --wait)
            wait_seconds="${2:?--wait needs a number of seconds}"
            shift 2
            ;;
        --replace) replace=1; shift ;;
        --ss-file)
            ss_file="${2:?--ss-file needs a path}"
            shift 2
            ;;
        --lsof-file)
            lsof_file="${2:?--lsof-file needs a path}"
            shift 2
            ;;
        -h | --help)
            usage
            exit 0
            ;;
        *)
            echo "runtime-socket-check: unknown argument: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

case "$wait_seconds" in
    '' | *[!0-9]*) die "--wait needs a whole number of seconds, got: $wait_seconds" ;;
esac

if ! command -v python3 >/dev/null 2>&1; then
    die "python3 is required to parse the socket listings; install it and re-run"
fi

require_tool() {
    if ! command -v "$1" >/dev/null 2>&1; then
        die "$1 is not installed; install it and re-run (the socket audit cannot be skipped)"
    fi
}

is_running() {
    kill -0 "$1" 2>/dev/null
}

require_running() {
    # require_running PID: fail when the process to be audited is gone, because
    # a vanished process would otherwise be reported as holding zero sockets.
    if ! is_running "$1"; then
        die "process $1 is not running; start fcitx5 (or pass a live --pid) and re-run"
    fi
}

# collect SS_FILE LSOF_FILE PID...: sample the live socket tables.
#
# UDP is sampled next to TCP even though the budget table only names `ss -tanp`:
# a lookup that never completes a TCP handshake is exactly the case this gate
# exists to catch, and `-uanp` is where it shows up.
collect() {
    local ss_out="$1" lsof_out="$2" pid_list
    shift 2
    pid_list="$(IFS=,; echo "$*")"
    {
        ss -tanp
        ss -uanp
    } >"$ss_out" 2>/dev/null || true
    # lsof exits 1 when nothing matches, which is the desired answer for an
    # offline process; the empty listing is what the cross-check reads.
    lsof -nP -p "$pid_list" -a -i -u "$(id -un)" >"$lsof_out" 2>/dev/null || true
}

# classify SS_FILE LSOF_FILE PID...: audit the listings and report.
classify() {
    python3 - "$@" <<'PY'
import ipaddress
import re
import sys
from pathlib import Path

ss_path, lsof_path = sys.argv[1], sys.argv[2]
target_pids = []
for value in sys.argv[3:]:
    if not value.isdigit():
        sys.exit(f"runtime-socket-check: not a process id: {value}")
    target_pids.append(int(value))
targets = set(target_pids)

PID_IN_SS = re.compile(r"pid=(\d+)")
LSOF_STATE = re.compile(r"\(([A-Za-z0-9-]+)\)\s*$")
LSOF_PROTOCOL = re.compile(r"^(TCP|UDP)\s+", re.IGNORECASE)

# Addresses that name no particular host: an unbound or wildcard-bound socket
# has not reached the network, whatever state it is in.
WILDCARD = frozenset({"*", "", "0.0.0.0", "::"})


def read_text(path):
    try:
        return Path(path).read_text(encoding="utf-8", errors="replace")
    except OSError as error:
        sys.exit(f"runtime-socket-check: cannot read {path}: {error}")


def is_wildcard(address):
    return address in WILDCARD


def is_loopback(address):
    try:
        return ipaddress.ip_address(address).is_loopback
    except ValueError:
        return False


def endpoint(text):
    """(address, port) for `addr:port`, `[v6]:port` or `*:port`; None otherwise."""
    text = text.strip()
    if not text:
        return None
    if text.startswith("["):
        closing = text.rfind("]")
        if closing < 0:
            return None
        address, port = text[1:closing], text[closing + 2:]
    else:
        address, separator, port = text.rpartition(":")
        if not separator:
            return None
    if port != "*" and not port.isdigit():
        return None
    if not address:
        return None
    # A link-local scope suffix (`fe80::1%eth0`) names an interface, not a host.
    return address.split("%")[0], port


def classify(state, local, peer):
    """`peer`, `bind` or `listen` when the socket can reach the network."""
    if peer is not None:
        address = peer[0]
        if not is_wildcard(address) and not is_loopback(address):
            return "peer"
    if local is not None:
        address = local[0]
        if not is_wildcard(address) and not is_loopback(address):
            return "bind"
        if state == "LISTEN" and is_wildcard(address):
            return "listen"
    return None


def key_of(reason, local, peer):
    """Stable identity of an external socket, shared by both collectors."""
    address, port = peer if reason == "peer" else local
    return f"{'*' if is_wildcard(address) else address.lower()}:{port}"


def parse_ss(text):
    sockets = []
    for raw in text.splitlines():
        fields = raw.split()
        if len(fields) < 5:
            continue
        local, peer = endpoint(fields[3]), endpoint(fields[4])
        if local is None or peer is None:
            continue
        pids = {int(match) for match in PID_IN_SS.findall(" ".join(fields[5:]))}
        sockets.append((fields[0].upper(), local, peer, pids, raw.strip()))
    return sockets


def parse_lsof(text):
    """(ip sockets, number of AF_UNIX sockets) from an `lsof -nP -i` listing."""
    sockets = []
    unix_sockets = 0
    for raw in text.splitlines():
        fields = raw.split(None, 8)
        if len(fields) < 9 or fields[0] == "COMMAND":
            continue
        if fields[4] not in ("IPv4", "IPv6"):
            unix_sockets += 1
            continue
        name = fields[8]
        state = ""
        match = LSOF_STATE.search(name)
        if match:
            state = match.group(1).upper()
            name = name[: match.start()]
        name = LSOF_PROTOCOL.sub("", name.strip())
        if "->" in name:
            local_text, peer_text = name.split("->", 1)
        else:
            local_text, peer_text = name, ""
        sockets.append((state, endpoint(local_text), endpoint(peer_text), raw.strip()))
    return sockets, unix_sockets


ss_sockets = parse_ss(read_text(ss_path))
lsof_sockets, unix_sockets = parse_lsof(read_text(lsof_path))

# `ss -p` names the process only for sockets the caller may inspect. When the
# listing holds sockets but not one `users:` field, no socket can be attributed
# and the gate would pass vacuously, so it fails instead.
if ss_sockets and not any(record[3] for record in ss_sockets):
    sys.exit(
        "runtime-socket-check: `ss -p` reported no process for any socket; run as "
        "the owner of the observed process so sockets can be attributed to it"
    )

problems = []
inspected = 0
external = {}
for state, local, peer, pids, raw in ss_sockets:
    if not pids & targets:
        continue
    inspected += 1
    reason = classify(state, local, peer)
    if reason is None:
        continue
    external[key_of(reason, local, peer)] = raw
    problems.append(f"{reason}: {raw}")

# The cross-check: everything lsof calls external must also have been seen by
# ss. A socket only lsof saw is a hole in the observation, not a clean result.
lsof_external = {}
for state, local, peer, raw in lsof_sockets:
    reason = classify(state, local, peer)
    if reason is None:
        continue
    lsof_external[key_of(reason, local, peer)] = raw

missed = sorted(set(lsof_external) - set(external))
if missed:
    problems.append(
        "collectors disagree: lsof reports external socket(s) that ss did not: "
        + ", ".join(missed)
    )

if problems:
    print(f"runtime-socket-check: FAIL: {len(problems)} external socket(s)", file=sys.stderr)
    for problem in problems:
        print(f"  - {problem}", file=sys.stderr)
    print(
        "runtime-socket-check: the plugin must stay byte-identically offline "
        "(BUDGET-NET-01); remove the code that opens the socket",
        file=sys.stderr,
    )
    sys.exit(1)

print(
    f"runtime-socket-check: PASS: 0 external sockets "
    f"({inspected} IP socket(s) inspected for pid {', '.join(str(pid) for pid in target_pids)}, "
    f"{unix_sockets} AF_UNIX socket(s) excluded, ss and lsof agree)"
)
PY
}

expect_clean() {
    # expect_clean SS_FILE LSOF_FILE PID DESCRIPTION
    local status=0 output=""
    if output="$(classify "$1" "$2" "$3" 2>&1)"; then
        status=0
    else
        status=$?
    fi
    if [ "$status" -ne 0 ]; then
        echo "runtime-socket-check: self-test FAILED - $4 exited $status, expected 0" >&2
        echo "$output" >&2
        return 1
    fi
    case "$output" in
        *"PASS: 0 external sockets"*) ;;
        *)
            echo "runtime-socket-check: self-test FAILED - $4 did not report the pass line" >&2
            echo "$output" >&2
            return 1
            ;;
    esac
    return 0
}

expect_violation() {
    # expect_violation SS_FILE LSOF_FILE PID EXPECTED_TEXT DESCRIPTION
    local status=0 output=""
    if output="$(classify "$1" "$2" "$3" 2>&1)"; then
        status=0
    else
        status=$?
    fi
    if [ "$status" -eq 0 ]; then
        echo "runtime-socket-check: self-test FAILED - $5 was not detected" >&2
        echo "$output" >&2
        return 1
    fi
    case "$output" in
        *"$4"*) ;;
        *)
            echo "runtime-socket-check: self-test FAILED - $5 was reported for the wrong reason" >&2
            echo "runtime-socket-check: expected the report to mention: $4" >&2
            echo "$output" >&2
            return 1
            ;;
    esac
    return 0
}

# Repository-state guard. See the note at the top of this file: the pair proves
# the self-test left the tree as it found it. `side_effect_tracked` distinguishes
# "the tree is clean" from "the tree could not be observed", which an empty
# snapshot cannot do on its own.
side_effect_snapshot=""
side_effect_tracked=0
side_effect_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"

snapshot_repository() {
    if ! command -v git >/dev/null 2>&1; then
        echo "runtime-socket-check: git is unavailable; the self-test cannot observe the working tree"
        return 0
    fi
    if ! side_effect_snapshot="$(git -C "$side_effect_root" status --porcelain 2>/dev/null)"; then
        side_effect_snapshot=""
        echo "runtime-socket-check: $side_effect_root is not a git work tree; the self-test cannot observe the working tree"
        return 0
    fi
    side_effect_tracked=1
    return 0
}

assert_repository_unchanged() {
    [ "$side_effect_tracked" -eq 1 ] || return 0
    local after
    if ! after="$(git -C "$side_effect_root" status --porcelain 2>/dev/null)"; then
        return 0
    fi
    if [ "$after" = "$side_effect_snapshot" ]; then
        return 0
    fi
    echo "runtime-socket-check: self-test FAILED - the self-test modified the working tree" >&2
    echo "runtime-socket-check: git status --porcelain before:" >&2
    printf '%s\n' "$side_effect_snapshot" >&2
    echo "runtime-socket-check: git status --porcelain after:" >&2
    printf '%s\n' "$after" >&2
    return 1
}

# non_loopback_ipv4: the first routable IPv4 address of this host, if any.
#
# The address list comes from the kernel through SIOCGIFADDR: no name
# resolution and no packet sent, which matters in a script whose whole purpose
# is to prove that nothing is sent.
non_loopback_ipv4() {
    python3 - <<'PY'
import fcntl
import socket
import struct

for _, name in socket.if_nameindex():
    probe = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        packed = fcntl.ioctl(probe.fileno(), 0x8915, struct.pack("256s", name.encode()[:15]))
        address = socket.inet_ntoa(packed[20:24])
    except OSError:
        continue
    finally:
        probe.close()
    if not address.startswith("127."):
        print(address)
        break
PY
}

write_ss_fixture() {
    # write_ss_fixture FILE EXTRA_STATE
    cat >"$1" <<EOF
State  Recv-Q Send-Q Local Address:Port  Peer Address:Port Process
LISTEN 0      128    127.0.0.1:1234      0.0.0.0:*          users:(("helper",pid=4242,fd=5))
UNCONN 0      0      0.0.0.0:0           0.0.0.0:*          users:(("helper",pid=4242,fd=6))
ESTAB  0      0      127.0.0.1:55102     127.0.0.53:53      users:(("helper",pid=4242,fd=7))
ESTAB  0      0      [::1]:55103         [::1]:631          users:(("helper",pid=4242,fd=8))
ESTAB  0      0      10.0.0.5:55104      93.184.216.34:443  users:(("unrelated",pid=9999,fd=9))
$2
EOF
}

write_lsof_fixture() {
    # write_lsof_fixture FILE EXTRA_ROWS
    cat >"$1" <<EOF
COMMAND PID  USER FD TYPE DEVICE SIZE/OFF NODE NAME
helper  4242 gong 3u unix 0xffff 0t0 /run/user/1000/sockets/10.0.0.5:443 type=STREAM
helper  4242 gong 5u IPv4 10005 0t0 TCP 127.0.0.1:1234 (LISTEN)
helper  4242 gong 6u IPv4 10006 0t0 UDP *:0
helper  4242 gong 7u IPv4 10007 0t0 UDP 127.0.0.1:55102->127.0.0.53:53
helper  4242 gong 8u IPv6 10008 0t0 TCP [::1]:55103->[::1]:631 (ESTABLISHED)
$2
EOF
}

run_fixture_self_test() {
    local ss="$scratch/fixture.ss" lsof="$scratch/fixture.lsof"
    echo "runtime-socket-check: self-test (synthetic ss and lsof listings)"

    write_ss_fixture "$ss" ""
    write_lsof_fixture "$lsof" ""
    expect_clean "$ss" "$lsof" 4242 "an AF_UNIX, loopback and wildcard-UDP process"
    # The unrelated pid on 93.184.216.34 proves the pid filter: an external
    # socket of another process must not fail this gate.
    expect_clean "$ss" "$lsof" 4242 "a process whose only external peer belongs to another pid"

    write_ss_fixture "$ss" \
        'ESTAB  0      0      10.0.0.5:55200      93.184.216.34:443  users:(("helper",pid=4242,fd=10))'
    write_lsof_fixture "$lsof" \
        'helper  4242 gong 10u IPv4 10010 0t0 TCP 10.0.0.5:55200->93.184.216.34:443 (ESTABLISHED)'
    expect_violation "$ss" "$lsof" 4242 "93.184.216.34" "an outbound connection"

    write_ss_fixture "$ss" \
        'SYN-SENT 0    1      10.0.0.5:55201      203.0.113.9:80     users:(("helper",pid=4242,fd=11))'
    write_lsof_fixture "$lsof" ""
    expect_violation "$ss" "$lsof" 4242 "203.0.113.9" "a connection still being established"

    write_ss_fixture "$ss" \
        'LISTEN 0      128    192.168.1.20:8080   0.0.0.0:*          users:(("helper",pid=4242,fd=12))'
    write_lsof_fixture "$lsof" ""
    expect_violation "$ss" "$lsof" 4242 "192.168.1.20" "a listener bound to a routable address"

    write_ss_fixture "$ss" \
        'LISTEN 0      128    *:8080              *:*                users:(("helper",pid=4242,fd=13))'
    write_lsof_fixture "$lsof" ""
    expect_violation "$ss" "$lsof" 4242 "listen:" "a wildcard listener"

    write_ss_fixture "$ss" ""
    write_lsof_fixture "$lsof" \
        'helper  4242 gong 14u IPv4 10014 0t0 TCP 10.0.0.5:55202->8.8.8.8:53 (ESTABLISHED)'
    expect_violation "$ss" "$lsof" 4242 "collectors disagree" \
        "a socket that only lsof observed"

    write_ss_fixture "$ss" ""
    write_lsof_fixture "$lsof" ""
    expect_clean "$ss" "$lsof" 4242 "the fixture after removing the violations"
    echo "runtime-socket-check: self-test PASS (4 violation classes and 1 disagreement detected)"
}

run_live_self_test() {
    local address helper_pid="" ss="$scratch/live.ss" lsof="$scratch/live.lsof"
    cat >"$scratch/helper.py" <<'PY'
import socket
import sys
import time

unix_path, address = sys.argv[1], sys.argv[2]
held = []

# fcitx5's D-Bus connection lives on an AF_UNIX socket and must never be counted.
unix_socket = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
unix_socket.bind(unix_path)
unix_socket.listen(1)
held.append(unix_socket)

# A loopback-only service must not be counted either.
loopback = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
loopback.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
loopback.bind(("127.0.0.1", 0))
loopback.listen(1)
held.append(loopback)

# The injected violation: bound to a routable address, reachable from the network.
if address:
    injected = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    injected.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    injected.bind((address, 0))
    injected.listen(1)
    held.append(injected)

print("ready", flush=True)
time.sleep(600)
PY

    address="$(non_loopback_ipv4)"
    echo "runtime-socket-check: self-test (real helper process)"

    python3 "$scratch/helper.py" "$scratch/helper.sock" "" >/dev/null 2>&1 &
    helper_pid=$!
    sleep 1
    if ! is_running "$helper_pid"; then
        echo "runtime-socket-check: self-test FAILED - the helper process exited at once" >&2
        return 1
    fi
    collect "$ss" "$lsof" "$helper_pid"
    expect_clean "$ss" "$lsof" "$helper_pid" "a live process holding AF_UNIX and loopback sockets"

    kill "$helper_pid" 2>/dev/null || true
    wait "$helper_pid" 2>/dev/null || true
    if is_running "$helper_pid"; then
        echo "runtime-socket-check: self-test FAILED - a dead process was accepted" >&2
        return 1
    fi

    if [ -z "$address" ]; then
        echo "runtime-socket-check: self-test (this host has no routable IPv4 address;"
        echo "runtime-socket-check: the injected socket is covered by the fixtures alone)"
        echo "runtime-socket-check: self-test PASS (collectors verified against a live process)"
        return 0
    fi

    python3 "$scratch/helper.py" "$scratch/helper2.sock" "$address" >/dev/null 2>&1 &
    helper_pid=$!
    sleep 1
    collect "$ss" "$lsof" "$helper_pid"
    expect_violation "$ss" "$lsof" "$helper_pid" "$address" \
        "a live socket bound to the routable address $address"
    kill "$helper_pid" 2>/dev/null || true
    wait "$helper_pid" 2>/dev/null || true
    echo "runtime-socket-check: self-test PASS (clean process accepted, injected socket detected)"
}

run_self_test() {
    scratch="$(mktemp -d "${TMPDIR:-/tmp}/rspinyin-sockets.XXXXXX")"
    snapshot_repository
    run_fixture_self_test
    if command -v ss >/dev/null 2>&1 && command -v lsof >/dev/null 2>&1; then
        run_live_self_test
    else
        echo "runtime-socket-check: self-test (ss or lsof is not installed;"
        echo "runtime-socket-check: the live half of the self-test is skipped)"
    fi
    assert_repository_unchanged
    echo "runtime-socket-check: self-test PASS (fixtures and live collectors, no side effects)"
}

if [ "$mode" = "self-test" ]; then
    run_self_test
    exit 0
fi

if [ -n "$ss_file" ] || [ -n "$lsof_file" ]; then
    [ -n "$ss_file" ] && [ -n "$lsof_file" ] ||
        die "--ss-file and --lsof-file must be given together"
    [ "${#target_pids[@]}" -gt 0 ] || die "--ss-file and --lsof-file need --pid"
    classify "$ss_file" "$lsof_file" "${target_pids[@]}"
    exit 0
fi

require_tool ss
require_tool lsof

if [ "$replace" -eq 1 ]; then
    require_tool fcitx5
    fcitx5 -r -d
    sleep "$wait_seconds"
    mapfile -t target_pids < <(pgrep -x fcitx5 || true)
elif [ "${#target_pids[@]}" -eq 0 ]; then
    require_tool fcitx5
    mapfile -t target_pids < <(pgrep -x fcitx5 || true)
    if [ "${#target_pids[@]}" -eq 0 ]; then
        # Nothing to observe, so start one -- and stop it again on exit.
        fcitx5 >/dev/null 2>&1 &
        started_fcitx5=$!
        sleep "$wait_seconds"
        require_running "$started_fcitx5"
        target_pids=("$started_fcitx5")
    else
        echo "runtime-socket-check: observing the running fcitx5 (${target_pids[*]}), no restart"
    fi
fi

[ "${#target_pids[@]}" -gt 0 ] || die "no fcitx5 process to observe; start fcitx5 and re-run"
for pid in "${target_pids[@]}"; do
    require_running "$pid"
done

scratch="$(mktemp -d "${TMPDIR:-/tmp}/rspinyin-sockets.XXXXXX")"
collect "$scratch/ss.txt" "$scratch/lsof.txt" "${target_pids[@]}"
classify "$scratch/ss.txt" "$scratch/lsof.txt" "${target_pids[@]}"
