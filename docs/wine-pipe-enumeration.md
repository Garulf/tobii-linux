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
patches/wine/apply.sh --emit [path/to/wine-source] > tobii-pipe-listing.mypatch
```

The script picks one of three variants from the tree:

| Variant | Tree | Selected when |
| --- | --- | --- |
| `0001-...patch` | upstream Wine 10.0 or later, wine-staging, wine-tkg | default |
| `proton/0001-...patch` | Valve's Proton 10 and 11, GE-Proton | `dlls/ntdll/unix/file.c` has Proton's `server_query_directory_file` |
| `proton-9.0/0001-...patch` | Proton 9 (Wine 9.0) | Proton tree without the pipe root directory object |

It refuses other trees without the named pipe root directory object (Wine
9.20 and older), drops the hunks for the three generated protocol files,
applies the rest with `patch --fuzz=2`, checks that the hunks that fuzz could
misplace landed in the right functions, and runs `tools/make_requests` to
regenerate the protocol files. Build Wine as usual afterwards. The patched
ntdll and wineserver change the server protocol together, so both must come
from the same build; a game runner that ships its own wineserver needs the
patched one.

`--emit` prints the selected variant without the generated files, for build
systems that apply user patches before running `tools/make_requests`
themselves; wine-tkg does so for `*.mypatch` files in `wine-tkg-userpatches/`
(`wine-tkg-scripts/prepare.sh`, user patches before `tools/make_requests`).

### Proton's own pipe listing

Valve's Wine already answers `NtQueryDirectoryFile` on `\Device\NamedPipe\`,
since Proton 9 (commit 55436bd6d3, "ntdll: Partially support querying
directory entries for '\\Device\\NamedPipe\\'",
https://github.com/ValveSoftware/wine/commit/55436bd6d374a3f70781bd45458aff5bde6cafb5).
It returns one name per call and only for `FileDirectoryInformation`; any
other class gets `STATUS_NOT_SUPPORTED`. `FindFirstFileExW` asks for
`FileBothDirectoryInformation` with several entries per call, so the listing
still fails: `pipelist.exe` under stock GE-Proton 11-9 printed
`FindFirstFileA failed: error 50` (`ERROR_NOT_SUPPORTED`). The Proton variants
answer every class through the new request first and keep Valve's request as a
fallback; Valve's own `test_pipe_directory_listing` expectations (access
check, length check, single entries, `STATUS_NO_MORE_FILES`) hold for the new
code as well.

## Runner sources

Sources were fetched on 2026-10-14 and removed after the checks; the `apply.sh`
output is `evidence/private/wine-forks/apply-matrix.txt`. Builds ran on albedo's WSL2
Ubuntu 24.04 with `--without-x --without-freetype`
(`evidence/private/launch-work/`).

| Runner source | Commit | Variant | `apply.sh` | Built and tested |
| --- | --- | --- | --- | --- |
| Wine 11.0 (nixpkgs `wineWow64Packages.stable`) | `wine-11.0` | upstream | applies, one hunk with fuzz 1 | earlier, see Results |
| Wine 11.19 (nixpkgs `unstable`) | `wine-11.19` | upstream | applies | Nix patch phase only |
| wine-staging 11.19 on 11.19 (nixpkgs `staging`) | staging `v11.19` | upstream | applies after all staging patchsets | Nix patch phase only |
| LUG runner, `lug-wine` default preset, wine-tkg, Wine 11.19 | lug-wine `42f45c8e`, wine-tkg `ca3f1a3c`, LUG patches `01a904b6` | upstream, as a `.mypatch` (`--emit`) | applied by wine-tkg with the 20 LUG patches | compiled, see below |
| Proton 11 (`proton_11.0`) | `e60892cc` | proton | applies without fuzz | yes, see below |
| Proton 10 (`proton_10.0`) | `b8fdff8e` | proton | applies without fuzz | no |
| GE-Proton11-9 (its Wine after `protonprep-valve-staging.sh`) | wine `981e2f0b`, staging `6cc805ea` | proton | applies without fuzz | no |
| Proton 9 (`proton_9.0`) | `015230dc` | proton-9.0 | applies without fuzz | not completed |
| Proton 8 (`proton_8.0`), Lutris `wine-ge-8` | | | refused (no pipe root directory, no Valve listing) | |
| Ubuntu Wine 9.0, upstream before 9.21 | | | refused | |

Builds of the patched trees (Valve's 11.0 tree with `--enable-win64
--without-ffmpeg`; its `winedmo` does not build against Ubuntu's FFmpeg):

| Build | ntdll `pipe` tests | kernel32 `pipe` tests | `pipelist.exe` | Discovery, `PRP_URL=discover` |
| --- | --- | --- | --- | --- |
| Proton 11 + proton variant | 2489, 0 failures (Valve's `test_pipe_directory_listing` and ours) | 38466, 0 failures | lists `TOBIIPRP-test` and 11 system pipes | game DLL: `tobii-prp://ET5-LINUX`, 97 head-pose and gaze samples per 3 s |

The LUG tree (wine-tkg with the 20 LUG patches and ours as a `.mypatch`)
applied and compiled as a 64-bit-only build, but Wine 11.19 starts programs
through the 32-bit loader by default, so that build could not run anything; a
build with both PE architectures was still running when the test host went
offline. The Proton 9 build was in progress at the same time.

GE-Proton and Valve's Proton were not built as complete Proton packages; their
build system runs in Valve's SDK container. The Proton builds above are the
patched Valve Wine trees on their own, after the steps Proton's makefile runs
first (`make_vulkan`, `make_specfiles`, `autoreconf`).

nixpkgs (`7fd4004d`): `nix/wine.nix` overrides `wineWow64Packages.{stable,
staging,unstable}` with `postPatch = "patchShebangs tools/make_requests; bash
.../apply.sh ."`; staging's own patches run in `prePatch`, so ours apply on
top of them. The three derivations evaluate, and `patchedSourceCheck.*`
(unpack and patch phases only) built for all three. A full Wine build through
the overlay was not run.

## Without a Wine patch

Star Citizen uses Easy Anti-Cheat, so nothing may be loaded into or patched in
the game process. Under that constraint:

- **Stock Wine, wine-staging, wine-tkg**: nothing answers
  `NtQueryDirectoryFile` on the pipe root (no staging patchset touches it), and
  the call fails inside the game process (ntdll, kernelbase) on a handle whose
  type the wineserver decides. A DLL override changes only processes that load
  the DLL, and the listing happens in the game process, so no override outside
  it can make the call succeed.
- **Stock Proton and GE-Proton**: Valve's partial listing does not cover
  `FindFirstFile` (above).
- **Redirecting `\??\pipe`**: the symbolic link that `FindFirstFile` follows
  is the same one `CreateFile` and `WaitNamedPipe` use for the pipes
  themselves, so pointing it at a listable directory would break the
  connection the listing is meant to find.
- **A wineserver-only change**: `NtQueryDirectoryFile` lists Unix directories
  through the descriptor `get_handle_fd` returns. A wineserver that handed out
  a real directory mirroring the pipe names for the pipe root would be listed
  by an unmodified ntdll, and without a protocol change such a wineserver could
  replace the one of an existing runner of the same version. This is still a
  Wine modification and was not tried.
- **The TCP discovery route**: the game DLL's enumeration also connects to
  `127.0.0.1:4455` and reports `tet-tcp://127.0.0.1` for a listener there
  (live, stock Ubuntu Wine 9.0, both client DLLs). One connection carried a TTP
  request, kind `0x51`, command 1000 (protocol upgrade), in the framing
  `src/tobii_linux/ttp.py` decodes. `device_create` opens such URLs with the
  in-process platform module instead of PRP
  ([service-transport.md](service-transport.md#discovery)). That module gets
  head pose from the fixed-name pipe `streamengineservices`, which needs no
  listing; `cli serve` serves the route with the unchanged bridge
  (emulated under unpatched Wine 8.0, not yet live: [tet-tcp.md](tet-tcp.md),
  [sesp.md](sesp.md)).
- **`\\.\pipe\ETDefaultPIPE`**: the default enumeration never called it
  (no request reached a listener on that pipe in the same run).

Evidence: `evidence/private/launch-work/test-nopatch-1.txt`,
`evidence/private/launch-work/nopatch/`.

## Limitations

- Field values for instance counts follow PipeList's documented output, not a
  Windows trace; `NtQueryDirectoryFile` on `\Device\NamedPipe\` was not run on
  Windows for this work.
- Wine 10.0, 10.20 and 11.19 upstream were only checked for a clean apply, not
  built; Proton 10 and GE-Proton only for a clean apply.
- Star Citizen itself was not run; the game DLL was driven by the test client.
