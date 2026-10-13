# Installed Tobii managed API evidence

Scope: only the installed Tobii assembly copied from albedo, and REA static analysis. No other driver projects were consulted. This report records managed declarations and CIL references, not USB traffic or runtime observations.

## Artifact and evidence

- Assembly: Tobii.StreamEngine.Net, version 4.72.0.29983, .NET Framework 4.8, AnyCPU CIL.
- SHA-256: `51ec6fd76bd126fd97afe86c920b83750f9700b40d3c0b2e07459b5dff69042a`.
- MVID: `f840b997-97e4-4a53-b8eb-7b9966fdad6c`. Tokens below are meaningful only for this build.
- Identity evidence: `ev_2a1bbbd2f42390587f48e8e2f618493153cda9729eaa5b64a68ff4761d76beb0`, retained in `evidence/private/streamengine-managed.json`.
- Members evidence: `ev_f2fd89ff0420d721a6e3347f10f24176beab8a11ec9cd7c1facf3a7eb0a11e7f`, retained completely in `evidence/private/streamengine-managed-members.json`.
- Boundaries evidence: `ev_214e60a2bd81fa42eb032d2951992233e110b81d9199593d1dd60191eab4d90e`, retained completely in `evidence/private/streamengine-managed-native-boundaries.json`.
- Producer: REA CLI 6.4.0, rea-dotnet-static provider 1.

## Observed native boundary

All 186 P/Invoke rows import from `tobii_stream_engine` with cdecl calling convention. This establishes a native library boundary; it does not verify exports, ELF availability, or implementations. The managed assembly is an API wrapper rather than evidence for a self-contained managed eye-tracking algorithm.

The current build declares `tobii_device_create(api, url, field_of_use, out device)`. Its `tobii_wait_for_callbacks` import takes a device count and device array. These signatures must be versioned rather than assumed from older API examples.

## Stream structures

The following are ordered managed fields from members evidence. `i8` is signed 64-bit, `i4` signed 32-bit, `r4` float32. They are not yet verified native byte offsets, alignment, or USB packet layouts. Enum numeric constants and custom marshalling are not recovered by this member projection; a supplemental raw-metadata constant decoder is described below.

### TobiiVector2 (0x02000010)

```text
r4 x
r4 y
```

### TobiiVector3 (0x02000011)

```text
r4 x
r4 y
r4 z
```

### tobii_gaze_point_t (0x0200001b)

```text
i8 timestamp_us
tobii_validity_t validity
TobiiVector2 position
```

### tobii_gaze_origin_t (0x0200001d)

```text
i8 timestamp_us
tobii_validity_t left_validity
TobiiVector3 left
tobii_validity_t right_validity
TobiiVector3 right
```

### tobii_responsive_gaze_point_t (0x02000020)

```text
i8 timestamp_us
tobii_validity_t validity
TobiiVector2 position
```

### tobii_display_area_t (0x02000036)

```text
TobiiVector3 top_left_mm_xyz
TobiiVector3 top_right_mm_xyz
TobiiVector3 bottom_left_mm_xyz
```

### tobii_calibration_point_data_t (0x02000039)

```text
TobiiVector2 point_xy
tobii_calibration_point_status_t left_status
TobiiVector2 left_mapping_xy
tobii_calibration_point_status_t right_status
TobiiVector2 right_mapping_xy
```

### tobii_gaze_data_eye_t (0x0200003b)

```text
tobii_validity_t gaze_origin_validity
TobiiVector3 gaze_origin_from_eye_tracker_mm_xyz
tobii_validity_t eye_position_validity
TobiiVector3 eye_position_in_track_box_normalized_xyz
tobii_validity_t gaze_point_validity
TobiiVector3 gaze_point_from_eye_tracker_mm_xyz
TobiiVector2 gaze_point_on_display_normalized_xy
tobii_validity_t eyeball_center_validity
TobiiVector3 eyeball_center_from_eye_tracker_mm_xyz
tobii_validity_t pupil_validity
r4 pupil_diameter_mm
```

### tobii_gaze_data_t (0x0200003c)

```text
i8 timestamp_tracker_us
i8 timestamp_system_us
tobii_gaze_data_eye_t left
tobii_gaze_data_eye_t right
```

### tobii_head_pose_t (0x02000044)

```text
i8 timestamp_us
tobii_validity_t position_validity
TobiiVector3 position_xyz
tobii_validity_t rotation_x_validity
tobii_validity_t rotation_y_validity
tobii_validity_t rotation_z_validity
TobiiVector3 rotation_xyz
```

### tobii_low_frequency_head_rotation_t (0x02000078)

```text
i8 timestamp_us
tobii_validity_t validity
TobiiVector3 rotation_xyz
```

### tobii_low_frequency_head_position_t (0x0200009b)

```text
i8 timestamp_us
tobii_validity_t validity
TobiiVector3 position_xyz
```

## Relevant API declarations

These signatures are translated from ECMA-335 metadata. `native-int` is pointer-sized; `&` means by-reference. The exact C interpretation of pointer-sized size parameters needs native corroboration. Rows below include only actual P/Invoke imports.

```text
0x06000077 tobii_error_t tobii_api_create(native-int&, native-int, tobii_custom_log_t)
0x0600007d tobii_error_t tobii_device_create(native-int, string, tobii_field_of_use_t, native-int&)
0x0600007e tobii_error_t tobii_device_create_ex(native-int, string, tobii_field_of_use_t, tobii_license_key_t[], i4, tobii_license_validation_result_t[], native-int&)
0x06000082 tobii_error_t tobii_wait_for_callbacks(i4, native-int[])
0x06000084 tobii_error_t tobii_device_process_callbacks(native-int)
0x06000092 tobii_error_t tobii_capability_supported(native-int, tobii_capability_t, bool&)
0x06000093 tobii_error_t tobii_stream_supported(native-int, tobii_stream_t, bool&)
0x06000094 tobii_error_t tobii_gaze_point_subscribe(native-int, tobii_gaze_point_callback_t, native-int)
0x06000095 tobii_error_t tobii_gaze_point_unsubscribe(native-int)
0x0600009c tobii_error_t tobii_head_pose_subscribe(native-int, tobii_head_pose_callback_t, native-int)
0x0600009d tobii_error_t tobii_head_pose_unsubscribe(native-int)
0x060000a2 tobii_error_t tobii_responsive_gaze_point_subscribe(native-int, tobii_responsive_gaze_point_callback_t, native-int)
0x060000a3 tobii_error_t tobii_responsive_gaze_point_unsubscribe(native-int)
0x060000b2 tobii_error_t tobii_calibration_start(native-int, tobii_enabled_eye_t)
0x060000b3 tobii_error_t tobii_calibration_stop(native-int)
0x060000b4 tobii_error_t tobii_calibration_clear(native-int)
0x060000b5 tobii_error_t tobii_calibration_collect_data_2d(native-int, r4, r4)
0x060000b6 tobii_error_t tobii_calibration_collect_data_3d(native-int, r4, r4, r4)
0x060000b7 tobii_error_t tobii_calibration_collect_data_per_eye_2d(native-int, r4, r4, tobii_enabled_eye_t, tobii_enabled_eye_t&)
0x060000b8 tobii_error_t tobii_calibration_discard_data_2d(native-int, r4, r4)
0x060000b9 tobii_error_t tobii_calibration_discard_data_3d(native-int, r4, r4, r4)
0x060000ba tobii_error_t tobii_calibration_discard_data_per_eye_2d(native-int, r4, r4, tobii_enabled_eye_t)
0x060000bb tobii_error_t tobii_calibration_compute_and_apply(native-int)
0x060000bc tobii_error_t tobii_calibration_compute_and_apply_per_eye(native-int, tobii_enabled_eye_t&)
0x060000be tobii_error_t tobii_calibration_retrieve(native-int, tobii_data_receiver_t, native-int)
0x060000c0 tobii_error_t tobii_calibration_parse(native-int, native-int, native-int, tobii_calibration_point_data_receiver_t, native-int)
0x060000c1 tobii_error_t tobii_calibration_apply(native-int, native-int, native-int)
0x060000c3 tobii_error_t tobii_calculate_display_area_basic(native-int, r4, r4, r4, tobii_geometry_mounting_t&, tobii_display_area_t&)
0x060000c4 tobii_error_t tobii_get_display_area(native-int, tobii_display_area_t&)
0x060000c5 tobii_error_t tobii_set_display_area(native-int, tobii_display_area_t&)
0x060000d4 tobii_error_t tobii_calibration_stimulus_points_get(native-int, tobii_calibration_stimulus_points_t&)
0x060001cc tobii_error_t tobii_internal_capability_supported(native-int, tobii_internal_capability_t, bool&)
0x060001cd tobii_error_t tobii_internal_stream_supported(native-int, tobii_internal_stream_t, bool&)
```

### Callback declarations

```text
0x060000ed Tobii.StreamEngine.tobii_data_receiver_t.Invoke(native-int, native-int, native-int) -> void
0x060000fd Tobii.StreamEngine.tobii_gaze_point_callback_t.Invoke(tobii_gaze_point_t&, native-int) -> void
0x0600012d Tobii.StreamEngine.tobii_head_pose_callback_t.Invoke(tobii_head_pose_t&, native-int) -> void
0x06000216 tobii_calibration_point_data_receiver_t.Invoke(tobii_calibration_point_data_t&, native-int) -> void
```

## Observed wrapper call relationships

Members evidence records CIL calls, not their execution or branch conditions:

- `EyeTracker.add_GazePoint` (`0x06000019`) calls `tobii_gaze_point_subscribe`; `add_HeadPose` (`0x0600002b`) calls `tobii_head_pose_subscribe`.
- `StartProcessingCallbacks` (`0x06000047`) references `RunProcessCallbackLoop`, sets a thread background flag and name, and calls Start.
- `RunProcessCallbackLoop` (`0x0600004a`) references `tobii_wait_for_callbacks`, `ProcessCallbacks`, `tobii_device_reconnect`, and Sleep. `ProcessCallbacks` (`0x06000049`) calls `tobii_device_process_callbacks`.
- `Dispose` (`0x0600004b`) calls `StopProcessingCallbacks` and `tobii_device_destroy`.

Inference: a Linux implementation needs a device lifecycle, callback queue processing, separate gaze and head subscriptions, invalid-sample handling, reconnection, and per-device calibration state to reproduce this boundary. These are implementation obligations, not proof that the native device transport has been recovered.

Calibration exposes start/stop/clear, 2D and 3D sample collection/discard, per-eye variants, compute/apply, retrieve/parse/apply blobs, stimulus points, and display geometry. The declarations do not reveal the blob format, calibration algorithm, or which calls Eye Tracker 5 accepts.

## Limits and remaining work

- REA reports partial member coverage: 188 types, 742 fields, 564 methods; 10 method signatures are not decoded. Full raw evidence is retained rather than treating the display projection as complete.
- Head rotation units, coordinate orientation, native struct packing, actual callback rate, and timing behavior still need native or runtime evidence. Names such as `timestamp_us` and `top_left_mm_xyz` are declarations, not measurement validation.
- The assembly covers multiple Tobii device families. Presence of wearable, diagnostic, or calibration declarations does not imply Eye Tracker 5 support. Capability queries must be tested on the actual device.
- No USB command framing, sensor initialization, firmware loading, image processing, calibration algorithm, or service IPC protocol is established here.
- No Star Citizen/Wine ABI compatibility or Linux end-to-end gaze/head tracking has been demonstrated. A subsequent Windows stream probe is described below.

## Supplemental constants and Windows oracle

`tools/decode_managed_constants.py` reads this exact assembly without executing it, anchors the ECMA-335 Constant table against the REA first CustomAttribute row and table counts, and decodes selected int32 constants from the Blob heap. The complete output, including source hash and byte offsets, is retained as `evidence/private/streamengine-constants.json`. This supplemental decoder does not claim its own REA Evidence ID.

Observed numeric values include `TOBII_ERROR_NO_ERROR=0`, invalid/valid `0/1`, `TOBII_STREAM_GAZE_POINT=0`, `TOBII_STREAM_HEAD_POSE=4`, and field-of-use store/transfer false/true `1/2`. The latter must not be guessed as a conventional zero/one boolean.

`tools/windows_stream_probe.py` is a bounded Windows ctypes research oracle. It verifies the selected native DLL SHA-256, obtains and reports its actual API version, enumerates device URLs, creates one device with recording field of use, subscribes gaze and head pose, and emits JSONL. It polls callback processing, then unsubscribes and destroys device/API handles. A subprocess supervisor terminates blocked native calls after the requested capture duration plus 30 seconds; such termination cannot guarantee graceful cleanup. This tool has no calibration or firmware write operations.

Managed TypeDef flags establish sequential layout for the selected structs and there are no ClassLayout table rows. Default x64 sequential packing therefore predicts gaze size 24 with timestamp/validity/vector offsets 0/8/12; head size 48 with offsets 0/8/12/24/28/32/36. The probe verifies these ctypes offsets and reports them explicitly. The prediction is not independent confirmation from native machine code. `tobii_get_api_version` is a P/Invoke returning `tobii_error_t` with a by-reference four-int32 version struct (method token `0x06000075`). The managed assembly version must not be substituted for the native version returned at runtime.

## Windows runtime corroboration

The parent investigation ran `tools/windows_stream_probe.py` on albedo against the exact native DLL hash above. Full JSONL is retained in `evidence/private/windows-stream-probe.jsonl`; this is a direct probe artifact, not REA-generated runtime evidence. The native library returned version **4.25.0.3**, distinct from managed assembly version **4.72.0.29983**.

During a three-second observation the probe received **99 gaze samples and 92 head-pose samples**, and made 553 successful callback-processing calls. API creation, enumeration, device creation with recording field of use `2`, both subscriptions, both unsubscriptions, device destruction and API destruction all returned error code `0`. A device URL using the `tobii-prp://` scheme was returned; its private identifier remains in the retained local evidence.

This corroborates that the selected native library accepts the recovered call signatures and callback layouts sufficiently to deliver samples in this short Windows session. It does not establish calibration correctness, coordinate units or signs, every field's interpretation, long-term reconnection behavior, maximum sample rate, USB transport implementation, Linux portability, or Star Citizen compatibility. The observed counts must not be presented as rated hardware sampling frequencies.

## Standard USB descriptor probe

`tools/windows_usb_descriptors.py` accepts an explicit device path and restricts it to the exact observed ET5 MI_00 instance and interface GUID. It requests shared read access, initializes WinUSB, obtains standard device/configuration descriptors, reports configuration interfaces/endpoints, and queries current-interface pipes. It does not issue custom control requests, endpoint reads/writes, reset, alternate-setting changes, driver replacement or service intervention. Failures retain Windows error codes, with no attempt to steal a claim. It closes WinUSB and file handles; a 30-second supervisor bounds blocking calls.

The script is descriptor-only, but not completely side-effect-free: Microsoft documents that [WinUsb_Initialize](https://learn.microsoft.com/en-us/windows/win32/api/winusb/nf-winusb-winusb_initialize) resets interface policy settings to defaults. This needs review before using it against a live runtime. Standard descriptor reads use [WinUsb_GetDescriptor](https://learn.microsoft.com/en-us/windows/win32/api/winusb/nf-winusb-winusb_getdescriptor); pipe metadata uses [WinUsb_QueryPipe](https://learn.microsoft.com/en-us/windows/win32/api/winusb/nf-winusb-winusb_querypipe). At report update time this script was syntax/layout checked locally and had not been run on albedo.
