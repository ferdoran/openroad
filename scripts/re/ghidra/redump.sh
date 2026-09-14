#!/bin/zsh
# Bulk corpus extractor against the quarantined vSRO Ghidra project (static, read-only).
# usage: redump.sh <projdir> <program> <outdir> index
#        redump.sh <projdir> <program> <outdir> decompile <addr-list-file>
set -e
HERE="$(cd "$(dirname "$0")" && pwd)"
: "${GHIDRA_HOME:=/opt/homebrew/Cellar/ghidra/12.1.2/libexec}"
: "${JAVA_HOME:=/opt/homebrew/opt/openjdk@21}"
export JAVA_HOME PATH="$JAVA_HOME/bin:$PATH"
PROJ="$1"; PROG="$2"; OUT="$3"; MODE="$4"; LIST="$5"
"$GHIDRA_HOME/support/analyzeHeadless" "$PROJ" vsro \
  -process "$PROG" -noanalysis -readOnly \
  -scriptPath "$HERE" -postScript ReDump.java "$OUT" "$MODE" "$LIST" 2>&1 \
  | grep -E "ReDump|ERROR|Exception" | sed 's/ (GhidraScript)  $//'
