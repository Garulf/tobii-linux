#!/usr/bin/env bash
# Apply the named-pipe listing patch to a Wine source tree (10.0 or later).
#
# Usage: patches/wine/apply.sh <wine-source-dir>
# The patch is made against Wine master. Its generated protocol files differ on
# every release, so they are skipped here and regenerated with
# tools/make_requests; the remaining hunks apply with fuzz.
set -euo pipefail
patch_file=$(cd "$(dirname "$0")" && pwd)/0001-ntdll-Implement-NtQueryDirectoryFile-on-the-named-pi.patch
source_dir=${1:?wine source directory}
cd "$source_dir"
if ! grep -q named_pipe_dir_ops server/named_pipe.c; then
    echo "This Wine has no named pipe root directory object (added in 9.21); use 10.0 or later." >&2
    exit 1
fi
awk '
    /^diff --git / {
        skip = ($3 ~ /^a\/(include\/wine\/server_protocol\.h|server\/request_handlers\.h|server\/request_trace\.h)$/)
    }
    !skip
' "$patch_file" | patch -p1 --forward --fuzz=2
tools/make_requests
