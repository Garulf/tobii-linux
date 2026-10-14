#!/usr/bin/env bash
# Apply the named-pipe listing patch to a Wine source tree (Wine 10.0 or later, Proton 9 or later).
#
# Usage: patches/wine/apply.sh <wine-source-dir>
#        patches/wine/apply.sh --emit [wine-source-dir] > tobii-pipe-listing.mypatch
#
# Three variants exist: the upstream one, made against Wine master; one for
# Valve's Proton 10 and later trees (and GE-Proton), which already carry a
# single-entry pipe listing that the patch keeps as a fallback; and one for
# Proton 9, whose Wine 9.0 base has no named pipe root directory object. The
# tree decides which applies.
# The generated protocol files differ on every release, so their hunks are
# skipped and regenerated with tools/make_requests; the remaining hunks apply
# with fuzz. --emit prints the patch without those hunks for build systems that
# apply user patches and then run make_requests themselves (wine-tkg).
set -euo pipefail
patch_dir=$(cd "$(dirname "$0")" && pwd)
patch_name=0001-ntdll-Implement-NtQueryDirectoryFile-on-the-named-pi.patch

emit=false
if [ "${1:-}" = --emit ]; then
    emit=true
    shift
fi

is_proton_tree() {
    grep -q server_query_directory_file "$1/dlls/ntdll/unix/file.c"
}

has_pipe_root_directory() {
    grep -q named_pipe_dir_ops "$1/server/named_pipe.c"
}

select_patch() {
    if [ -z "${1:-}" ]; then
        echo "$patch_dir/$patch_name"
    elif ! has_pipe_root_directory "$1"; then
        if is_proton_tree "$1"; then
            echo "$patch_dir/proton-9.0/$patch_name"
        else
            return 1
        fi
    elif is_proton_tree "$1"; then
        echo "$patch_dir/proton/$patch_name"
    else
        echo "$patch_dir/$patch_name"
    fi
}

scan_state_owner() {
    if has_pipe_root_directory .; then
        echo named_pipe_dir_destroy
    else
        echo named_pipe_device_file_destroy
    fi
}

without_generated_files() {
    awk '
        /^diff --git / {
            skip = ($3 ~ /^a\/(include\/wine\/server_protocol\.h|server\/request_handlers\.h|server\/request_trace\.h)$/)
        }
        !skip
    ' "$1"
}

function_contains() {
    awk -v start="$2" '$0 ~ start, /^}/' "$1" | grep -qF "$3"
}

check_placement() {
    function_contains server/named_pipe.c "^static void $(scan_state_owner)" 'free( file->mask )' &&
        function_contains dlls/ntdll/unix/file.c '^NTSTATUS WINAPI NtQueryDirectoryFile' 'query_named_pipe_directory('
}

if $emit; then
    without_generated_files "$(select_patch "${1:-}")"
    exit 0
fi

source_dir=${1:?wine source directory}
cd "$source_dir"
if ! patch_file=$(select_patch .); then
    echo "This Wine has no named pipe root directory object (added in 9.21); use 10.0 or later." >&2
    exit 1
fi
echo "Applying ${patch_file#"$patch_dir"/}"
without_generated_files "$patch_file" | patch -p1 --forward --fuzz=2
if ! check_placement; then
    echo "The patch applied, but a hunk landed in the wrong function; fix the tree by hand." >&2
    exit 1
fi
tools/make_requests
