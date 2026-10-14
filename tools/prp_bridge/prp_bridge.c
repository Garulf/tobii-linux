/*
 * Wine-side PRP pipe relay for the Linux PRP service (src/tobii_linux/prp_service.py).
 *
 * Owns the named pipes the game DLL's PRP client opens, opens each client's
 * reply pipe when the service names it, and relays every pipe message over one
 * loopback TCP connection per pipe connection. Relay framing: docs/prp-bridge.md.
 *
 * Usage: prp_bridge.exe [--until-prefix-exits] [host [port]] [-- program [arguments...]]
 *
 * host and port default to 127.0.0.1 7760. Without options the bridge exits when
 * the service closes its control connection. --until-prefix-exits makes it a Wine
 * system process instead: it reconnects when the service restarts and exits once
 * the last ordinary process of the prefix has exited, as Proton's steam.exe does.
 * Without a service it goes on after FIRST_ATTACH_TIMEOUT_MS and keeps trying.
 * "-- program" implies that mode and starts the program once the pipes are up
 * or that time has passed; a Unix program path is converted to a DOS path.
 */
#define WIN32_LEAN_AND_MEAN
#include <winsock2.h>
#include <ws2tcpip.h>
#include <windows.h>
#include <shellapi.h>
#include <devpropdef.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define RELAY_VERSION 1
#define FRAME_BRIDGE_HELLO 1
#define FRAME_PIPES 2
#define FRAME_PIPE_OPEN 3
#define FRAME_REPLY_PIPE 4
#define FRAME_MESSAGE 5
#define FRAME_DISPLAY_ID 6
#define MAX_FRAME (1u << 20)
#define MAX_PIPES 16
#define PIPE_NAME_LIMIT 256
#define SERVER_PIPE_IN_BUFFER 0x10000
#define PIPE_DEFAULT_TIMEOUT_MS 5000
#define CONNECT_ATTEMPTS 50
#define REPLY_OPEN_ATTEMPTS 4
#define FIRST_ATTACH_TIMEOUT_MS 10000
#define WINE_MAKE_PROCESS_SYSTEM 1000

static const char *service_host = "127.0.0.1";
static const char *service_port = "7760";
static CRITICAL_SECTION log_lock;
static FILE *log_file;
static HANDLE first_serving;
static HANDLE display_id_known;
static char display_id[512];

typedef struct {
    char name[PIPE_NAME_LIMIT];
    HANDLE stop;
} Listener;

typedef struct {
    SOCKET control;
    HANDLE stop;
} DisplayReport;

typedef struct {
    HANDLE pipe;
    HANDLE reply;
    SOCKET socket;
    HANDLE stop;
    char name[PIPE_NAME_LIMIT];
} Session;

static void write_log_line(FILE *stream, const SYSTEMTIME *now, const char *format, va_list args)
{
    fprintf(stream, "%02u:%02u:%02u.%03u ", now->wHour, now->wMinute, now->wSecond,
            now->wMilliseconds);
    vfprintf(stream, format, args);
    fputc('\n', stream);
    fflush(stream);
}

/* Logs to stderr and, when TOBII_LINUX_BRIDGE_LOG names a file, to that file. */
static void say(const char *format, ...)
{
    va_list args;
    SYSTEMTIME now;
    GetLocalTime(&now);
    EnterCriticalSection(&log_lock);
    va_start(args, format);
    write_log_line(stderr, &now, format, args);
    va_end(args);
    if (log_file) {
        va_start(args, format);
        write_log_line(log_file, &now, format, args);
        va_end(args);
    }
    LeaveCriticalSection(&log_lock);
}

static int send_all(SOCKET socket, const void *data, size_t size)
{
    const char *cursor = data;
    while (size > 0) {
        int sent = send(socket, cursor, (int)(size > 0x10000 ? 0x10000 : size), 0);
        if (sent <= 0)
            return 0;
        cursor += sent;
        size -= (size_t)sent;
    }
    return 1;
}

static int recv_all(SOCKET socket, void *data, size_t size)
{
    char *cursor = data;
    while (size > 0) {
        int got = recv(socket, cursor, (int)(size > 0x10000 ? 0x10000 : size), 0);
        if (got <= 0)
            return 0;
        cursor += got;
        size -= (size_t)got;
    }
    return 1;
}

static int send_frame(SOCKET socket, UINT32 kind, const void *payload, UINT32 size)
{
    UINT32 header[2] = {kind, size};
    return send_all(socket, header, sizeof header) && send_all(socket, payload, size);
}

/* Returns a malloc'd payload (NUL-terminated for convenience) or NULL on EOF or error. */
static char *recv_frame(SOCKET socket, UINT32 *kind, UINT32 *size)
{
    UINT32 header[2];
    char *payload;
    if (!recv_all(socket, header, sizeof header) || header[1] > MAX_FRAME)
        return NULL;
    payload = malloc(header[1] + 1);
    if (!payload)
        return NULL;
    if (!recv_all(socket, payload, header[1])) {
        free(payload);
        return NULL;
    }
    payload[header[1]] = 0;
    *kind = header[0];
    *size = header[1];
    return payload;
}

static SOCKET connect_service(void)
{
    struct addrinfo hints = {0}, *found = NULL, *each;
    SOCKET result = INVALID_SOCKET;
    BOOL no_delay = TRUE;
    hints.ai_family = AF_UNSPEC;
    hints.ai_socktype = SOCK_STREAM;
    hints.ai_protocol = IPPROTO_TCP;
    if (getaddrinfo(service_host, service_port, &hints, &found) != 0)
        return INVALID_SOCKET;
    for (each = found; each && result == INVALID_SOCKET; each = each->ai_next) {
        result = socket(each->ai_family, each->ai_socktype, each->ai_protocol);
        if (result == INVALID_SOCKET)
            continue;
        if (connect(result, each->ai_addr, (int)each->ai_addrlen) != 0) {
            closesocket(result);
            result = INVALID_SOCKET;
        }
    }
    freeaddrinfo(found);
    if (result != INVALID_SOCKET)
        setsockopt(result, IPPROTO_TCP, TCP_NODELAY, (const char *)&no_delay, sizeof no_delay);
    return result;
}

static void pipe_path(char *path, size_t size, const char *name)
{
    snprintf(path, size, "\\\\.\\pipe\\%s", name);
}

/* Opens the client's inbound reply pipe as the service does: write access, message-read mode. */
static HANDLE open_reply_pipe(const char *name)
{
    char path[PIPE_NAME_LIMIT + 16];
    DWORD mode = PIPE_READMODE_MESSAGE;
    int attempt;
    pipe_path(path, sizeof path, name);
    for (attempt = 0; attempt < REPLY_OPEN_ATTEMPTS; attempt++) {
        HANDLE reply = CreateFileA(path, GENERIC_WRITE, 0, NULL, OPEN_EXISTING, 0, NULL);
        if (reply != INVALID_HANDLE_VALUE) {
            if (SetNamedPipeHandleState(reply, &mode, NULL, NULL))
                return reply;
            say("SetNamedPipeHandleState(%s) failed: %lu", name, GetLastError());
            CloseHandle(reply);
            return INVALID_HANDLE_VALUE;
        }
        if (GetLastError() != ERROR_PIPE_BUSY) {
            say("open reply pipe %s failed: %lu", name, GetLastError());
            return INVALID_HANDLE_VALUE;
        }
        WaitNamedPipeA(path, 1000);
    }
    return INVALID_HANDLE_VALUE;
}

/* Service to client: opens the reply pipe on request and writes each message to it. */
static DWORD WINAPI downstream(void *context)
{
    Session *session = context;
    UINT32 kind, size;
    char *payload;
    while ((payload = recv_frame(session->socket, &kind, &size)) != NULL) {
        int ok = 1;
        if (kind == FRAME_REPLY_PIPE && session->reply == INVALID_HANDLE_VALUE) {
            session->reply = open_reply_pipe(payload);
            ok = session->reply != INVALID_HANDLE_VALUE;
            if (ok)
                say("%s: opened reply pipe %s", session->name, payload);
        } else if (kind == FRAME_MESSAGE && session->reply != INVALID_HANDLE_VALUE) {
            DWORD written;
            ok = WriteFile(session->reply, payload, size, &written, NULL) && written == size;
            if (!ok)
                say("%s: reply write failed: %lu", session->name, GetLastError());
        } else {
            say("%s: unexpected relay frame %u", session->name, kind);
            ok = 0;
        }
        free(payload);
        if (!ok)
            break;
    }
    SetEvent(session->stop);
    return 0;
}

/* Reads one whole pipe message; returns its length, or -1 when the session must end. */
static long read_message(Session *session, char **buffer, DWORD *capacity)
{
    OVERLAPPED overlapped = {0};
    HANDLE events[2];
    DWORD used = 0;
    long result = -1;
    overlapped.hEvent = CreateEventA(NULL, TRUE, FALSE, NULL);
    events[0] = overlapped.hEvent;
    events[1] = session->stop;
    for (;;) {
        DWORD got = 0, error = 0;
        if (*capacity - used < 0x1000) {
            char *grown = realloc(*buffer, *capacity * 2);
            if (!grown)
                break;
            *buffer = grown;
            *capacity *= 2;
        }
        ResetEvent(overlapped.hEvent);
        if (!ReadFile(session->pipe, *buffer + used, *capacity - used, NULL, &overlapped)) {
            error = GetLastError();
            if (error != ERROR_IO_PENDING && error != ERROR_MORE_DATA)
                break;
        }
        if (WaitForMultipleObjects(2, events, FALSE, INFINITE) != WAIT_OBJECT_0) {
            CancelIoEx(session->pipe, &overlapped);
            GetOverlappedResult(session->pipe, &overlapped, &got, TRUE);
            break;
        }
        error = GetOverlappedResult(session->pipe, &overlapped, &got, FALSE) ? 0 : GetLastError();
        used += got;
        if (error == 0) {
            result = (long)used;
            break;
        }
        if (error != ERROR_MORE_DATA)
            break;
        if (used > MAX_FRAME)
            break;
    }
    CloseHandle(overlapped.hEvent);
    return result;
}

static DWORD WINAPI serve_session(void *context)
{
    Session *session = context;
    ULONG client_pid = 0;
    DWORD capacity = 0x10000;
    char *buffer = malloc(capacity);
    char open_payload[8 + PIPE_NAME_LIMIT];
    size_t name_size = strlen(session->name) + 1;
    UINT32 header[2] = {RELAY_VERSION, 0};
    HANDLE relay = NULL;
    long size;

    GetNamedPipeClientProcessId(session->pipe, &client_pid);
    session->socket = connect_service();
    if (session->socket == INVALID_SOCKET || !buffer) {
        say("%s: cannot reach the PRP service at %s:%s", session->name, service_host,
            service_port);
        goto done;
    }
    header[1] = (UINT32)client_pid;
    memcpy(open_payload, header, sizeof header);
    memcpy(open_payload + 8, session->name, name_size);
    if (!send_frame(session->socket, FRAME_PIPE_OPEN, open_payload, (UINT32)(8 + name_size)))
        goto done;
    say("%s: client pid %lu connected", session->name, client_pid);
    relay = CreateThread(NULL, 0, downstream, session, 0, NULL);
    while ((size = read_message(session, &buffer, &capacity)) >= 0) {
        if (!send_frame(session->socket, FRAME_MESSAGE, buffer, (UINT32)size))
            break;
    }
done:
    SetEvent(session->stop);
    if (session->socket != INVALID_SOCKET)
        shutdown(session->socket, SD_BOTH);
    if (relay) {
        WaitForSingleObject(relay, INFINITE);
        CloseHandle(relay);
    }
    if (session->socket != INVALID_SOCKET)
        closesocket(session->socket);
    if (session->reply != INVALID_HANDLE_VALUE)
        CloseHandle(session->reply);
    DisconnectNamedPipe(session->pipe);
    CloseHandle(session->pipe);
    CloseHandle(session->stop);
    say("%s: session closed", session->name);
    free(buffer);
    free(session);
    return 0;
}

/* Waits for a client on an overlapped server pipe; returns the Win32 error, or ERROR_CANCELLED on stop. */
static DWORD await_client(HANDLE pipe, HANDLE stop)
{
    OVERLAPPED overlapped = {0};
    HANDLE events[2];
    DWORD error = 0, ignored;
    overlapped.hEvent = CreateEventA(NULL, TRUE, FALSE, NULL);
    events[0] = overlapped.hEvent;
    events[1] = stop;
    if (!ConnectNamedPipe(pipe, &overlapped)) {
        error = GetLastError();
        if (error == ERROR_PIPE_CONNECTED) {
            error = 0;
        } else if (error == ERROR_IO_PENDING) {
            if (WaitForMultipleObjects(2, events, FALSE, INFINITE) != WAIT_OBJECT_0) {
                CancelIoEx(pipe, &overlapped);
                GetOverlappedResult(pipe, &overlapped, &ignored, TRUE);
                error = ERROR_CANCELLED;
            } else {
                error = GetOverlappedResult(pipe, &overlapped, &ignored, FALSE) ? 0 : GetLastError();
            }
        }
    }
    CloseHandle(overlapped.hEvent);
    return error;
}

/* Server pipe as the DLL's own service code creates it (0x1800aae50): inbound, message type. */
static DWORD WINAPI listen_pipe(void *context)
{
    const Listener *listener = context;
    const char *name = listener->name;
    char path[PIPE_NAME_LIMIT + 16];
    pipe_path(path, sizeof path, name);
    while (WaitForSingleObject(listener->stop, 0) == WAIT_TIMEOUT) {
        Session *session;
        DWORD error;
        HANDLE pipe = CreateNamedPipeA(path, PIPE_ACCESS_INBOUND | FILE_FLAG_OVERLAPPED,
                                       PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT,
                                       PIPE_UNLIMITED_INSTANCES, 0, SERVER_PIPE_IN_BUFFER,
                                       PIPE_DEFAULT_TIMEOUT_MS, NULL);
        if (pipe == INVALID_HANDLE_VALUE) {
            say("CreateNamedPipeA(%s) failed: %lu", path, GetLastError());
            WaitForSingleObject(listener->stop, 1000);
            continue;
        }
        error = await_client(pipe, listener->stop);
        if (error != 0) {
            if (error != ERROR_CANCELLED)
                say("ConnectNamedPipe(%s) failed: %lu", name, error);
            CloseHandle(pipe);
            continue;
        }
        session = calloc(1, sizeof *session);
        if (!session) {
            CloseHandle(pipe);
            continue;
        }
        session->pipe = pipe;
        session->reply = INVALID_HANDLE_VALUE;
        session->socket = INVALID_SOCKET;
        session->stop = CreateEventA(NULL, TRUE, FALSE, NULL);
        snprintf(session->name, sizeof session->name, "%s", name);
        CloseHandle(CreateThread(NULL, 0, serve_session, session, 0, NULL));
    }
    return 0;
}

typedef DWORD (WINAPI *GetInterfaceProperty)(const WCHAR *, const DEVPROPKEY *, DEVPROPTYPE *,
                                             BYTE *, ULONG *, ULONG);

static const DEVPROPKEY device_instance_id = {
    {0x78c34fc8, 0x104a, 0x4aca, {0x9e, 0xa4, 0x52, 0x4d, 0x52, 0x99, 0x6e, 0x57}}, 256};

/* Wine before 10.0 aborts the process in CM_Get_Device_Interface_PropertyW. */
static int has_device_interface_properties(void)
{
    typedef const char *(CDECL *WineGetVersion)(void);
    FARPROC function = GetProcAddress(GetModuleHandleA("ntdll.dll"), "wine_get_version");
    WineGetVersion wine_get_version;
    if (!function)
        return 1;
    memcpy(&wine_get_version, &function, sizeof wine_get_version);
    return atoi(wine_get_version()) >= 10;
}

static int primary_monitor_instance_id(WCHAR *instance_id, ULONG size)
{
    DISPLAY_DEVICEW adapter, monitor;
    HMODULE cfgmgr = LoadLibraryA("cfgmgr32.dll");
    FARPROC function = cfgmgr ? GetProcAddress(cfgmgr, "CM_Get_Device_Interface_PropertyW") : NULL;
    GetInterfaceProperty get_property;
    DEVPROPTYPE type;
    DWORD index;
    if (!function || !has_device_interface_properties())
        return 0;
    adapter.cb = sizeof adapter;
    monitor.cb = sizeof monitor;
    memcpy(&get_property, &function, sizeof get_property);
    for (index = 0; EnumDisplayDevicesW(NULL, index, &adapter, 0); index++) {
        if ((adapter.StateFlags & DISPLAY_DEVICE_PRIMARY_DEVICE)
            && EnumDisplayDevicesW(adapter.DeviceName, 0, &monitor, EDD_GET_DEVICE_INTERFACE_NAME))
            return get_property(monitor.DeviceID, &device_instance_id, &type,
                                (BYTE *)instance_id, &size, 0) == 0;
        adapter.cb = sizeof adapter;
    }
    return 0;
}

/* Display enumeration can take seconds under a host display server, so it runs
   once in the background and never delays the pipes. */
static DWORD WINAPI find_display_id(void *unused)
{
    WCHAR instance_id[256];
    (void)unused;
    if (!primary_monitor_instance_id(instance_id, sizeof instance_id)
        || WideCharToMultiByte(CP_UTF8, 0, instance_id, -1, display_id, sizeof display_id,
                               NULL, NULL) <= 1) {
        display_id[0] = 0;
        say("primary monitor id unavailable; the service needs --display-id for window tracking");
    }
    SetEvent(display_id_known);
    return 0;
}

static DWORD WINAPI report_display_id(void *context)
{
    DisplayReport *report = context;
    HANDLE waits[2] = {display_id_known, report->stop};
    if (WaitForMultipleObjects(2, waits, FALSE, INFINITE) == WAIT_OBJECT_0 && display_id[0]
        && send_frame(report->control, FRAME_DISPLAY_ID, display_id, (UINT32)strlen(display_id)))
        say("reported primary monitor %s", display_id);
    return 0;
}

static SOCKET attach_to_service(char **names_out, UINT32 *size_out)
{
    int attempt;
    for (attempt = 0; attempt < CONNECT_ATTEMPTS; attempt++) {
        SOCKET control = connect_service();
        if (control != INVALID_SOCKET) {
            UINT32 version = RELAY_VERSION, kind;
            if (send_frame(control, FRAME_BRIDGE_HELLO, &version, sizeof version)) {
                char *names = recv_frame(control, &kind, size_out);
                if (names && kind == FRAME_PIPES) {
                    *names_out = names;
                    return control;
                }
                free(names);
            }
            closesocket(control);
        }
        Sleep(200);
    }
    return INVALID_SOCKET;
}

/* Serves one set of pipes until the service closes the control connection. */
static void serve_pipes(SOCKET control, char *names, UINT32 size)
{
    Listener listeners[MAX_PIPES];
    HANDLE threads[MAX_PIPES + 1];
    HANDLE stop = CreateEventA(NULL, TRUE, FALSE, NULL);
    DisplayReport report = {control, stop};
    const char *cursor;
    char sink[16];
    int count = 0, index;

    for (cursor = names; cursor < names + size && *cursor && count < MAX_PIPES;
         cursor += strlen(cursor) + 1) {
        snprintf(listeners[count].name, sizeof listeners[count].name, "%s", cursor);
        listeners[count].stop = stop;
        threads[count] = CreateThread(NULL, 0, listen_pipe, &listeners[count], 0, NULL);
        if (threads[count]) {
            say("serving \\\\.\\pipe\\%s", cursor);
            count++;
        }
    }
    SetEvent(first_serving);
    threads[count] = CreateThread(NULL, 0, report_display_id, &report, 0, NULL);
    if (threads[count])
        count++;
    while (recv(control, sink, sizeof sink, 0) > 0)
        ;
    SetEvent(stop);
    WaitForMultipleObjects((DWORD)count, threads, TRUE, INFINITE);
    for (index = 0; index < count; index++)
        CloseHandle(threads[index]);
    CloseHandle(stop);
}

/* Persistent relay: attaches, serves, and attaches again whenever the service restarts. */
static DWORD WINAPI relay_forever(void *unused)
{
    int waiting_logged = 0;
    (void)unused;
    for (;;) {
        char *names = NULL;
        UINT32 size = 0;
        SOCKET control = attach_to_service(&names, &size);
        if (control == INVALID_SOCKET) {
            if (!waiting_logged)
                say("no PRP service at %s:%s yet; still trying", service_host, service_port);
            waiting_logged = 1;
            Sleep(1000);
            continue;
        }
        waiting_logged = 0;
        serve_pipes(control, names, size);
        closesocket(control);
        free(names);
        say("PRP service closed the control connection; reconnecting");
    }
    return 0;
}

/* Marks this process as a Wine system process; returns the event Wine signals when
   only system processes remain, or NULL outside Wine. */
static HANDLE become_wine_system_process(void)
{
    typedef LONG (NTAPI *SetInformationProcess)(HANDLE, ULONG, void *, ULONG);
    HMODULE ntdll = GetModuleHandleA("ntdll.dll");
    FARPROC function = GetProcAddress(ntdll, "NtSetInformationProcess");
    SetInformationProcess set_information;
    HANDLE prefix_idle = NULL;
    if (!function || !GetProcAddress(ntdll, "wine_get_version"))
        return NULL;
    memcpy(&set_information, &function, sizeof set_information);
    if (set_information(GetCurrentProcess(), WINE_MAKE_PROCESS_SYSTEM, &prefix_idle,
                        sizeof(HANDLE *)) != 0)
        return NULL;
    return prefix_idle;
}

/* Appends one argument with the quoting CommandLineToArgvW reverses. */
static void append_argument(WCHAR *line, size_t *used, const WCHAR *argument)
{
    size_t backslashes = 0;
    const WCHAR *each;
    line[(*used)++] = L'"';
    for (each = argument; *each; each++) {
        if (*each == L'\\') {
            backslashes++;
            continue;
        }
        if (*each == L'"')
            backslashes = backslashes * 2 + 1;
        while (backslashes) {
            line[(*used)++] = L'\\';
            backslashes--;
        }
        line[(*used)++] = *each;
    }
    for (backslashes *= 2; backslashes; backslashes--)
        line[(*used)++] = L'\\';
    line[(*used)++] = L'"';
    line[(*used)++] = L' ';
}

static WCHAR *dos_path_of_unix_path(const WCHAR *unix_path)
{
    typedef WCHAR *(CDECL *GetDosFileName)(const char *);
    FARPROC function = GetProcAddress(GetModuleHandleA("kernel32.dll"), "wine_get_dos_file_name");
    GetDosFileName get_dos_file_name;
    char utf8[4 * MAX_PATH];
    if (unix_path[0] != L'/' || !function
        || !WideCharToMultiByte(CP_UTF8, 0, unix_path, -1, utf8, sizeof utf8, NULL, NULL))
        return NULL;
    memcpy(&get_dos_file_name, &function, sizeof get_dos_file_name);
    return get_dos_file_name(utf8);
}

static WCHAR *program_command_line(WCHAR **arguments, int count)
{
    WCHAR *dos_program = dos_path_of_unix_path(arguments[0]);
    size_t capacity = 1, used = 0;
    WCHAR *line;
    int index;
    for (index = 0; index < count; index++)
        capacity += 3 + 2 * wcslen(index == 0 && dos_program ? dos_program : arguments[index]);
    line = malloc(capacity * sizeof *line);
    if (!line)
        return NULL;
    for (index = 0; index < count; index++)
        append_argument(line, &used, index == 0 && dos_program ? dos_program : arguments[index]);
    line[used ? used - 1 : 0] = 0;
    if (dos_program)
        HeapFree(GetProcessHeap(), 0, dos_program);
    return line;
}

static HANDLE start_program(WCHAR **arguments, int count)
{
    STARTUPINFOW startup;
    PROCESS_INFORMATION process;
    WCHAR *line = program_command_line(arguments, count);
    BOOL started;
    memset(&startup, 0, sizeof startup);
    startup.cb = sizeof startup;
    started = line && CreateProcessW(NULL, line, NULL, NULL, FALSE, 0, NULL, NULL,
                                          &startup, &process);
    if (!started) {
        say("cannot start %ls: %lu", arguments[0], GetLastError());
        free(line);
        return NULL;
    }
    say("started %ls", line);
    free(line);
    CloseHandle(process.hThread);
    return process.hProcess;
}

static int separator_index(WCHAR **arguments, int count)
{
    int index;
    for (index = 1; index < count; index++)
        if (!wcscmp(arguments[index], L"--"))
            return index;
    return count;
}

/* Runs the relay as a system process until the prefix's ordinary processes are gone. */
static int run_until_prefix_exits(WCHAR **program, int program_count)
{
    HANDLE relay = CreateThread(NULL, 0, relay_forever, NULL, 0, NULL);
    HANDLE child = NULL, prefix_idle, wait_for;
    DWORD exit_code = 0;
    if (!relay)
        return 1;
    if (WaitForSingleObject(first_serving, FIRST_ATTACH_TIMEOUT_MS) != WAIT_OBJECT_0)
        say("going on without head tracking until the service appears");
    if (program_count > 0) {
        child = start_program(program, program_count);
        if (!child)
            return 1;
    }
    prefix_idle = become_wine_system_process();
    wait_for = prefix_idle ? prefix_idle : child ? child : relay;
    WaitForSingleObject(wait_for, INFINITE);
    if (child && GetExitCodeProcess(child, &exit_code) && exit_code == STILL_ACTIVE)
        exit_code = 0;
    say("prefix has no running programs left; exiting");
    return (int)exit_code;
}

static void open_log_file(void)
{
    WCHAR path[MAX_PATH], *dos_path;
    if (GetEnvironmentVariableW(L"TOBII_LINUX_BRIDGE_LOG", path, MAX_PATH) - 1u >= MAX_PATH - 1u)
        return;
    dos_path = dos_path_of_unix_path(path);
    log_file = _wfopen(dos_path ? dos_path : path, L"a");
    if (dos_path)
        HeapFree(GetProcessHeap(), 0, dos_path);
}

int main(void)
{
    WSADATA wsa;
    int count = 0, separator, positional = 0, index;
    WCHAR **arguments = CommandLineToArgvW(GetCommandLineW(), &count);
    static char host[256], port[32];
    int persistent = 0;
    SOCKET control;
    char *names = NULL;
    UINT32 size = 0;

    InitializeCriticalSection(&log_lock);
    open_log_file();
    first_serving = CreateEventA(NULL, TRUE, FALSE, NULL);
    display_id_known = CreateEventA(NULL, TRUE, FALSE, NULL);
    CloseHandle(CreateThread(NULL, 0, find_display_id, NULL, 0, NULL));
    if (!arguments)
        return 1;
    separator = separator_index(arguments, count);
    for (index = 1; index < separator; index++) {
        if (!wcscmp(arguments[index], L"--until-prefix-exits")) {
            persistent = 1;
        } else if (positional == 0) {
            WideCharToMultiByte(CP_UTF8, 0, arguments[index], -1, host, sizeof host, NULL, NULL);
            service_host = host;
            positional++;
        } else if (positional == 1) {
            WideCharToMultiByte(CP_UTF8, 0, arguments[index], -1, port, sizeof port, NULL, NULL);
            service_port = port;
            positional++;
        }
    }
    if (WSAStartup(MAKEWORD(2, 2), &wsa) != 0) {
        say("WSAStartup failed");
        return 1;
    }
    if (persistent || separator < count)
        return run_until_prefix_exits(arguments + separator + 1, count - separator - 1);

    control = attach_to_service(&names, &size);
    if (control == INVALID_SOCKET) {
        say("no PRP service at %s:%s", service_host, service_port);
        return 1;
    }
    serve_pipes(control, names, size);
    say("PRP service closed the control connection; exiting");
    return 0;
}
