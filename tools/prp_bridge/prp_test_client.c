/*
 * Head-pose and gaze test client: loads a Tobii Stream Engine DLL, creates a device
 * from an explicit tobii-prp:// URL, or from the first one the DLL's own discovery
 * reports when <url> is "discover", subscribes head pose and gaze point and prints
 * samples.
 *
 * Usage: prp_test_client.exe <dll> <url|discover> [seconds] [field_of_use]
 * <dll> is tobii_stream_engine.dll (named exports) or Star Citizen's
 * tobii_gameintegration_x64.dll 9.0.3.25, whose embedded Stream Engine entry points
 * are not exported and are called at their static addresses (docs/prp-bridge.md).
 * Output: one line per event; "head" and "gaze" lines carry the sample, the Stream
 * Engine system clock and the local QueryPerformanceCounter time in microseconds.
 */
#include <windows.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef struct {
    int64_t timestamp_us;
    int32_t position_validity;
    float position_xyz[3];
    int32_t rotation_validity_xyz[3];
    float rotation_xyz[3];
} HeadPose;

typedef struct {
    int64_t timestamp_us;
    int32_t validity;
    float position_xy[2];
} GazePoint;

typedef void (*UrlCallback)(const char *url, void *user);
typedef void (*HeadCallback)(const HeadPose *pose, void *user);
typedef void (*GazeCallback)(const GazePoint *point, void *user);
typedef void (*LogFunction)(void *context, int level, const char *text);
typedef struct {
    void *context;
    LogFunction log;
} CustomLog;
typedef int (*ApiCreate)(void **api, void *allocator, const CustomLog *logger);
typedef int (*ApiDestroy)(void *api);
typedef int (*EnumerateUrls)(void *api, UrlCallback receiver, void *user);
typedef int (*DeviceCreate)(void *api, const char *url, int field_of_use, void **device);
typedef int (*DeviceDestroy)(void *device);
typedef int (*HeadSubscribe)(void *device, HeadCallback callback, void *user);
typedef int (*HeadUnsubscribe)(void *device);
typedef int (*GazeSubscribe)(void *device, GazeCallback callback, void *user);
typedef int (*GazeUnsubscribe)(void *device);
typedef int (*ProcessCallbacks)(void *device);
typedef int (*SystemClock)(void *api, int64_t *time_us);
typedef int (*UpdateTimesync)(void *device);
typedef int (*GetDeviceInfo)(void *device, void *info);
typedef const char *(*ErrorMessage)(int error);

#define DEVICE_INFO_STRING 256
#define DEVICE_INFO_STRINGS 6

static void *api;
static SystemClock system_clock;
static int head_count, gaze_count, valid_gaze_count;
static LARGE_INTEGER frequency;

static int64_t qpc_us(void)
{
    LARGE_INTEGER now;
    QueryPerformanceCounter(&now);
    return now.QuadPart / frequency.QuadPart * 1000000
           + now.QuadPart % frequency.QuadPart * 1000000 / frequency.QuadPart;
}

static void on_head(const HeadPose *pose, void *user)
{
    int64_t clock_us = 0;
    (void)user;
    system_clock(api, &clock_us);
    head_count++;
    printf("head timestamp_us=%lld system_clock_us=%lld qpc_us=%lld position_valid=%d "
           "position=%.3f,%.3f,%.3f rotation_valid=%d,%d,%d rotation=%.4f,%.4f,%.4f\n",
           (long long)pose->timestamp_us, (long long)clock_us, (long long)qpc_us(),
           pose->position_validity, pose->position_xyz[0], pose->position_xyz[1],
           pose->position_xyz[2], pose->rotation_validity_xyz[0],
           pose->rotation_validity_xyz[1], pose->rotation_validity_xyz[2],
           pose->rotation_xyz[0], pose->rotation_xyz[1], pose->rotation_xyz[2]);
    fflush(stdout);
}

static void on_gaze(const GazePoint *point, void *user)
{
    int64_t clock_us = 0;
    (void)user;
    system_clock(api, &clock_us);
    gaze_count++;
    valid_gaze_count += point->validity != 0;
    printf("gaze timestamp_us=%lld system_clock_us=%lld qpc_us=%lld validity=%d "
           "position=%.6f,%.6f\n",
           (long long)point->timestamp_us, (long long)clock_us, (long long)qpc_us(),
           point->validity, point->position_xy[0], point->position_xy[1]);
    fflush(stdout);
}

/* Game DLL build identity (PE TimeDateStamp, SizeOfImage) and entry RVAs, found statically. */
#define GAME_DLL_TIMESTAMP 0x661d6182u
#define GAME_DLL_IMAGE_SIZE 0x664000u
static const struct {
    const char *name;
    DWORD rva;
} game_entries[] = {
    {"tobii_error_message", 0xc9b0},       {"tobii_api_create", 0xcb80},
    {"tobii_api_destroy", 0xcbc0},         {"tobii_system_clock", 0xcc70},
    {"tobii_enumerate_local_device_urls", 0xcd20}, {"tobii_device_create", 0xd1c0},       {"tobii_device_destroy", 0xd740},
    {"tobii_device_process_callbacks", 0xda50}, {"tobii_update_timesync", 0xdef0},
    {"tobii_get_device_info", 0xe310},     {"tobii_head_pose_subscribe", 0xfd80},
    {"tobii_head_pose_unsubscribe", 0xfe20}, {"tobii_gaze_point_subscribe", 0xf920},
    {"tobii_gaze_point_unsubscribe", 0xf9b0},
};

static HMODULE module;
static ErrorMessage error_message;

static int is_game_dll(void)
{
    const IMAGE_DOS_HEADER *dos = (const IMAGE_DOS_HEADER *)module;
    const IMAGE_NT_HEADERS64 *nt = (const IMAGE_NT_HEADERS64 *)((const char *)module + dos->e_lfanew);
    return nt->FileHeader.TimeDateStamp == GAME_DLL_TIMESTAMP
           && nt->OptionalHeader.SizeOfImage == GAME_DLL_IMAGE_SIZE;
}

static void *need(const char *name)
{
    FARPROC function = GetProcAddress(module, name);
    void *address;
    size_t index;
    if (function) {
        memcpy(&address, &function, sizeof address);
        return address;
    }
    if (is_game_dll())
        for (index = 0; index < sizeof game_entries / sizeof game_entries[0]; index++)
            if (strcmp(game_entries[index].name, name) == 0)
                return (char *)module + game_entries[index].rva;
    printf("error missing_export=%s\n", name);
    exit(2);
}

static void on_log(void *context, int level, const char *text)
{
    (void)context;
    printf("log level=%d %s\n", level, text);
}

static int report(const char *name, int code)
{
    printf("result %s code=%d message=%s\n", name, code, error_message(code));
    return code;
}

static char discovered_url[256];

static void on_url(const char *url, void *user)
{
    (void)user;
    printf("discovered %s\n", url);
    if (!discovered_url[0] && strncmp(url, "tobii-prp://", 12) == 0)
        snprintf(discovered_url, sizeof discovered_url, "%s", url);
}

int main(int argc, char **argv)
{
    void *device = NULL;
    double seconds = argc > 3 ? atof(argv[3]) : 5.0;
    int field_of_use = argc > 4 ? atoi(argv[4]) : 1;
    static char info[16384];
    int code, calls = 0, index;
    ULONGLONG deadline;
    CustomLog logger = {NULL, on_log};
    ApiCreate api_create;
    ApiDestroy api_destroy;
    EnumerateUrls enumerate_urls;
    const char *url = argv[2];
    DeviceCreate device_create;
    DeviceDestroy device_destroy;
    GetDeviceInfo get_device_info;
    UpdateTimesync update_timesync;
    HeadSubscribe head_subscribe;
    HeadUnsubscribe head_unsubscribe;
    GazeSubscribe gaze_subscribe;
    GazeUnsubscribe gaze_unsubscribe;
    int gaze_subscribed = 0;
    ProcessCallbacks process_callbacks;

    setvbuf(stdout, NULL, _IOLBF, 0);
    QueryPerformanceFrequency(&frequency);
    if (argc < 3) {
        printf("usage: %s <tobii_stream_engine.dll> <tobii-prp://name|discover> [seconds] [field_of_use]\n",
               argv[0]);
        return 2;
    }
    module = LoadLibraryA(argv[1]);
    if (!module) {
        printf("error load_library=%lu\n", GetLastError());
        return 2;
    }
    printf("dll %s entry_points=%s\n", argv[1],
           GetProcAddress(module, "tobii_api_create") ? "exports"
           : is_game_dll() ? "game_dll_static_rva" : "unknown");
    error_message = need("tobii_error_message");
    system_clock = need("tobii_system_clock");
    api_create = need("tobii_api_create");
    api_destroy = need("tobii_api_destroy");
    device_create = need("tobii_device_create");
    enumerate_urls = need("tobii_enumerate_local_device_urls");
    device_destroy = need("tobii_device_destroy");
    get_device_info = need("tobii_get_device_info");
    update_timesync = need("tobii_update_timesync");
    head_subscribe = need("tobii_head_pose_subscribe");
    head_unsubscribe = need("tobii_head_pose_unsubscribe");
    gaze_subscribe = need("tobii_gaze_point_subscribe");
    gaze_unsubscribe = need("tobii_gaze_point_unsubscribe");
    process_callbacks = need("tobii_device_process_callbacks");

    if (report("api_create", api_create(&api, NULL, &logger)))
        return 1;
    if (strcmp(url, "discover") == 0) {
        report("enumerate_local_device_urls", enumerate_urls(api, on_url, NULL));
        if (!discovered_url[0]) {
            printf("error no_prp_device_discovered\n");
            api_destroy(api);
            return 1;
        }
        url = discovered_url;
    }
    printf("url %s field_of_use=%d\n", url, field_of_use);
    if (report("device_create", device_create(api, url, field_of_use, &device)) == 0) {
        if (report("get_device_info", get_device_info(device, info)) == 0)
            for (index = 0; index < DEVICE_INFO_STRINGS; index++)
                printf("device_info[%d] %.255s\n", index, info + index * DEVICE_INFO_STRING);
        report("update_timesync", update_timesync(device));
        gaze_subscribed = report("gaze_point_subscribe",
                                 gaze_subscribe(device, on_gaze, NULL)) == 0;
        if (report("head_pose_subscribe", head_subscribe(device, on_head, NULL)) == 0) {
            deadline = GetTickCount64() + (ULONGLONG)(seconds * 1000);
            while (GetTickCount64() < deadline) {
                code = process_callbacks(device);
                calls++;
                if (code != 0) {
                    report("device_process_callbacks", code);
                    break;
                }
                Sleep(5);
            }
            report("head_pose_unsubscribe", head_unsubscribe(device));
        }
        if (gaze_subscribed)
            report("gaze_point_unsubscribe", gaze_unsubscribe(device));
        report("device_destroy", device_destroy(device));
    }
    report("api_destroy", api_destroy(api));
    printf("summary head_samples=%d gaze_samples=%d valid_gaze_samples=%d process_calls=%d "
           "seconds=%.1f\n", head_count, gaze_count, valid_gaze_count, calls, seconds);
    return head_count > 0 && (!gaze_subscribed || gaze_count > 0) ? 0 : 1;
}
