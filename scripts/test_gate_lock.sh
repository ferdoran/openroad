#!/usr/bin/env bash
# Exercises gate-lock.sh against an ISOLATED lock+queue (GATE_LOCK_DIR /
# GATE_QUEUE_DIR) so it never disturbs a real gate that may be running.
#   T2 a lock held by a dead pid is reaped
#   T3 three waiters racing one dead lock still serialize strictly
#   T4 exit code propagates
#   T7 FIFO fairness: a late arrival cannot overtake an earlier waiter
#   T8 priority: a `pr-*` review gate overtakes queued worker gates
#   T9 a crashed waiter's ticket is reaped and does not wedge the queue
set -u
S="$(cd "$(dirname "$0")" && pwd)/gate-lock.sh"
export GATE_LOCK_DIR=/tmp/gl-test.lock GATE_QUEUE_DIR=/tmp/gl-test.queue
reset() { rm -rf "$GATE_LOCK_DIR" "$GATE_QUEUE_DIR" /tmp/gl-conc.log "$GATE_LOCK_DIR".stale.* 2>/dev/null; }
reset

echo "== T2 stale lock from a DEAD pid is reaped =="
mkdir -p "$GATE_LOCK_DIR"; echo 999999 > "$GATE_LOCK_DIR/pid"; echo deadguy > "$GATE_LOCK_DIR/label"
$S t2 true >/dev/null 2>&1 && echo "T2 ok (reaped)" || echo "T2 FAIL"
reset

echo "== T3 3 waiters race a DEAD lock; mutual exclusion must hold =="
mkdir -p "$GATE_LOCK_DIR"; echo 999999 > "$GATE_LOCK_DIR/pid"; echo deadguy > "$GATE_LOCK_DIR/label"
for i in 1 2 3; do
  ( $S "c$i" bash -c 'echo "IN $$" >> /tmp/gl-conc.log; sleep 2; echo "OUT $$" >> /tmp/gl-conc.log' >/dev/null 2>&1 ) &
done
wait
python3 -c "
ls=[l.split()[0] for l in open('/tmp/gl-conc.log')]
ok=len(ls)==6 and all(ls[i]==('IN' if i%2==0 else 'OUT') for i in range(6))
print('T3','ok - strict mutual exclusion' if ok else 'FAIL '+str(ls))"
reset

echo "== T7 FIFO: an EARLIER waiter must run before a LATER arrival =="
( $S first bash -c 'echo first >> /tmp/gl-conc.log; sleep 1' >/dev/null 2>&1 ) &
sleep 1
( $S second bash -c 'echo second >> /tmp/gl-conc.log' >/dev/null 2>&1 ) &
sleep 1
( $S third bash -c 'echo third >> /tmp/gl-conc.log' >/dev/null 2>&1 ) &
wait
echo "  order: $(tr '\n' ' ' < /tmp/gl-conc.log)"
[ "$(tr '\n' ' ' < /tmp/gl-conc.log)" = "first second third " ] && echo "T7 ok - FIFO held" || echo "T7 FAIL"
reset

echo "== T8 PRIORITY: a pr-* review gate overtakes already-queued worker gates =="
# holder occupies the lock; w1,w2 queue; THEN pr-9 arrives last and must still go first
( $S holder bash -c 'echo holder >> /tmp/gl-conc.log; sleep 4' >/dev/null 2>&1 ) &
sleep 1
( $S w1 bash -c 'echo w1 >> /tmp/gl-conc.log' >/dev/null 2>&1 ) &
sleep 1
( $S w2 bash -c 'echo w2 >> /tmp/gl-conc.log' >/dev/null 2>&1 ) &
sleep 1
( $S pr-9 bash -c 'echo pr-9 >> /tmp/gl-conc.log' >/dev/null 2>&1 ) &
wait
echo "  order: $(tr '\n' ' ' < /tmp/gl-conc.log)"
python3 -c "
o=[l.strip() for l in open('/tmp/gl-conc.log')]
print('T8','ok - review gate jumped the queue' if o[0]=='holder' and o[1]=='pr-9' else 'FAIL '+str(o))"
reset

echo "== T9 a CRASHED waiter's ticket is reaped, queue does not wedge =="
mkdir -p "$GATE_QUEUE_DIR"; : > "$GATE_QUEUE_DIR/0-00000000000000000001-999999"   # ghost, prio 0, oldest
$S t9 true >/dev/null 2>&1 && echo "T9 ok (ghost reaped)" || echo "T9 FAIL - wedged behind a dead ticket"
ls "$GATE_QUEUE_DIR" 2>/dev/null | grep -q 999999 && echo "  T9 FAIL residue" || echo "  T9 ok no residue"
reset

echo "== T4 exit code propagates =="
$S t4 bash -c 'exit 7' >/dev/null 2>&1; echo "  rc=$? (expect 7)"
reset
echo "== production lock untouched =="; cat /tmp/openroad-gate.lock/label 2>/dev/null || echo "(free)"
