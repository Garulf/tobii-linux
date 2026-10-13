# Named pipe enumeration under Wine

The game integration DLL finds the PRP service by listing `\\.\pipe\*` and
keeping names that start with `TOBIIPRP-` (see `docs/service-transport.md`,
section Discovery). Wine cannot list that directory, so the DLL never sees a
service pipe that our Linux service creates, even though `WaitNamedPipeA` and
`CreateFileA` on the exact name work.

This document covers the cause, a Wine patch that adds the listing, how it was
built and tested, and how to apply it to a Wine runner.

Upstream tracks the same problem as WineHQ bug 55485, "FindFirstFileA fails on
\\.\pipe\*" (https://bugs.winehq.org/show_bug.cgi?id=55485), open since Wine
8.12.

## Problem

`evidence/private/wine-tests/pipelist.c` creates `\\.\pipe\TOBIIPRP-test`,
then calls `FindFirstFileA("\\.\pipe\*")`. On Ubuntu's Wine 9.0 and on
unpatched Wine master it prints:

```text
create pipe: ok (error 0)
FindFirstFileA failed: error 66
WaitNamedPipe exists check: 1 (error 66)
```

Error 66 is `ERROR_BAD_DEV_TYPE`, the Win32 form of `STATUS_BAD_DEVICE_TYPE`.
(`WaitNamedPipeA` succeeds; the error code on that line is stale.)

## Where it fails

Wine master at commit `081042ede6a165a36bf9988350dd584b440d7a3d`
(2026-10-12, "cmd: Fix quoted redirection targets in pipes."):

1. `FindFirstFileExW` in `dlls/kernelbase/file.c` converts the path to
   `\??\pipe\*`, splits off the mask `*` and opens `\??\pipe\` with
   `NtOpenFile(FILE_LIST_DIRECTORY | SYNCHRONIZE, FILE_DIRECTORY_FILE | ...)`.
2. `\??\pipe` is a symbolic link to `\Device\NamedPipe`. The trailing
   backslash leaves an empty name, and `named_pipe_device_lookup_name` in
   `server/named_pipe.c` returns the root directory object
   (`named_pipe_dir_ops`). `named_pipe_dir_open_file` gives it a pseudo fd
   from `alloc_pseudo_fd`. The open succeeds.
3. `FindFirstFileExW` calls `NtQueryDirectoryFile(..., FileBothDirectoryInformation,
   FALSE, mask, TRUE)`.
4. `NtQueryDirectoryFile` in `dlls/ntdll/unix/file.c` only knows Unix
   directories. It calls `server_get_unix_fd`, which sends `get_handle_fd`.
   `get_unix_fd` in `server/fd.c` fails on a pseudo fd with its
   `no_fd_status`, which `alloc_pseudo_fd` sets to `STATUS_BAD_DEVICE_TYPE`.
5. `NtQueryDirectoryFile` returns that status, and `FindFirstFileExW` maps it
   to `ERROR_BAD_DEV_TYPE`.

Nothing in the server can produce a listing either: the pipe directory object
has no request or fd operation that walks the pipe namespace
(`named_pipe_device.pipes`).

Wine 9.0 fails at the same `NtQueryDirectoryFile` step, through a different
object. The root directory object was added later, in commit 663bc96653
("server: Allow creating named pipes using \Device\NamedPipe\ as
RootDirectory", https://gitlab.winehq.org/wine/wine/-/commit/663bc96653),
first released in Wine 9.21 (absent from 9.20). In 9.0,
`\Device\NamedPipe\` opens the device itself (`named_pipe_device_file_ops`),
which also has a pseudo fd.

## Windows behaviour this aims for

Sources:

- Sysinternals PipeList,
  https://learn.microsoft.com/en-us/sysinternals/downloads/pipelist : NPFS is
  a file system driver, `NtQueryDirectoryFile` on the pipe root lists the
  pipes, and "the directory listing NPFS returns also indicates the maximum
  number of pipe instances set for each pipe and the number of active
  instances".
- `ZwQueryDirectoryFile`,
  https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/nf-ntifs-zwquerydirectoryfile :
  the search expression is captured on the first call for a handle and later
  masks are ignored; the first call acts as a restart; `ReturnSingleEntry`
  returns one entry; entries that do not fit wait for the next call, except
  that the first entry of a first call is returned truncated with
  `STATUS_BUFFER_OVERFLOW`; the last call reports `STATUS_NO_MORE_FILES`;
  fields the file system does not support are zero.
- `FILE_DIRECTORY_INFORMATION`,
  https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/ns-ntifs-_file_directory_information .

Microsoft does not document which fields carry the instance counts. The patch
follows the common reading of PipeList's output: `EndOfFile` holds the current
number of instances and `AllocationSize` the maximum, with
`PIPE_UNLIMITED_INSTANCES` reported as -1. `FileAttributes` is
`FILE_ATTRIBUTE_NORMAL`; time stamps, short names and file IDs are zero. None
of these values could be checked on Windows for this work (see Limitations).

One entry is returned per pipe name. Every instance of a pipe shares one Wine
server object, whatever process created it, so a pipe with several instances
appears once with `EndOfFile` equal to the instance count.

## Design

The patch is `patches/wine/0001-ntdll-Implement-NtQueryDirectoryFile-on-the-named-pi.patch`
(one commit, `git format-patch` output). It touches the server, ntdll and the
ntdll pipe tests.

### Server request

`server/protocol.def` gains `struct named_pipe_entry` (current instances,
maximum instances, name length, then the name) and one request:

```text
@REQ(get_named_pipe_entries)
    obj_handle_t   handle;       /* handle to the named pipe root directory */
    int            restart;      /* restart the scan at the first pipe */
    unsigned int   skip;         /* number of entries to move past before reading */
    VARARG(mask,unicode_str);    /* search expression, captured on the first scan */
@REPLY
    int            first;        /* whether this request started the first scan */
    data_size_t    mask_len;     /* length of the captured search expression */
    VARARG(mask,unicode_str,mask_len); /* captured search expression */
    VARARG(entries,named_pipe_entries); /* entries following the scan position */
@END
```

The handler in `server/named_pipe.c` accepts only a handle to the root
directory object (`named_pipe_dir_ops`) opened with `FILE_LIST_DIRECTORY`.
The scan state lives in that file object, as NPFS keeps it per file object on
Windows:

- `scanned`, `index`: whether a scan has started and the position of the next
  entry. The first request on a handle behaves as a restart.
- `mask`, `mask_len`: the search expression captured by the first request.
  A restart with a new non-empty expression replaces it, and a restart
  without one keeps it, which matches ntdll's existing directory cache for
  Unix directories.

The handler moves the position by `skip`, then returns the captured mask and
as many entries from the position as fit in the reply, without consuming
them. Entries come from `find_object_index` on the device's pipe namespace,
the same helper `get_directory_entries` uses. A pipe leaves that namespace
when its last server instance closes (`pipe_server_destroy` unlinks it), so
the listing shows pipes that have at least one server instance.

`server/trace.c` gets a dumper for the new variable-size type. The generated
files (`include/wine/server_protocol.h`, `server/request_handlers.h`,
`server/request_trace.h`) were regenerated with `tools/make_requests`, which
raised `SERVER_PROTOCOL_VERSION` from 963 to 964.

### ntdll

`NtQueryDirectoryFile` in `dlls/ntdll/unix/file.c` still tries
`server_get_unix_fd` first. When that fails with `STATUS_BAD_DEVICE_TYPE` (a
pseudo fd), it calls the new `query_named_pipe_directory`. If the server
answers `STATUS_OBJECT_TYPE_MISMATCH` (the handle is some other pseudo-fd
object), the original `STATUS_BAD_DEVICE_TYPE` is returned, so other devices
behave as before.

`query_named_pipe_directory`:

- sends the caller's mask and `restart_scan` with the first request only;
- filters entries with ntdll's existing `match_filename` against the
  captured mask, so `*`, `?` and the DOS wildcards `<`, `>`, `"` work as for
  disk directories;
- fills every class `NtQueryDirectoryFile` already accepts
  (`FileDirectoryInformation`, `FileFullDirectoryInformation`,
  `FileBothDirectoryInformation`, the `FileId*` classes and
  `FileNamesInformation`), using the same alignment and truncation rules as
  `get_dir_data_entry`;
- stops after one entry for `ReturnSingleEntry`, at the first entry that does
  not fit, or after a truncated first entry (`STATUS_BUFFER_OVERFLOW`);
- reports consumed entries, including skipped non-matching ones, through
  `skip` in the next request. That request also fetches the following batch,
  so a call that fits every pipe takes two server round trips, and one that
  stops early sends a final request with no reply buffer.
- returns `STATUS_NO_SUCH_FILE` when the very first scan of a handle finds
  nothing and `STATUS_NO_MORE_FILES` afterwards, as the Unix directory path
  does. `FindFirstFile` turns these into `ERROR_FILE_NOT_FOUND` and
  `ERROR_NO_MORE_FILES`.

No change was needed in kernelbase: once `NtQueryDirectoryFile` works on the
handle, `FindFirstFileExW` and `FindNextFileW` list the pipes.

### Test

`test_pipe_directory` in `dlls/ntdll/tests/pipe.c` creates two pipes with
process-unique names (one with one instance, one with two, both with a
maximum of three), opens `\Device\NamedPipe\` and checks:

- `FileDirectoryInformation`, `FileBothDirectoryInformation` and
  `FileFullDirectoryInformation` list both pipes once, with `EndOfFile` equal
  to the instance count and `AllocationSize` equal to 3, followed by
  `STATUS_NO_MORE_FILES`;
- `ReturnSingleEntry` returns one entry per call, then `STATUS_NO_MORE_FILES`;
- a fresh handle with a mask that matches nothing returns
  `STATUS_NO_SUCH_FILE` (`STATUS_NO_MORE_FILES` accepted as `broken()`,
  because NPFS's status here is not documented);
- `FindFirstFileW`/`FindNextFileW` on `\\.\pipe\<prefix>*` find both pipes
  and end with `ERROR_NO_MORE_FILES`, and the same search fails with
  `ERROR_FILE_NOT_FOUND` once the pipes are closed.

## Results

All runs were on albedo's WSL2 Ubuntu 24.04, 64-bit builds configured with
`--enable-win64 --without-x --without-freetype`.

| Wine | `pipelist.exe` | ntdll `pipe` tests | kernel32 `pipe` tests | Discovery, both client DLLs |
| --- | --- | --- | --- | --- |
| 9.0 (Ubuntu package), unpatched | error 66 | | | no device found |
| master `081042ed` + patch | lists `TOBIIPRP-test` | 2451, 0 failures | | `tobii-prp://ET5-LINUX`, about 100 head-pose samples per 3 s |
| 11.0 + patch (`apply.sh`) | lists `TOBIIPRP-test` | 2451, 0 failures | 38473, 0 failures | `tobii-prp://ET5-LINUX`, about 100 head-pose samples per 3 s |

Discovery was checked with `tools/prp_bridge/run_wine_test.sh` and
`PRP_URL=discover`: the test client calls the DLL's own
`tobii_enumerate_local_device_urls` (the game DLL's copy at RVA `0xcd20`),
then connects to the first `tobii-prp://` URL it reports. The client DLLs were
Tobii's `tobii_stream_engine.dll` and Star Citizen's
`tobii_gameintegration_x64.dll`; see [prp-bridge.md](prp-bridge.md) for the
service and bridge.

`apply.sh` applies without rejects to clean 10.0, 10.20, 11.0 and 11.19 trees.
Only 11.0 and master were built and run.

## Applying it to a Wine runner

```sh
patches/wine/apply.sh path/to/wine-source
```

The script refuses trees without the named pipe root directory object (Wine
9.20 and older), drops the hunks for the three generated protocol files,
applies the rest with `patch --fuzz=2` and runs `tools/make_requests` to
regenerate them. Build Wine as usual afterwards. The patched ntdll and
wineserver change the server protocol together, so both must come from the
same build; a game runner that ships its own wineserver needs the patched one.

## Limitations

- Field values for instance counts follow PipeList's documented output, not a
  Windows trace; `NtQueryDirectoryFile` on `\Device\NamedPipe\` was not run on
  Windows for this work.
- Wine 10.0, 10.20 and 11.19 were only checked for a clean apply, not built.
- Proton and runners derived from wine-staging or wine-tkg were not tried.
- Star Citizen itself was not run; the game DLL was driven by the test client.
