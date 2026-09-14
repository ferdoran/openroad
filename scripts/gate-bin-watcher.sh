#!/usr/bin/env bash
# Gate-medic binary rescuer: the central gate builds every branch into the ONE
# shared target dir, so /tmp/gate-shared/debug/client is overwritten by the next
# gate. Snapshot it, keyed by its own mtime, so a §4b smoke can be run later on
# the exact binary a given gate produced instead of rebuilding (runbook §4a-bis).
set -u
BIN=/tmp/gate-shared/debug/client
OUT=/tmp/gate-bins
mkdir -p "$OUT"
last=""
while true; do
  if [ -f "$BIN" ]; then
    m=$(stat -f %m "$BIN")
    if [ "$m" != "$last" ]; then
      sleep 3                       # let the linker finish
      m=$(stat -f %m "$BIN")
      cp "$BIN" "$OUT/client-$m.tmp" && mv "$OUT/client-$m.tmp" "$OUT/client-$m"
      last=$m
      ls -1t "$OUT"/client-* 2>/dev/null | tail -n +4 | xargs -r rm -f
      echo "$(date -Iseconds) snapshot client-$m" >> "$OUT/watcher.log"
    fi
  fi
  sleep 5
done
