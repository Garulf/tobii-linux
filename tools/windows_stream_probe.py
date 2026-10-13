#!/usr/bin/env python3
"""Bounded Windows research oracle for the installed Tobii Stream Engine.

Derived from REA evidence documented in docs/managed-api.md. This is not a Linux
backend. Defaults to STORE_OR_TRANSFER_TRUE because JSONL records are retained.
No calibration, firmware or persistent-device settings are changed.
"""
import argparse
import ctypes as C
import hashlib
import json
import math
import os
from pathlib import Path
import subprocess
import sys
import time

DLL_PATH = r"C:\Program Files\Tobii\Tobii EyeX\tobii_stream_engine.dll"
# Exact managed Constant table values, evidence/private/streamengine-constants.json.
KNOWN_DLL_SHA256 = "f60c90c41a6c29718f0255ab22db160b038f9305fe92d184c9cf3e8e938ec5a3"
NO_ERROR = 0
STORE_OR_TRANSFER_FALSE = 1
STORE_OR_TRANSFER_TRUE = 2


class Version(C.Structure):
    _fields_ = [(n, C.c_int32) for n in ("major", "minor", "revision", "build")]


class Vector2(C.Structure):
    _fields_ = [(n, C.c_float) for n in ("x", "y")]


class Vector3(C.Structure):
    _fields_ = [(n, C.c_float) for n in ("x", "y", "z")]


class GazePoint(C.Structure):
    _fields_ = [("timestamp_us", C.c_int64), ("validity", C.c_int32),
                ("position", Vector2)]


class HeadPose(C.Structure):
    _fields_ = [("timestamp_us", C.c_int64), ("position_validity", C.c_int32),
                ("position_xyz", Vector3), ("rotation_x_validity", C.c_int32),
                ("rotation_y_validity", C.c_int32), ("rotation_z_validity", C.c_int32),
                ("rotation_xyz", Vector3)]


URL_CALLBACK = C.CFUNCTYPE(None, C.c_char_p, C.c_void_p)
GAZE_CALLBACK = C.CFUNCTYPE(None, C.POINTER(GazePoint), C.c_void_p)
HEAD_CALLBACK = C.CFUNCTYPE(None, C.POINTER(HeadPose), C.c_void_p)


def emit(kind, **values):
    print(json.dumps({"kind": kind, **values}, allow_nan=False), flush=True)


def vector(value):
    return [float(getattr(value, n)) if math.isfinite(getattr(value, n)) else None
            for n, _ in value._fields_]


def layouts():
    """Default sequential layout follows observed managed TypeDef flags.

    No ClassLayout rows occur in the assembly, so no explicit pack override is
    declared. These offsets still require runtime/native corroboration.
    """
    result = {}
    for struct, expected_size, expected_offsets in (
        (Version, 16, [0, 4, 8, 12]),
        (GazePoint, 24, [0, 8, 12]),
        (HeadPose, 48, [0, 8, 12, 24, 28, 32, 36]),
    ):
        offsets = [getattr(struct, n).offset for n, _ in struct._fields_]
        if C.sizeof(struct) != expected_size or offsets != expected_offsets:
            raise RuntimeError(f"Unexpected ctypes ABI for {struct.__name__}")
        result[struct.__name__] = {"size": C.sizeof(struct), "offsets":
                                  dict(zip([n for n, _ in struct._fields_], offsets))}
    return result


def worker(args):
    if sys.platform != "win32" or C.sizeof(C.c_void_p) != 8:
        raise RuntimeError("This installed DLL probe requires 64-bit Windows Python")
    emit("abi", layouts=layouts(), status="managed-declaration-derived; native corroboration pending")
    path = Path(args.dll).resolve(strict=True)
    digest = hashlib.sha256(path.read_bytes()).hexdigest()
    emit("artifact", path=str(path), sha256=digest)
    if digest != KNOWN_DLL_SHA256:
        raise RuntimeError("DLL digest differs from the installed artifact selected for this investigation")
    # Keep directory handle alive until all native calls and cleanup complete.
    directory = os.add_dll_directory(str(path.parent))
    try:
        lib = C.CDLL(str(path))  # Observed cdecl imports, not WinDLL/stdcall.
        def bind(name, parameters, result=C.c_int32):
            fn = getattr(lib, name)
            fn.argtypes = parameters
            fn.restype = result
            return fn
        get_version = bind("tobii_get_api_version", [C.POINTER(Version)])
        error_message = bind("tobii_error_message", [C.c_int32], C.c_char_p)
        api_create = bind("tobii_api_create", [C.POINTER(C.c_void_p), C.c_void_p, C.c_void_p])
        api_destroy = bind("tobii_api_destroy", [C.c_void_p])
        enumerate_urls = bind("tobii_enumerate_local_device_urls", [C.c_void_p, URL_CALLBACK, C.c_void_p])
        device_create = bind("tobii_device_create", [C.c_void_p, C.c_char_p, C.c_int32, C.POINTER(C.c_void_p)])
        device_destroy = bind("tobii_device_destroy", [C.c_void_p])
        gaze_subscribe = bind("tobii_gaze_point_subscribe", [C.c_void_p, GAZE_CALLBACK, C.c_void_p])
        gaze_unsubscribe = bind("tobii_gaze_point_unsubscribe", [C.c_void_p])
        head_subscribe = bind("tobii_head_pose_subscribe", [C.c_void_p, HEAD_CALLBACK, C.c_void_p])
        head_unsubscribe = bind("tobii_head_pose_unsubscribe", [C.c_void_p])
        process = bind("tobii_device_process_callbacks", [C.c_void_p])

        def report(name, code, required=False):
            msg = error_message(code)
            emit("result", operation=name, code=int(code),
                 message=msg.decode("utf-8", "replace") if msg else None)
            if required and code != NO_ERROR:
                raise RuntimeError(f"{name} returned {code}")
            return code

        version = Version()
        report("get_api_version", get_version(C.byref(version)), True)
        emit("version", **{n: getattr(version, n) for n, _ in version._fields_})
        api, device = C.c_void_p(), C.c_void_p()
        subscriptions = []
        counts = {"gaze": 0, "head": 0}
        urls = []

        @URL_CALLBACK
        def on_url(url, _):
            if url:
                urls.append(bytes(url))

        @GAZE_CALLBACK
        def on_gaze(pointer, _):
            if pointer:
                value = pointer.contents
                counts["gaze"] += 1
                emit("gaze", timestamp_us=value.timestamp_us, validity=value.validity,
                     position=vector(value.position))

        @HEAD_CALLBACK
        def on_head(pointer, _):
            if pointer:
                value = pointer.contents
                counts["head"] += 1
                emit("head", timestamp_us=value.timestamp_us,
                     position_validity=value.position_validity, position=vector(value.position_xyz),
                     rotation_validity=[value.rotation_x_validity, value.rotation_y_validity,
                                        value.rotation_z_validity], rotation=vector(value.rotation_xyz))

        # Callback objects remain strongly referenced for the entire device lifetime.
        report("api_create", api_create(C.byref(api), None, None), True)
        try:
            report("enumerate_local_device_urls", enumerate_urls(api, on_url, None), True)
            for index, url in enumerate(urls):
                emit("device_url", index=index, url=url.decode("utf-8", "replace"))
            if not urls:
                raise RuntimeError("No Tobii device URL returned")
            if args.index >= len(urls):
                raise RuntimeError("Requested device index exceeds returned URL list")
            # This is a recorded research probe, so report the recording field of use.
            emit("field_of_use", value=STORE_OR_TRANSFER_TRUE,
                 declaration="TOBII_FIELD_OF_USE_STORE_OR_TRANSFER_TRUE")
            report("device_create", device_create(api, urls[args.index], STORE_OR_TRANSFER_TRUE,
                                                  C.byref(device)), True)
            try:
                if report("gaze_subscribe", gaze_subscribe(device, on_gaze, None)) == NO_ERROR:
                    subscriptions.append(("gaze_unsubscribe", gaze_unsubscribe))
                if report("head_subscribe", head_subscribe(device, on_head, None)) == NO_ERROR:
                    subscriptions.append(("head_unsubscribe", head_unsubscribe))
                if not subscriptions:
                    raise RuntimeError("Neither stream subscription succeeded")
                deadline = time.monotonic() + args.seconds
                calls = 0
                # Poll instead of the wait function whose blocking timeout is unknown.
                while time.monotonic() < deadline:
                    code = process(device)
                    calls += 1
                    if code != NO_ERROR:
                        report("device_process_callbacks", code, True)
                    time.sleep(0.005)
                emit("summary", counts=counts, process_calls=calls,
                     process_success_code=NO_ERROR, duration_seconds=args.seconds)
            finally:
                for name, unsubscribe in reversed(subscriptions):
                    report(name, unsubscribe(device))
                report("device_destroy", device_destroy(device))
        finally:
            report("api_destroy", api_destroy(api))
    finally:
        directory.close()
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dll", default=DLL_PATH)
    parser.add_argument("--seconds", type=float, default=5.0)
    parser.add_argument("--index", type=int, default=0)
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if not math.isfinite(args.seconds) or not 0 < args.seconds <= 60 or args.index < 0:
        parser.error("seconds must be in (0, 60]; index must be nonnegative")
    if args.worker:
        try:
            return worker(args)
        except Exception as exc:
            emit("error", message=str(exc))
            return 1
    # Bound setup/cleanup too if a native function blocks; child has inherited JSONL stdout.
    command = [sys.executable, str(Path(__file__).resolve()), "--worker", "--dll", args.dll,
               "--seconds", str(args.seconds), "--index", str(args.index)]
    try:
        return subprocess.run(command, timeout=args.seconds + 30).returncode
    except subprocess.TimeoutExpired:
        emit("error", message="Native probe exceeded deadline; child killed; graceful cleanup unconfirmed")
        return 124


if __name__ == "__main__":
    raise SystemExit(main())
