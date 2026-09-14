#!/usr/bin/env bash
# Serialize expensive cargo gates across concurrent worker sessions on ONE machine.
#
# Idea: `mkdir` is atomic on every POSIX filesystem, so an empty directory is a
# mutex that works without flock (which macOS does not ship as a CLI). But a bare
# mkdir-retry loop has NO fairness: every waiter races on each retry, so a waiter
# can be beaten indefinitely by freshly-arriving contenders. That starved a
# reviewer's gate for 25 minutes on 2026-08-15 while new worker gates kept
# jumping ahead -- which deadlocks the REVIEW end of the loop while the IMPLEMENT
# end keeps feeding it.
#
# So arrival order is made explicit: each waiter drops a ticket file into a queue
# directory and may only attempt the mutex when its ticket sorts first. Tickets
# are "<prio>-<arrival-ns>-<pid>", so ordering is priority, then FIFO. Review
# gates (labels `pr-*` / `review-*`) take priority 0 because the queue drains
# only through them; everything else is priority 1. Dead waiters' tickets are
# reaped, so a crashed session cannot wedge the queue.
#
# Usage:  scripts/gate-lock.sh <label> <command...>
#   scripts/gate-lock.sh 331 env CARGO_TARGET_DIR=/tmp/gate-331 make ci
#   scripts/gate-lock.sh pr-440 env CARGO_TARGET_DIR=/tmp/gate-pr-440 make ci
#
# NOTE: only *building* needs this lock. Running an already-built binary (a smoke
# run) allocates no target dir and burns no disk -- do not queue for that.
set -uo pipefail

# Overridable only so the lock logic itself can be tested without touching the real,
# possibly-held gate lock. Production callers must leave these at the default.
LOCK=${GATE_LOCK_DIR:-/tmp/openroad-gate.lock}
QUEUE=${GATE_QUEUE_DIR:-${LOCK}.queue}
LABEL="${1:?usage: gate-lock.sh <label> <command...>}"; shift
[ "$#" -gt 0 ] || { echo "gate-lock: no command given" >&2; exit 2; }

# Priority 0 = review/merge gates, 1 = everything else. See the header comment.
case "$LABEL" in pr-*|review-*) PRIO=0 ;; *) PRIO=1 ;; esac

# Arrival stamp with sub-second resolution. macOS `date` has no %N, so use python3
# (already a build dependency: scripts/check_warnings.py, scripts/check_ssot_fresh.py)
# and fall back to whole seconds if it is somehow unavailable.
NOW_NS=$(python3 -c 'import time;print(f"{time.time_ns():020d}")' 2>/dev/null) \
  || NOW_NS=$(printf '%020d' "$(( $(date +%s) * 1000000000 ))")

mkdir -p "$QUEUE" 2>/dev/null
TICKET="$PRIO-$NOW_NS-$$"
: > "$QUEUE/$TICKET"
cleanup() { rm -f "$QUEUE/$TICKET"; rm -rf "$LOCK"; }
trap cleanup EXIT INT TERM

WAITED=0
while :; do
  # Reap tickets whose owner died, or we would queue behind a ghost forever.
  for t in "$QUEUE"/*; do
    [ -e "$t" ] || continue
    tp=${t##*-}
    [ "$tp" = "$$" ] && continue
    kill -0 "$tp" 2>/dev/null || rm -f "$t"
  done

  HEAD=$(ls "$QUEUE" 2>/dev/null | sort | head -1)
  if [ "$HEAD" = "$TICKET" ] && mkdir "$LOCK" 2>/dev/null; then
    break
  fi

  # Reap a lock whose holder died. Claim it by atomic RENAME, never `rm -rf`: two
  # waiters can observe the same dead pid, and with rm both delete -- the first
  # then wins mkdir and the second's rm removes the WINNER's fresh lock, admitting
  # a second concurrent gate. rename() lets exactly one waiter move a directory.
  HOLDER=$(cat "$LOCK/pid" 2>/dev/null || echo "")
  if [ -n "$HOLDER" ] && ! kill -0 "$HOLDER" 2>/dev/null; then
    if mv "$LOCK" "$LOCK.stale.$$" 2>/dev/null; then
      echo "gate-lock[$LABEL]: reaped stale lock from dead pid $HOLDER" >&2
      rm -rf "$LOCK.stale.$$"
    fi
    continue
  fi

  if [ $((WAITED % 300)) -eq 0 ]; then
    AHEAD=$(ls "$QUEUE" 2>/dev/null | sort | grep -n -m1 -F "$TICKET" | cut -d: -f1)
    echo "gate-lock[$LABEL]: waiting ${WAITED}s — holder=$(cat "$LOCK/label" 2>/dev/null || echo none), queue position ${AHEAD:-?} of $(ls "$QUEUE" 2>/dev/null | wc -l | tr -d ' ')" >&2
  fi
  sleep 5; WAITED=$((WAITED + 5))
done

echo "$$" > "$LOCK/pid"; echo "$LABEL" > "$LOCK/label"
rm -f "$QUEUE/$TICKET"

# Disk guard: on macOS the sealed system volume "/" always reports ~12Gi free and
# is NOT where builds land. The real budget is the data volume.
AVAIL=$(df -g /System/Volumes/Data 2>/dev/null | awk 'NR==2{print $4}')
# Fail safe: if df broke, AVAIL is empty and `[ "" -lt 25 ]` would abort the test with
# "integer expression expected" and -- since we deliberately run without `set -e` --
# fall through leaving incremental compilation ON, exactly when we least know the disk.
case "$AVAIL" in ''|*[!0-9]*) AVAIL=0 ;; esac
echo "gate-lock[$LABEL]: acquired after ${WAITED}s, ${AVAIL}Gi free on data volume"
if [ "$AVAIL" -lt 25 ]; then
  echo "gate-lock[$LABEL]: WARNING only ${AVAIL}Gi free — exporting CARGO_INCREMENTAL=0" >&2
  export CARGO_INCREMENTAL=0
fi

"$@"; RC=$?
echo "gate-lock[$LABEL]: released, exit=$RC"
exit $RC
