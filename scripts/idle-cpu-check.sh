#!/usr/bin/env bash
#
# idle-cpu-check.sh - assert that an idle candidate window costs nothing.
#
# BUDGET-CPU-01 is the claim this script tests: with no input and the candidate window
# hidden, the plugin stays inside its idle CPU ceiling, redraws nothing, and runs no polling
# timer. Two halves of that are measurable from outside the process, and both have to hold:
#
#   cpu     `pidstat -p PID SECONDS`, the collector the budget table names, measures the
#           process over the whole window. An independent `/proc/PID/stat` delta is taken
#           across the same window as well, and *either* collector over the threshold fails
#           the run: a collector that cannot see the process cannot certify its silence, and
#           one tool's number is not evidence about another's.
#
#   redraw  the UI thread's render counter, read before and after the window through a
#           command the caller names (`--counter-cmd`), because the counter lives inside the
#           plugin's process. Its delta must be no larger than `cpu_pct.idle_redraw_count`,
#           which the budget states as zero. A counter that goes backwards means the process
#           restarted inside the window, so the run measured two processes and is refused
#           rather than reported.
#
# Both thresholds are read from `docs/dev/budgets.json`, the machine-readable mirror of the
# budget table, so this script never states a number the specification owns.
#
# The window is a live, idle session: the script never starts, stops or reconfigures fcitx5,
# because an idle measurement of a process it had just restarted would be a measurement of
# the restart.
#
# The timer-free half of BUDGET-CPU-01 is not measured here. It is a property of the loop's
# code rather than of a running process -- an idle surface makes the `poll(2)` timeout
# infinite -- and it is asserted beside the loop, in `ui_thread/event_loop.rs`.
#
# Exit codes: 0 = pass, 1 = violation, 2 = usage or environment error.
#
# Self-test: `--self-test` classifies synthetic `pidstat` listings -- one inside the budget,
# one past it, one with no header and one with an unreadable column -- and drives the redraw
# half with a counter that stands still, one that advances and one that rewinds, so both
# halves are proven sensitive rather than just the parser. The listings are derived from the
# threshold the budget document carries, so the self-test keeps working when the budget is
# revised.
#
# Requires sysstat (`pidstat`) and python3 (present by default on every distro in the
# platform baseline).

set -euo pipefail

usage() {
    cat <<'USAGE'
usage: scripts/idle-cpu-check.sh --counter-cmd CMD [--pid PID] [--seconds N]
                                 [--threshold PCT] [--pidstat-file FILE]
       scripts/idle-cpu-check.sh --self-test

  --pid PID          process to observe (default: the running fcitx5)
  --seconds N        length of the idle window in seconds (default: 60)
  --counter-cmd CMD  command that prints the UI thread's render counter, run under `sh -c`
                     before and after the window; required, because the redraw half of the
                     budget cannot be read from outside the process
  --threshold PCT    idle CPU ceiling as a percentage of one core (default: cpu_pct.idle
                     from docs/dev/budgets.json)
  --pidstat-file F   classify a captured `pidstat` listing instead of running pidstat; no
                     process is then observed and no window is waited out
  --self-test        exercise both halves against synthetic input
USAGE
}

die() {
    echo "idle-cpu-check: $*" >&2
    exit 2
}

mode="check"
pid=""
seconds=60
threshold=""
counter_cmd=""
pidstat_file=""
root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
scratch=""

cleanup() {
    if [ -n "$scratch" ]; then
        rm -rf -- "$scratch"
    fi
}
trap cleanup EXIT

while [ $# -gt 0 ]; do
    case "$1" in
        --self-test) mode="self-test"; shift ;;
        --pid)
            pid="${2:?--pid needs a process id}"
            shift 2
            ;;
        --seconds)
            seconds="${2:?--seconds needs a number}"
            shift 2
            ;;
        --threshold)
            threshold="${2:?--threshold needs a percentage}"
            shift 2
            ;;
        --counter-cmd)
            counter_cmd="${2:?--counter-cmd needs a command}"
            shift 2
            ;;
        --pidstat-file)
            pidstat_file="${2:?--pidstat-file needs a path}"
            shift 2
            ;;
        -h | --help)
            usage
            exit 0
            ;;
        *)
            echo "idle-cpu-check: unknown argument: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

# read_budgets: print "<idle cpu ceiling> <idle redraw ceiling>" from the budget document.
read_budgets() {
    python3 - "$root/docs/dev/budgets.json" <<'PY'
import json
import sys

try:
    with open(sys.argv[1], encoding="utf-8") as document:
        cpu = json.load(document)["cpu_pct"]
except (OSError, KeyError, ValueError) as error:
    sys.exit(f"idle-cpu-check: cannot read cpu_pct from {sys.argv[1]}: {error}")

print(f"{cpu['idle']} {cpu['idle_redraw_count']}")
PY
}

# classify_listing FILE: print "<pid> <cpu>" for the newest row of a pidstat listing.
#
# The `%CPU` column is located by name in the header rather than by position, because its
# position depends on the sysstat version (`%wait` is not in every one) and on the line's
# shape: a sample row starts with a clock, an `Average:` row does not.
classify_listing() {
    python3 - "$1" <<'PY'
import re
import sys

path = sys.argv[1]
try:
    with open(path, encoding="utf-8", errors="replace") as listing:
        lines = listing.read().splitlines()
except OSError as error:
    sys.exit(f"idle-cpu-check: cannot read {path}: {error}")

CLOCK = re.compile(r"^\d{1,2}:\d{2}:\d{2}$")

header = None
for line in lines:
    fields = line.split()
    if "UID" in fields and "%CPU" in fields:
        header = fields
        break
if header is None:
    sys.exit(f"idle-cpu-check: {path} holds no pidstat header naming UID and %CPU")

# Column offsets relative to UID, which is the first column of a normalized data row.
uid_at = header.index("UID")
pid_at = header.index("PID") - uid_at
cpu_at = header.index("%CPU") - uid_at

row = None
for line in lines:
    fields = line.split()
    if not fields:
        continue
    if fields[0] == "Average:":
        fields = fields[1:]
    elif CLOCK.match(fields[0]):
        fields = fields[1:]
        if fields and fields[0] in ("AM", "PM"):
            fields = fields[1:]
    else:
        continue
    if len(fields) > max(pid_at, cpu_at):
        row = fields

if row is None:
    sys.exit(f"idle-cpu-check: {path} holds no pidstat data row")

try:
    print(f"{int(row[pid_at])} {float(row[cpu_at])}")
except ValueError:
    sys.exit(f"idle-cpu-check: {path} has an unreadable row: {' '.join(row)}")
PY
}

# read_proc_ticks PID: print the process's own CPU time, in clock ticks.
#
# The command name sits in parentheses and may contain spaces and parentheses of its own, so
# the fields are counted from the last ')': the state field follows it, which makes utime and
# stime fields 14 and 15 of the line, that is indices 11 and 12 of what follows.
read_proc_ticks() {
    python3 - "$1" <<'PY'
import sys
from pathlib import Path

path = Path(f"/proc/{sys.argv[1]}/stat")
try:
    stat = path.read_text(encoding="utf-8", errors="replace")
    rest = stat[stat.rindex(")") + 1 :].split()
    ticks = int(rest[11]) + int(rest[12])
except (OSError, ValueError, IndexError) as error:
    sys.exit(f"idle-cpu-check: cannot read {path}: {error}")

print(ticks)
PY
}

# now_nanos: print the wall clock, in nanoseconds since the epoch.
now_nanos() {
    python3 -c 'import time; print(time.time_ns())'
}

# proc_percent BEFORE_TICKS AFTER_TICKS BEFORE_NS AFTER_NS: the process's CPU usage over the
# window, as a percentage of one core, from the two `/proc/PID/stat` samples.
proc_percent() {
    python3 - "$@" <<'PY'
import os
import sys

before_ticks, after_ticks, before_ns, after_ns = (int(value) for value in sys.argv[1:5])
wall_ns = after_ns - before_ns
if wall_ns <= 0:
    print("idle-cpu-check: the observed window has no duration", file=sys.stderr)
    sys.exit(2)

hz = os.sysconf("SC_CLK_TCK")
ticks = after_ticks - before_ticks
print(f"{ticks / hz / (wall_ns / 1e9) * 100:.4f}")
PY
}

# read_counter CMD: run CMD under `sh -c` and print the single non-negative integer it
# prints.
read_counter() {
    local value=""
    if ! value="$(sh -c "$1")"; then
        die "the counter command failed: $1"
    fi
    case "$value" in
        '' | *[!0-9]*) die "the counter command must print one non-negative integer, got: $value" ;;
    esac
    printf '%s' "$value"
}

# resolve_pid: print the pid of the one running fcitx5.
resolve_pid() {
    local pids=""
    pids="$(pgrep -x fcitx5 || true)"
    if [ -z "$pids" ]; then
        die "no fcitx5 is running: start a session, or name the process with --pid"
    fi
    if [ "$(printf '%s\n' "$pids" | wc -l)" -ne 1 ]; then
        die "several fcitx5 processes are running: name the one to observe with --pid"
    fi
    printf '%s' "$pids"
}

# verdict THRESHOLD REDRAW_LIMIT PIDSTAT_PCT PROC_PCT COUNTER_BEFORE COUNTER_AFTER: compare
# the measurements against the budget and exit 0 or 1.
#
# An empty PIDSTAT_PCT or PROC_PCT means that collector did not run; the summary says so
# rather than reporting it as zero.
verdict() {
    python3 - "$@" <<'PY'
import sys

threshold = float(sys.argv[1])
redraw_limit = int(sys.argv[2])
pidstat_pct = sys.argv[3]
proc_pct = sys.argv[4]
counter_before = int(sys.argv[5])
counter_after = int(sys.argv[6])

failures = []

if pidstat_pct:
    measured = float(pidstat_pct)
    print(f"idle-cpu-check: pidstat measured {measured:.3f}% of one core (ceiling {threshold}%)")
    if measured > threshold:
        failures.append(f"pidstat measured {measured:.3f}%, past the {threshold}% ceiling")
else:
    print("idle-cpu-check: pidstat did not run (a captured listing was classified instead)")

if proc_pct:
    measured = float(proc_pct)
    print(f"idle-cpu-check: /proc measured {measured:.3f}% of one core (ceiling {threshold}%)")
    if measured > threshold:
        failures.append(f"the /proc delta measured {measured:.3f}%, past the {threshold}% ceiling")
else:
    print("idle-cpu-check: /proc was not sampled (no window was observed)")

redraws = counter_after - counter_before
print(
    f"idle-cpu-check: the render counter went {counter_before} -> {counter_after} "
    f"({redraws} redraw(s), ceiling {redraw_limit})"
)
if redraws < 0:
    print(
        "idle-cpu-check: the render counter went backwards, so the process restarted inside "
        "the window; the run measured two processes and is refused",
        file=sys.stderr,
    )
    sys.exit(2)
if redraws > redraw_limit:
    failures.append(f"{redraws} redraw(s) during an idle window, past the ceiling of {redraw_limit}")

if failures:
    print("idle-cpu-check: FAIL", file=sys.stderr)
    for failure in failures:
        print(f"  - {failure}", file=sys.stderr)
    print(
        "idle-cpu-check: BUDGET-CPU-01 requires an idle window that neither burns CPU nor "
        "redraws",
        file=sys.stderr,
    )
    sys.exit(1)

print("idle-cpu-check: PASS")
PY
}

# write_listing FILE CPU: write a pidstat listing carrying CPU in both the sample row and the
# average row, which is the shape sysstat prints.
write_listing() {
    cat >"$1" <<EOF
Linux 6.18.0 (idle-cpu-check) 	09/29/2026 	_x86_64_	(8 CPU)

#      Time   UID       PID    %usr %system  %guest   %wait    %CPU   CPU  Command
09:04:20 AM  1000      4242    0.00    0.00    0.00    0.00    $2     0  fcitx5
Average:     1000      4242    0.00    0.00    0.00    0.00    $2     -  fcitx5
EOF
}

# derive_listings THRESHOLD: print "<inside> <outside>", two percentages either side of the
# ceiling, so the self-test keeps working when the budget is revised.
derive_listings() {
    python3 - "$1" <<'PY'
import sys

threshold = float(sys.argv[1])
inside = threshold / 2
outside = threshold * 2 if threshold > 0 else 1.0
print(f"{inside:.3f} {outside:.3f}")
PY
}

# run_case EXPECTED_STATUS EXPECTED_TEXT DESCRIPTION ARGS...: run this script with ARGS and
# assert its exit status and that its report mentions EXPECTED_TEXT.
run_case() {
    local expected="$1" text="$2" description="$3"
    shift 3
    local status=0 output=""
    if output="$(bash "${BASH_SOURCE[0]}" "$@" 2>&1)"; then
        status=0
    else
        status=$?
    fi
    if [ "$status" -ne "$expected" ]; then
        echo "idle-cpu-check: self-test FAILED - $description exited $status, expected $expected" >&2
        echo "$output" >&2
        return 1
    fi
    case "$output" in
        *"$text"*) return 0 ;;
        *)
            echo "idle-cpu-check: self-test FAILED - $description was reported for the wrong reason" >&2
            echo "idle-cpu-check: the report had to mention: $text" >&2
            echo "$output" >&2
            return 1
            ;;
    esac
}

run_self_test() {
    scratch="$(mktemp -d "${TMPDIR:-/tmp}/rspinyin-idle.XXXXXX")"
    echo "idle-cpu-check: self-test (synthetic pidstat listings and counter commands)"

    local budget_line="" idle_threshold="" redraw_limit="" inside="" outside=""
    if ! budget_line="$(read_budgets)"; then
        return 1
    fi
    read -r idle_threshold redraw_limit <<<"$budget_line"
    read -r inside outside <<<"$(derive_listings "$idle_threshold")"

    write_listing "$scratch/inside.txt" "$inside"
    write_listing "$scratch/outside.txt" "$outside"
    write_listing "$scratch/unreadable.txt" "n/a"
    printf 'not a pidstat listing\n' >"$scratch/broken.txt"

    cat >"$scratch/steady.sh" <<'EOF'
#!/bin/sh
echo 4242
EOF
    cat >"$scratch/advancing.sh" <<'EOF'
#!/bin/sh
state="$1"
value=$(cat "$state" 2>/dev/null || echo 0)
echo $((value + 1)) >"$state"
echo "$value"
EOF
    cat >"$scratch/rewinding.sh" <<'EOF'
#!/bin/sh
state="$1"
value=$(cat "$state" 2>/dev/null || echo 5)
echo $((value - 1)) >"$state"
echo "$value"
EOF

    run_case 0 "" "a listing inside the budget" \
        --pidstat-file "$scratch/inside.txt" --counter-cmd "sh $scratch/steady.sh"
    run_case 1 "$idle_threshold" "a listing past the budget" \
        --pidstat-file "$scratch/outside.txt" --counter-cmd "sh $scratch/steady.sh"
    run_case 1 "redraw" "a render counter that advances" \
        --pidstat-file "$scratch/inside.txt" \
        --counter-cmd "sh $scratch/advancing.sh $scratch/advancing.state"
    run_case 2 "backwards" "a render counter that rewinds" \
        --pidstat-file "$scratch/inside.txt" \
        --counter-cmd "sh $scratch/rewinding.sh $scratch/rewinding.state"
    run_case 2 "no pidstat header" "a listing with no header" \
        --pidstat-file "$scratch/broken.txt" --counter-cmd "sh $scratch/steady.sh"
    run_case 2 "unreadable row" "a listing with an unreadable column" \
        --pidstat-file "$scratch/unreadable.txt" --counter-cmd "sh $scratch/steady.sh"
    run_case 2 "--counter-cmd is required" "a missing counter command" \
        --pidstat-file "$scratch/inside.txt"
    run_case 0 "" "the same listing with the violation removed" \
        --pidstat-file "$scratch/inside.txt" --counter-cmd "sh $scratch/steady.sh"

    echo "idle-cpu-check: self-test PASS (both halves are sensitive)"
}

if [ "$mode" = "self-test" ]; then
    run_self_test
    exit 0
fi

command -v python3 >/dev/null 2>&1 || die "python3 is required to read the measurement; install it and re-run"
[ -n "$counter_cmd" ] || die "--counter-cmd is required: the redraw half of BUDGET-CPU-01 is a counter inside the plugin's process, and a run that does not read it cannot certify zero redraws"
case "$seconds" in
    '' | *[!0-9]*) die "--seconds must be a whole number of seconds, got: $seconds" ;;
esac
[ "$seconds" -ge 1 ] || die "--seconds must be at least one second"

if [ -z "$pidstat_file" ]; then
    command -v pidstat >/dev/null 2>&1 || die "pidstat is required (it is the collector the budget table names); install sysstat, or classify a captured listing with --pidstat-file"
fi

if ! budget_line="$(read_budgets)"; then
    exit 2
fi
read -r budget_idle budget_redraw <<<"$budget_line"
[ -n "$threshold" ] || threshold="$budget_idle"
case "$threshold" in
    '' | *[!0-9.]*) die "--threshold must be a percentage, got: $threshold" ;;
esac

scratch="$(mktemp -d "${TMPDIR:-/tmp}/rspinyin-idle.XXXXXX")"
echo "idle-cpu-check: idle ceiling ${threshold}% of one core, redraw ceiling ${budget_redraw}"

counter_before="$(read_counter "$counter_cmd")"

if [ -n "$pidstat_file" ]; then
    [ -r "$pidstat_file" ] || die "cannot read the captured listing: $pidstat_file"
    listing="$pidstat_file"
    proc_pct=""
else
    [ -n "$pid" ] || pid="$(resolve_pid)"
    [ -d "/proc/$pid" ] || die "no process $pid to observe"
    echo "idle-cpu-check: observing pid $pid for ${seconds}s"
    before_ticks="$(read_proc_ticks "$pid")"
    before_ns="$(now_nanos)"
    if ! pidstat_output="$(LC_ALL=C pidstat -p "$pid" "$seconds" 1)"; then
        die "pidstat failed for pid $pid"
    fi
    after_ns="$(now_nanos)"
    after_ticks="$(read_proc_ticks "$pid")"
    proc_pct="$(proc_percent "$before_ticks" "$after_ticks" "$before_ns" "$after_ns")"
    listing="$scratch/pidstat.txt"
    printf '%s\n' "$pidstat_output" >"$listing"
fi

counter_after="$(read_counter "$counter_cmd")"

if ! row="$(classify_listing "$listing")"; then
    exit 2
fi
read -r measured_pid pidstat_pct <<<"$row"
if [ -n "$pid" ] && [ "$measured_pid" != "$pid" ]; then
    die "the listing reports pid $measured_pid, not the process that was observed ($pid)"
fi

verdict "$threshold" "$budget_redraw" "$pidstat_pct" "$proc_pct" "$counter_before" "$counter_after"
