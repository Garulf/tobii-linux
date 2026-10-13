/*
 * Wine-side PRP pipe relay for the Linux PRP service (src/tobii_linux/prp_service.py).
 *
 * Owns the named pipes the game DLL's PRP client opens, opens each client's
 * reply pipe when the service names it, and relays every pipe message over one
 * loopback TCP connection per pipe connection. Relay framing: docs/prp-bridge.md.
 *
 * Usage: prp_bridge.exe [host] [port]   (defaults 127.0.0.1 7760)
 */
#define WIN32_LEAN_AND_MEAN
#include <winsock2.h>
#include <ws2tcpip.h>
#include <windows.h>
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

static const char *service_host = "127.0.0.1";
static const char *service_port = "7760";
static CRITICAL_SECTION log_lock;

typedef struct {
    HANDLE pipe;
    HANDLE reply;
    SOCKET socket;
    HANDLE stop;
    char name[PIPE_NAME_LIMIT];
} Session;

static void say(const char *format, ...)
{
    va_list args;
    SYSTEMTIME now;
    GetLocalTime(&now);
    EnterCriticalSection(&log_lock);
    fprintf(stderr, "%02u:%02u:%02u.%03u ", now.wHour, now.wMinute, now.wSecond,
            now.wMilliseconds);
    va_start(args, format);
    vfprintf(stderr, format, args);
    va_end(args);
    fputc('\n', stderr);
    fflush(stderr);
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

/* Server pipe as the DLL's own service code creates it (0x1800aae50): inbound, message type. */
static DWORD WINAPI listen_pipe(void *context)
{
    const char *name = context;
    char path[PIPE_NAME_LIMIT + 16];
    pipe_path(path, sizeof path, name);
    for (;;) {
        OVERLAPPED overlapped = {0};
        DWORD ignored;
        Session *session;
        HANDLE pipe = CreateNamedPipeA(path, PIPE_ACCESS_INBOUND | FILE_FLAG_OVERLAPPED,
                                       PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT,
                                       PIPE_UNLIMITED_INSTANCES, 0, SERVER_PIPE_IN_BUFFER,
                                       PIPE_DEFAULT_TIMEOUT_MS, NULL);
        if (pipe == INVALID_HANDLE_VALUE) {
            say("CreateNamedPipeA(%s) failed: %lu", path, GetLastError());
            Sleep(1000);
            continue;
        }
        overlapped.hEvent = CreateEventA(NULL, TRUE, FALSE, NULL);
        if (!ConnectNamedPipe(pipe, &overlapped)) {
            DWORD error = GetLastError();
            if (error == ERROR_IO_PENDING) {
                if (!GetOverlappedResult(pipe, &overlapped, &ignored, TRUE))
                    error = GetLastError();
                else
                    error = 0;
            } else if (error == ERROR_PIPE_CONNECTED) {
                error = 0;
            }
            if (error != 0) {
                say("ConnectNamedPipe(%s) failed: %lu", name, error);
                CloseHandle(overlapped.hEvent);
                CloseHandle(pipe);
                continue;
            }
        }
        CloseHandle(overlapped.hEvent);
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

static void report_display_id(SOCKET control)
{
    WCHAR instance_id[256];
    char utf8[512];
    int length;
    if (!primary_monitor_instance_id(instance_id, sizeof instance_id)) {
        say("primary monitor id unavailable; the service needs --display-id for window tracking");
        return;
    }
    length = WideCharToMultiByte(CP_UTF8, 0, instance_id, -1, utf8, sizeof utf8, NULL, NULL);
    if (length > 1 && send_frame(control, FRAME_DISPLAY_ID, utf8, (UINT32)length - 1))
        say("reported primary monitor %s", utf8);
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

int main(int argc, char **argv)
{
    WSADATA wsa;
    SOCKET control;
    char *names = NULL, *cursor, sink[16];
    UINT32 size = 0;
    int count = 0;

    InitializeCriticalSection(&log_lock);
    if (argc > 1)
        service_host = argv[1];
    if (argc > 2)
        service_port = argv[2];
    if (WSAStartup(MAKEWORD(2, 2), &wsa) != 0) {
        say("WSAStartup failed");
        return 1;
    }
    control = attach_to_service(&names, &size);
    if (control == INVALID_SOCKET) {
        say("no PRP service at %s:%s", service_host, service_port);
        return 1;
    }
    report_display_id(control);
    for (cursor = names; cursor < names + size && *cursor && count < MAX_PIPES;
         cursor += strlen(cursor) + 1, count++) {
        say("serving \\\\.\\pipe\\%s", cursor);
        CloseHandle(CreateThread(NULL, 0, listen_pipe, cursor, 0, NULL));
    }
    while (recv(control, sink, sizeof sink, 0) > 0)
        ;
    say("PRP service closed the control connection; exiting");
    return 0;
}
