#!/bin/sh
# Usage: GHIDRA_PROJECT_DIR=<copy> tools/ghidra/focused-dll.sh <name> <address>...
# Same as focused.sh, for the game DLL program in a "dlls" project copy
# (evidence/private/ghidra-copy-*-dlls). Never reimports or runs analysis.
# GHIDRA_PROGRAM selects another program in that project.
set -eu
root=$(cd "$(dirname "$0")/../.." && pwd)
name=$1
shift
: "${GHIDRA_PROJECT_DIR:?set GHIDRA_PROJECT_DIR to a private project copy}"
export JAVA_HOME="$root/.local/tools/jdk25"
export PATH="$JAVA_HOME/bin:$PATH"
"$root/.local/tools/ghidra_12.1.4_PUBLIC/support/analyzeHeadless" \
    "$GHIDRA_PROJECT_DIR" dlls \
    -process "${GHIDRA_PROGRAM:-tobii_gameintegration_x64.dll}" -noanalysis \
    -scriptPath "$root/tools/ghidra" \
    -postScript HeadposeFocused.java "$root/evidence/private/$name-focused.c" "$@" \
    > "$root/evidence/private/$name-focused.log" 2>&1
grep -c '^ADDRESS' "$root/evidence/private/$name-focused.c"
