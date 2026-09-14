#!/usr/bin/env bash
# safe-analyze — the never-execute quarantine front door for untrusted SRO drops.
#
# IDEA. SRO-scene binaries (client/server EXEs, installers, PK2 archives) are treated
# as trojaned by default, and the reference quarantine really did contain Swisyn-
# infected server EXEs. So nothing is decompiled, parsed or opened until it has a
# verdict — and the verdict is produced *without running the file*. The whole design
# is one idea: a disposable Debian container with no network, no capabilities and a
# read-only root filesystem reads a read-only COPY of the target and emits
# report.json. Everything the target could try (exfiltrate, escalate, fork-bomb,
# zip-bomb, write to the host) is denied by a docker flag, not by trust.
#
# Ported from the same-owner sibling workspace sro-rs-client/scripts/safe-analyze.sh
# (spec: docs/re/machinery/safe-analyze-port.md). The container envelope is verbatim;
# the deltas are: quarantine root is parameterized (OPENROAD_QUARANTINE_DIR) and there
# is deliberately NO download path — the script accepts local paths only.
#
# Build (first time / refresh sigs):  scripts/re/safe-analyze.sh --build
# Analyze:                            scripts/re/safe-analyze.sh <archive-or-dir>
#
# Exit codes (from the container's analyze.py):
#    0  clean       — proceed safely
#   10  suspicious  — human review required before further steps
#   20  infected    — STOP, ClamAV match
#   30  error       — analysis itself failed
#  any other: tooling / Docker error
#
# Testing hooks (used by scripts/test_safe_analyze.sh, never in production use):
#   SAFE_ANALYZE_DOCKER   path to the docker executable (default: docker)

set -euo pipefail

IMAGE_TAG="${SAFE_ANALYZE_IMAGE:-openroad-safe-analyze:latest}"
REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DOCKERFILE_DIR="${REPO_DIR}/scripts/re/safe-analyze"
DOCKER="${SAFE_ANALYZE_DOCKER:-docker}"
QUARANTINE_DIR="${OPENROAD_QUARANTINE_DIR:-${HOME}/sro-quarantine}"
OUT_BASE="${QUARANTINE_DIR}/safe-analyze"

usage() {
  cat <<EOF
usage: $0 [--build] <archive-or-dir>
       $0 --build

  --build         (re)build the analysis container, refreshing baked sigs
  <input>         LOCAL file or directory to analyze (must exist)

This tool never downloads anything and never executes the analyzed file.
Verdict + JSON report land in:
  ${OUT_BASE}/<input-basename>-<timestamp>/output/report.json
EOF
  exit 2
}

DO_BUILD=0
INPUT=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --build) DO_BUILD=1; shift ;;
    -h|--help) usage ;;
    -*) echo "unknown option: $1" >&2; usage ;;
    *) INPUT="$1"; shift ;;
  esac
done

# HARD BOUND: no remote fetch. The sibling's vsro-fetch.sh had a `curl -L` branch;
# it is deliberately not ported (spec §8 Delta A) — agents and tooling never
# download binaries. A URL is rejected rather than fetched.
if [[ "${INPUT}" == *://* ]]; then
  echo "refusing a URL: this tool never downloads. Pass a local path." >&2
  exit 1
fi

if ! command -v "${DOCKER}" >/dev/null 2>&1; then
  echo "docker not installed. Install Docker Desktop or Colima:" >&2
  echo "  brew install --cask docker" >&2
  echo "  OR: brew install colima docker && colima start" >&2
  exit 1
fi

if ! "${DOCKER}" info >/dev/null 2>&1; then
  echo "docker daemon not reachable. Start Docker Desktop or run: colima start" >&2
  exit 1
fi

if (( DO_BUILD )) || ! "${DOCKER}" image inspect "${IMAGE_TAG}" >/dev/null 2>&1; then
  echo "==> building ${IMAGE_TAG} (one-time, ~2-5 min; refreshes ClamAV sigs)"
  "${DOCKER}" build -t "${IMAGE_TAG}" "${DOCKERFILE_DIR}"
fi

if [[ -z "${INPUT}" ]]; then
  if (( DO_BUILD )); then
    echo "==> build complete; pass an input to analyze."
    exit 0
  fi
  usage
fi

if [[ ! -e "${INPUT}" ]]; then
  echo "input does not exist: ${INPUT}" >&2
  exit 1
fi

INPUT_ABS="$(cd "$(dirname "${INPUT}")" && pwd)/$(basename "${INPUT}")"
TS="$(date -u '+%Y%m%dT%H%M%SZ')"
RUN_DIR="${OUT_BASE}/$(basename "${INPUT_ABS}")-${TS}"
INPUT_STAGE="${RUN_DIR}/input"
OUTPUT_STAGE="${RUN_DIR}/output"
mkdir -p "${INPUT_STAGE}" "${OUTPUT_STAGE}"

# Stage a COPY read-only, so neither the container nor a bug here can mutate the
# original drop. The container only ever sees ${INPUT_STAGE}.
if [[ -d "${INPUT_ABS}" ]]; then
  cp -a "${INPUT_ABS}" "${INPUT_STAGE}/"
else
  cp -p "${INPUT_ABS}" "${INPUT_STAGE}/"
fi
chmod -R a-w "${INPUT_STAGE}"

echo "==> analyzing $(basename "${INPUT_ABS}") (output: ${OUTPUT_STAGE})"
# The target is DATA here, never a command: it is a read-only bind mount, and the
# only executable named on this line is the sandbox image's own entrypoint.
set +e
"${DOCKER}" run --rm \
  --network=none \
  --read-only \
  --tmpfs /tmp:rw,size=512m,exec \
  --cap-drop=ALL \
  --security-opt=no-new-privileges \
  --memory=2g --cpus=2 \
  --pids-limit=512 \
  -v "${INPUT_STAGE}:/work/input:ro" \
  -v "${OUTPUT_STAGE}:/work/output:rw" \
  "${IMAGE_TAG}"
RC=$?
set -e

REPORT="${OUTPUT_STAGE}/report.json"
if [[ -f "${REPORT}" ]]; then
  echo ""
  echo "==> verdict summary"
  if command -v jq >/dev/null 2>&1; then
    jq '{verdict, verdict_reasons, file_count: (.files|length), clamav_hits: (.clamav.hits|length), yara_hits: (.yara_hits|length)}' "${REPORT}"
  else
    head -40 "${REPORT}"
  fi
  echo ""
  echo "full report: ${REPORT}"
else
  echo "no report produced (container exit ${RC})" >&2
fi

case "${RC}" in
  0)  echo "clean — safe to proceed" ;;
  10) echo "SUSPICIOUS — review report before next step" ;;
  20) echo "INFECTED — do NOT run, do NOT decompile in unsandboxed Ghidra" ;;
  30) echo "analysis failed — see report" ;;
  *)  echo "docker/run error" ;;
esac
exit "${RC}"
