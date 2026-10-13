#!/bin/sh
# Usage: tools/ghidra/focused.sh <name> <address>...
# Writes evidence/private/<name>-focused.c and .log from the persistent
# no-analysis project; never reimports or runs whole-program analysis.
# GHIDRA_PROJECT_DIR selects a copy of the project for concurrent runs.
set -eu
root=$(cd "$(dirname "$0")/../.." && pwd)
name=$1
shift
export JAVA_HOME="$root/.local/tools/jdk25"
export PATH="$JAVA_HOME/bin:$PATH"
"$root/.local/tools/ghidra_12.1.4_PUBLIC/support/analyzeHeadless" \
    "${GHIDRA_PROJECT_DIR:-$root/evidence/private/ghidra-focused}" geometry \
    -process platform_runtime_IS5LEYETRACKER5_service.exe -noanalysis \
    -scriptPath "$root/tools/ghidra" \
    -postScript HeadposeFocused.java "$root/evidence/private/$name-focused.c" "$@" \
    > "$root/evidence/private/$name-focused.log" 2>&1
grep -c '^ADDRESS' "$root/evidence/private/$name-focused.c"
