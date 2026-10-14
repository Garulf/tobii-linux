/*
 * Game integration test client: drives Star Citizen's tobii_gameintegration_x64.dll
 * through its exported C functions the way a game would, and prints head poses and
 * gaze points.
 *
 * Usage: tgi_test_client.exe <dll> [first|window|rectangle|displays|<tracker url>] [seconds]
 * "first" (default) tracks the first tracker GetTrackerInfos reports, "window"
 * creates a window and calls TrackWindow, "rectangle" calls TrackRectangle with
 * 100,100,740,580 (the same monitor lookup as TrackWindow, without a window), anything
 * else is passed to TrackTracker. "displays" only lists the active monitors with the
 * device instance id the layer compares with the tracker's display id (0x1800084d0),
 * the value to give the service as --display-id; it needs
 * CM_Get_Device_Interface_PropertyW, which Wine 9.0 does not implement.
 * Every exported call goes through the DLL's lazily created "TGI C API" instance
 * (0x180005300). Signatures and structures were recovered statically, see
 * docs/game-integration-layer.md. OutputDebugString text of this process (the
 * layer's own log) is printed as "debug" lines. GetGazePoints returns the layer's
 * buffer of valid gaze points as (2x - 1, 1 - 2y) of the Stream Engine point, observed
 * on Windows and Wine; callback 0x18000a0a0 drops invalid points and points 0.75 or
 * more from the display centre on either axis.
 */
#include <windows.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef struct {
    float rotation_deg[3];
    float position_mm[3];
    int64_t timestamp_us;
} HeadPose;

typedef struct {
    int64_t timestamp_us;
    float x;
    float y;
} GazePoint;

typedef struct {
    int32_t type;
    uint32_t capabilities;
    int32_t monitor_rect[4];
    int32_t display_size_mm[2];
    const char *url;
    const char *strings[6];
    uint8_t attached;
    uint8_t padding[7];
} TrackerInfo;

typedef BOOL (*BoolCall)(void);
typedef void (*VoidCall)(void);
typedef BOOL (*TrackTrackerCall)(const char *url);
typedef BOOL (*TrackWindowCall)(HWND window);
typedef BOOL (*TrackRectangleCall)(const RECT *rectangle);
typedef BOOL (*GetTrackerInfosCall)(const TrackerInfo **infos, int *count);
typedef BOOL (*GetTrackerInfoCall)(TrackerInfo *info);
typedef BOOL (*IsStreamSupportedCall)(const uint32_t *capability);
typedef uint32_t (*GetHeadPosesCall)(const HeadPose **poses);
typedef BOOL (*GetLatestHeadPoseCall)(HeadPose *pose);
typedef uint32_t (*GetGazePointsCall)(const GazePoint **points);

#define HEAD_POSE_CAPABILITY 2u
#define TRACKER_INFO_STRINGS 6

static HMODULE module;
static LARGE_INTEGER frequency;
static const char *const tracker_string_names[TRACKER_INFO_STRINGS] = {
    "property_8", "display_name", "model", "generation", "serial", "firmware"};

static void *need(const char *name)
{
    FARPROC function = GetProcAddress(module, name);
    void *address;
    if (!function) {
        printf("error missing_export=%s\n", name);
        exit(2);
    }
    memcpy(&address, &function, sizeof address);
    return address;
}

static int64_t qpc_us(void)
{
    LARGE_INTEGER now;
    QueryPerformanceCounter(&now);
    return now.QuadPart / frequency.QuadPart * 1000000
           + now.QuadPart % frequency.QuadPart * 1000000 / frequency.QuadPart;
}

typedef struct {
    DWORD process_id;
    char text[4096 - sizeof(DWORD)];
} DebugBuffer;

static DWORD WINAPI debug_output_reader(void *unused)
{
    HANDLE buffer_ready, data_ready, mapping;
    DebugBuffer *buffer;
    char line[sizeof buffer->text + 1];
    size_t length;
    (void)unused;
    buffer_ready = CreateEventA(NULL, FALSE, FALSE, "DBWIN_BUFFER_READY");
    data_ready = CreateEventA(NULL, FALSE, FALSE, "DBWIN_DATA_READY");
    mapping = CreateFileMappingA(INVALID_HANDLE_VALUE, NULL, PAGE_READWRITE, 0, sizeof *buffer,
                                 "DBWIN_BUFFER");
    if (!buffer_ready || !data_ready || !mapping) {
        printf("debug_capture unavailable error=%lu\n", GetLastError());
        return 0;
    }
    buffer = MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, sizeof *buffer);
    if (!buffer)
        return 0;
    for (;;) {
        SetEvent(buffer_ready);
        if (WaitForSingleObject(data_ready, INFINITE) != WAIT_OBJECT_0)
            return 0;
        if (buffer->process_id != GetCurrentProcessId())
            continue;
        memcpy(line, buffer->text, sizeof buffer->text);
        line[sizeof buffer->text] = 0;
        length = strlen(line);
        while (length && (line[length - 1] == '\n' || line[length - 1] == '\r'))
            line[--length] = 0;
        printf("debug %s\n", line);
    }
}

typedef struct {
    GUID category;
    ULONG id;
} DeviceProperty;
typedef DWORD (WINAPI *GetInterfacePropertyCall)(const WCHAR *device_interface, const DeviceProperty *key,
                                                ULONG *type, BYTE *buffer, ULONG *size, ULONG flags);

static const DeviceProperty device_instance_id = {
    {0x78c34fc8, 0x104a, 0x4aca, {0x9e, 0xa4, 0x52, 0x4d, 0x52, 0x99, 0x6e, 0x57}}, 256};

static void list_displays(void)
{
    DISPLAY_DEVICEW adapter, monitor;
    WCHAR instance_id[256];
    ULONG type, size;
    DWORD adapter_index, monitor_index, status;
    HMODULE cfgmgr = LoadLibraryA("cfgmgr32.dll");
    FARPROC function = cfgmgr ? GetProcAddress(cfgmgr, "CM_Get_Device_Interface_PropertyW") : NULL;
    GetInterfacePropertyCall get_property = NULL;
    if (function)
        memcpy(&get_property, &function, sizeof get_property);
    adapter.cb = sizeof adapter;
    for (adapter_index = 0; EnumDisplayDevicesW(NULL, adapter_index, &adapter, 0); adapter_index++) {
        monitor.cb = sizeof monitor;
        for (monitor_index = 0; EnumDisplayDevicesW(adapter.DeviceName, monitor_index, &monitor,
                                                    EDD_GET_DEVICE_INTERFACE_NAME);
             monitor_index++) {
            instance_id[0] = 0;
            size = sizeof instance_id;
            status = get_property ? get_property(monitor.DeviceID, &device_instance_id, &type,
                                                 (BYTE *)instance_id, &size, 0)
                                  : 0xffffffff;
            printf("display adapter=%ls flags=0x%lx interface=%ls instance_id=%ls status=%lu\n",
                   adapter.DeviceName, monitor.StateFlags, monitor.DeviceID,
                   status == 0 ? instance_id : L"", status);
            monitor.cb = sizeof monitor;
        }
        adapter.cb = sizeof adapter;
    }
}

static void print_tracker(const char *label, int index, const TrackerInfo *info)
{
    int string;
    printf("%s[%d] type=%d capabilities=0x%x monitor_rect=%d,%d,%d,%d display_mm=%d,%d "
           "attached=%u url=%s",
           label, index, info->type, info->capabilities, info->monitor_rect[0],
           info->monitor_rect[1], info->monitor_rect[2], info->monitor_rect[3],
           info->display_size_mm[0], info->display_size_mm[1], info->attached,
           info->url ? info->url : "(null)");
    for (string = 0; string < TRACKER_INFO_STRINGS; string++)
        printf(" %s=\"%s\"", tracker_string_names[string],
               info->strings[string] ? info->strings[string] : "(null)");
    printf("\n");
}

static HWND create_test_window(void)
{
    WNDCLASSA window_class = {0};
    window_class.lpfnWndProc = DefWindowProcA;
    window_class.hInstance = GetModuleHandleA(NULL);
    window_class.lpszClassName = "tgi_test_client";
    RegisterClassA(&window_class);
    return CreateWindowA("tgi_test_client", "tgi_test_client", WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                         100, 100, 640, 480, NULL, NULL, window_class.hInstance, NULL);
}

static void pump_messages(void)
{
    MSG message;
    while (PeekMessageA(&message, NULL, 0, 0, PM_REMOVE)) {
        TranslateMessage(&message);
        DispatchMessageA(&message);
    }
}

int main(int argc, char **argv)
{
    const char *mode = argc > 2 ? argv[2] : "first";
    double seconds = argc > 3 ? atof(argv[3]) : 10.0;
    char url[256] = "";
    const TrackerInfo *infos = NULL;
    TrackerInfo tracked = {0};
    const HeadPose *poses;
    const GazePoint *points;
    HWND window = NULL;
    int count = 0, index, tracking = 0, updates = 0, last_connected = -1, connected;
    int last_present = -1, present;
    uint32_t pose_count, total_poses = 0, capability = HEAD_POSE_CAPABILITY;
    uint32_t point_count, total_points = 0;
    int64_t last_timestamp = 0, last_point_timestamp = 0;
    ULONGLONG deadline, next_track = 0;
    BoolCall is_api_initialized, is_connected, is_present;
    VoidCall update, update_tracker_infos, stop_tracking, shutdown_api;
    TrackTrackerCall track_tracker;
    TrackWindowCall track_window;
    TrackRectangleCall track_rectangle;
    const RECT rectangle = {100, 100, 740, 580};
    int use_rectangle = strcmp(mode, "rectangle") == 0;
    GetTrackerInfosCall get_tracker_infos;
    GetTrackerInfoCall get_tracker_info;
    IsStreamSupportedCall is_stream_supported;
    GetHeadPosesCall get_head_poses;
    GetGazePointsCall get_gaze_points;
    BOOL infos_ready = FALSE;

    setvbuf(stdout, NULL, _IOLBF, 0);
    QueryPerformanceFrequency(&frequency);
    if (argc < 2) {
        printf("usage: %s <tobii_gameintegration_x64.dll> [first|window|<url>] [seconds]\n",
               argv[0]);
        return 2;
    }
    if (strcmp(mode, "displays") == 0) {
        list_displays();
        return 0;
    }
    CloseHandle(CreateThread(NULL, 0, debug_output_reader, NULL, 0, NULL));
    Sleep(100);
    module = LoadLibraryA(argv[1]);
    if (!module) {
        printf("error load_library=%lu\n", GetLastError());
        return 2;
    }
    is_api_initialized = need("IsApiInitialized");
    is_connected = need("IsConnected");
    is_present = need("IsPresent");
    update = need("Update");
    update_tracker_infos = need("UpdateTrackerInfos");
    stop_tracking = need("StopTracking");
    shutdown_api = need("Shutdown");
    track_tracker = need("TrackTracker");
    track_window = need("TrackWindow");
    track_rectangle = need("TrackRectangle");
    get_tracker_infos = need("GetTrackerInfos");
    get_tracker_info = need("GetTrackerInfo");
    is_stream_supported = need("IsStreamSupported");
    get_head_poses = need("GetHeadPoses");
    get_gaze_points = need("GetGazePoints");

    printf("api_initialized %d\n", is_api_initialized() & 0xff);
    update_tracker_infos();
    deadline = GetTickCount64() + 15000;
    while (GetTickCount64() < deadline) {
        update();
        infos_ready = get_tracker_infos(&infos, &count) & 0xff;
        if (infos_ready && count > 0)
            break;
        Sleep(100);
    }
    printf("tracker_infos ready=%d count=%d after_ms=%llu\n", infos_ready, count,
           (unsigned long long)(GetTickCount64() + 15000 - deadline));
    for (index = 0; index < count; index++)
        print_tracker("tracker", index, &infos[index]);

    if (strcmp(mode, "window") == 0) {
        window = create_test_window();
        printf("window %p error=%lu\n", (void *)window, window ? 0 : GetLastError());
        if (!window) {
            shutdown_api();
            return 1;
        }
    } else if (strcmp(mode, "first") == 0) {
        if (count > 0 && infos[0].url)
            snprintf(url, sizeof url, "%s", infos[0].url);
    } else {
        snprintf(url, sizeof url, "%s", mode);
    }
    if (!window && !use_rectangle && !url[0]) {
        printf("error no_tracker_url\n");
        shutdown_api();
        return 1;
    }

    deadline = GetTickCount64() + (ULONGLONG)(seconds * 1000);
    while (GetTickCount64() < deadline) {
        if (window)
            pump_messages();
        if (!tracking && GetTickCount64() >= next_track) {
            if (window) {
                tracking = track_window(window) & 0xff;
                printf("track_window result=%d\n", tracking);
            } else if (use_rectangle) {
                tracking = track_rectangle(&rectangle) & 0xff;
                printf("track_rectangle %ld,%ld,%ld,%ld result=%d\n", rectangle.left,
                       rectangle.top, rectangle.right, rectangle.bottom, tracking);
            } else {
                tracking = track_tracker(url) & 0xff;
                printf("track_tracker %s result=%d\n", url, tracking);
            }
            next_track = GetTickCount64() + 1000;
        }
        update();
        updates++;
        connected = is_connected() & 0xff;
        if (connected != last_connected) {
            printf("connected %d present=%d head_stream_supported=%d qpc_us=%lld\n", connected,
                   is_present() & 0xff, is_stream_supported(&capability) & 0xff,
                   (long long)qpc_us());
            last_connected = connected;
        }
        present = is_present() & 0xff;
        if (present != last_present) {
            printf("present %d qpc_us=%lld\n", present, (long long)qpc_us());
            last_present = present;
        }
        pose_count = get_head_poses(&poses);
        for (index = 0; index < (int)pose_count; index++) {
            if (poses[index].timestamp_us == last_timestamp)
                continue;
            last_timestamp = poses[index].timestamp_us;
            total_poses++;
            printf("head timestamp_us=%lld qpc_us=%lld rotation_deg=%.4f,%.4f,%.4f "
                   "position_mm=%.3f,%.3f,%.3f\n",
                   (long long)poses[index].timestamp_us, (long long)qpc_us(),
                   poses[index].rotation_deg[0], poses[index].rotation_deg[1],
                   poses[index].rotation_deg[2], poses[index].position_mm[0],
                   poses[index].position_mm[1], poses[index].position_mm[2]);
        }
        point_count = get_gaze_points(&points);
        for (index = 0; index < (int)point_count; index++) {
            if (points[index].timestamp_us <= last_point_timestamp)
                continue;
            last_point_timestamp = points[index].timestamp_us;
            total_points++;
            printf("gaze timestamp_us=%lld qpc_us=%lld x=%.6f y=%.6f\n",
                   (long long)points[index].timestamp_us, (long long)qpc_us(), points[index].x,
                   points[index].y);
        }
        Sleep(16);
    }
    if (get_tracker_info(&tracked) & 0xff)
        print_tracker("tracked", 0, &tracked);
    stop_tracking();
    shutdown_api();
    if (window)
        DestroyWindow(window);
    printf("summary head_poses=%u gaze_points=%u updates=%d tracking=%d connected=%d "
           "seconds=%.1f\n", total_poses, total_points, updates, tracking, last_connected,
           seconds);
    return total_poses > 0 || total_points > 0 ? 0 : 1;
}
