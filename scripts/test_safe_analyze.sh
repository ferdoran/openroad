#!/usr/bin/env bash
# Exercises scripts/re/safe-analyze.sh WITHOUT docker and WITHOUT any SRO binary.
#
# IDEA. The security properties of safe-analyze are properties of the *command line
# it builds*, so we substitute a fake `docker` (SAFE_ANALYZE_DOCKER) that records its
# argv instead of running anything. That lets the test assert the whole sandbox
# envelope, and — more importantly — assert the negative: the analyzed sample is
# never executed. The sample used here is a host-generated shell script whose only
# effect would be to create a canary file; if that file ever appears, something ran
# the sample and the test fails loudly.
#
#   T1 the sandbox flags are all present on the docker command line
#   T2 the sample is mounted READ-ONLY as data and is never executed (canary)
#   T3 the original sample is not modified; the staged copy is non-writable
#   T4 nothing is written outside OPENROAD_QUARANTINE_DIR
#   T5 the container's verdict exit code propagates (20 = infected)
#   T6 a URL argument is refused — this tool never downloads
#   T7 no script in scripts/re/ names the analyzed input as a command
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
S="${HERE}/re/safe-analyze.sh"
TMP="$(mktemp -d /tmp/safe-analyze-test.XXXXXX)"
FAIL=0
ok()   { echo "$1 ok - $2"; }
bad()  { echo "$1 FAIL - $2"; FAIL=1; }

# --- fake docker: records argv, writes a report, exits with a chosen verdict ------
FAKE="${TMP}/bin"; mkdir -p "$FAKE"
cat > "${FAKE}/docker" <<'EOF'
#!/usr/bin/env bash
echo "$@" >> "${FAKE_LOG}"
case "$1" in
  info|inspect) exit 0 ;;
  image) exit 0 ;;
  run)
    # emulate the container writing its report into the :rw output mount
    for a in "$@"; do case "$a" in *:/work/output:rw) echo '{"verdict":"infected"}' > "${a%%:*}/report.json";; esac; done
    exit "${FAKE_VERDICT:-0}" ;;
esac
exit 0
EOF
chmod +x "${FAKE}/docker"
export FAKE_LOG="${TMP}/docker.log"; : > "$FAKE_LOG"
export SAFE_ANALYZE_DOCKER="${FAKE}/docker"
export OPENROAD_QUARANTINE_DIR="${TMP}/quarantine"

# --- the sample: an executable script that would leave a canary if ever run ------
CANARY="${TMP}/CANARY-SAMPLE-WAS-EXECUTED"
SAMPLE="${TMP}/dummy-drop.exe"
printf '#!/bin/sh\ntouch "%s"\n' "$CANARY" > "$SAMPLE"
chmod +x "$SAMPLE"
SAMPLE_SUM_BEFORE="$(shasum -a 256 < "$SAMPLE")"

FAKE_VERDICT=20 "$S" "$SAMPLE" >"${TMP}/run.out" 2>&1
RC=$?

RUNLINE="$(grep '^run ' "$FAKE_LOG" | head -1)"

echo "== T1 sandbox envelope present on the docker command line =="
MISSING=""
for flag in --network=none --read-only --cap-drop=ALL --security-opt=no-new-privileges \
            --pids-limit=512 --memory=2g --cpus=2 "--tmpfs /tmp:rw,size=512m,exec"; do
  case "$RUNLINE" in *"$flag"*) ;; *) MISSING="${MISSING} ${flag}";; esac
done
[ -z "$MISSING" ] && ok T1 "all hardening flags present" || bad T1 "missing:${MISSING}"

echo "== T2 sample mounted :ro as DATA and never executed =="
case "$RUNLINE" in *":/work/input:ro"*) ok T2a "input mounted read-only" ;;
                   *) bad T2a "no :ro input mount in: $RUNLINE" ;; esac
[ -e "$CANARY" ] && bad T2b "THE SAMPLE WAS EXECUTED" || ok T2b "canary absent - sample never executed"
# the last argument must be the image, not the sample
LAST="${RUNLINE##* }"
case "$LAST" in *safe-analyze*) ok T2c "image is the only executable named ($LAST)" ;;
                *) bad T2c "unexpected trailing argument: $LAST" ;; esac

echo "== T3 original untouched, staged copy non-writable =="
[ "$(shasum -a 256 < "$SAMPLE")" = "$SAMPLE_SUM_BEFORE" ] && ok T3a "original unmodified" || bad T3a "original changed"
STAGED="$(find "${OPENROAD_QUARANTINE_DIR}" -name "$(basename "$SAMPLE")" -type f 2>/dev/null | head -1)"
if [ -n "$STAGED" ] && [ ! -w "$STAGED" ]; then ok T3b "staged copy is chmod a-w"; else bad T3b "staged copy missing or writable: ${STAGED:-none}"; fi

echo "== T4 nothing written outside the quarantine dir =="
STRAY="$(find "$TMP" -newer "$FAKE_LOG" -type f 2>/dev/null | grep -v "^${OPENROAD_QUARANTINE_DIR}" | grep -v "${TMP}/run.out" | grep -v "$FAKE_LOG")"
[ -z "$STRAY" ] && ok T4 "no stray writes" || bad T4 "wrote outside quarantine: $STRAY"

echo "== T5 verdict exit code propagates =="
[ "$RC" = 20 ] && ok T5 "rc=20 (infected) propagated" || bad T5 "rc=$RC, expected 20"

echo "== T6 a URL is refused, never fetched =="
if "$S" "https://example.invalid/drop.zip" >"${TMP}/url.out" 2>&1; then
  bad T6 "accepted a URL"
else
  grep -qi "never downloads" "${TMP}/url.out" && ok T6 "URL refused" || bad T6 "wrong error: $(cat "${TMP}/url.out")"
fi

echo "== T7 no scripts/re/ script invokes the analyzed input =="
# Positive control first: the pattern must find the one legitimate `docker run`.
if grep -rn 'docker.*run' "${HERE}/re" >/dev/null 2>&1; then
  BADEXEC="$(grep -rnE '(^|[^#])[^ ]*(\$\{?INPUT|\$\{?TARGET|\$\{?SAMPLE)[A-Z_]*\}?"?[[:space:]]*$' "${HERE}/re" --include='*.sh' | grep -vE 'echo|cp |mkdir|chmod|\[\[|find|basename|dirname' || true)"
  [ -z "$BADEXEC" ] && ok T7 "no script executes its input (positive control: docker run found)" || bad T7 "possible execution of input: $BADEXEC"
else
  bad T7 "positive control failed - grep found no 'docker run' in scripts/re"
fi

chmod -R u+w "$TMP" 2>/dev/null
rm -rf "$TMP"
[ "$FAIL" = 0 ] && echo "ALL OK" || echo "FAILURES PRESENT"
exit "$FAIL"
