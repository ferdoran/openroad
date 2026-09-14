#!/bin/zsh
# Batch RE query against the quarantined vSRO Ghidra project (static, read-only).
# usage: requery.sh <program> <jobfile> <outfile> [projdir]
#   program: sro_client.exe | SR_GameServer_Clean.exe | SR_ShardManager.exe
# Job lines: id|decompile|<addrOrName> · id|bytes|<addr>|<len> · id|strings|<regex>|<limit>
#            id|xrefs|<addr> · id|funcs|<regex>|<limit> · id|callers|<f> · id|callees|<f>
#            id|disasm|<addr>|<n> · id|data|<addr>
set -e
HERE="$(cd "$(dirname "$0")" && pwd)"
: "${GHIDRA_HOME:=/opt/homebrew/Cellar/ghidra/12.1.2/libexec}"
: "${JAVA_HOME:=/opt/homebrew/opt/openjdk@21}"
export JAVA_HOME PATH="$JAVA_HOME/bin:$PATH"
PROG="$1"; JOB="$2"; OUT="$3"; PROJ="${4:-$HOME/sro-vsro-quarantine/ghidra-proj}"
"$GHIDRA_HOME/support/analyzeHeadless" "$PROJ" vsro \
  -process "$PROG" -noanalysis -readOnly \
  -scriptPath "$HERE" -postScript ReQuery.java "$JOB" "$OUT" 2>&1 \
  | grep -E "^(INFO  ReQuery|ERROR|Exception)" | sed 's/ (GhidraScript)  $//'
