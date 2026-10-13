#!/usr/bin/env python3
"""Read the configured head-pose geometry from the installed Stream Engine.

Windows research oracle only. Calls getters and never a setter: display area,
geometry mounting and track box, followed by a short head-pose capture so the
geometry and samples come from the same session. Structures whose layout is
not recovered are read into an oversized zeroed buffer and reported as raw
32-bit words.
"""
import argparse
import ctypes as C
import math
import struct
import subprocess
import sys
import time
from pathlib import Path

from windows_stream_probe import (DLL_PATH, HEAD_CALLBACK, KNOWN_DLL_SHA256, NO_ERROR,
                                  STORE_OR_TRANSFER_TRUE, URL_CALLBACK, Vector3, emit, vector)

RAW_BUFFER_BYTES = 512


class DisplayArea(C.Structure):
    _fields_ = [("top_left_mm", Vector3), ("top_right_mm", Vector3), ("bottom_left_mm", Vector3)]


def raw_words(buffer):
    data = bytes(buffer)
    end = len(data.rstrip(b"\0"))
    count = (end + 3) // 4
    return {"floats": [f if math.isfinite(f) else None for f in struct.unpack_from(f"<{count}f", data)],
            "ints": list(struct.unpack_from(f"<{count}i", data))}


def worker(args):
    import hashlib
    import os
    path = Path(args.dll).resolve(strict=True)
    digest = hashlib.sha256(path.read_bytes()).hexdigest()
    emit("artifact", path=str(path), sha256=digest)
    if digest != KNOWN_DLL_SHA256:
        raise RuntimeError("DLL digest differs from the installed artifact selected for this investigation")
    directory = os.add_dll_directory(str(path.parent))
    try:
        lib = C.CDLL(str(path))

        def bind(name, parameters, result=C.c_int32):
            fn = getattr(lib, name)
            fn.argtypes, fn.restype = parameters, result
            return fn
        error_message = bind("tobii_error_message", [C.c_int32], C.c_char_p)
        api_create = bind("tobii_api_create", [C.POINTER(C.c_void_p), C.c_void_p, C.c_void_p])
        api_destroy = bind("tobii_api_destroy", [C.c_void_p])
        enumerate_urls = bind("tobii_enumerate_local_device_urls", [C.c_void_p, URL_CALLBACK, C.c_void_p])
        device_create = bind("tobii_device_create", [C.c_void_p, C.c_char_p, C.c_int32, C.POINTER(C.c_void_p)])
        device_destroy = bind("tobii_device_destroy", [C.c_void_p])
        get_display_area = bind("tobii_get_display_area", [C.c_void_p, C.POINTER(DisplayArea)])
        get_mounting = bind("tobii_get_geometry_mounting", [C.c_void_p, C.c_void_p])
        get_track_box = bind("tobii_get_track_box", [C.c_void_p, C.c_void_p])
        head_subscribe = bind("tobii_head_pose_subscribe", [C.c_void_p, HEAD_CALLBACK, C.c_void_p])
        head_unsubscribe = bind("tobii_head_pose_unsubscribe", [C.c_void_p])
        process = bind("tobii_device_process_callbacks", [C.c_void_p])

        def report(name, code, required=False):
            message = error_message(code)
            emit("result", operation=name, code=int(code),
                 message=message.decode("utf-8", "replace") if message else None)
            if required and code != NO_ERROR:
                raise RuntimeError(f"{name} returned {code}")
            return code

        api, device, urls = C.c_void_p(), C.c_void_p(), []

        @URL_CALLBACK
        def on_url(url, _):
            if url:
                urls.append(bytes(url))

        @HEAD_CALLBACK
        def on_head(pointer, _):
            if pointer:
                value = pointer.contents
                emit("head", timestamp_us=value.timestamp_us, position_validity=value.position_validity,
                     position=vector(value.position_xyz),
                     rotation_validity=[value.rotation_x_validity, value.rotation_y_validity,
                                        value.rotation_z_validity],
                     rotation=vector(value.rotation_xyz))

        report("api_create", api_create(C.byref(api), None, None), True)
        try:
            report("enumerate_local_device_urls", enumerate_urls(api, on_url, None), True)
            if not urls:
                raise RuntimeError("No Tobii device URL returned")
            report("device_create", device_create(api, urls[0], STORE_OR_TRANSFER_TRUE, C.byref(device)), True)
            try:
                area = DisplayArea()
                if report("get_display_area", get_display_area(device, C.byref(area))) == NO_ERROR:
                    emit("display_area", **{n: vector(getattr(area, n)) for n, _ in area._fields_})
                for name, getter in (("geometry_mounting", get_mounting), ("track_box", get_track_box)):
                    buffer = C.create_string_buffer(RAW_BUFFER_BYTES)
                    if report("get_" + name, getter(device, buffer)) == NO_ERROR:
                        emit(name, raw=raw_words(buffer), buffer_bytes=RAW_BUFFER_BYTES)
                if args.seconds > 0 and report("head_subscribe", head_subscribe(device, on_head, None)) == NO_ERROR:
                    try:
                        deadline = time.monotonic() + args.seconds
                        while time.monotonic() < deadline:
                            report_code = process(device)
                            if report_code != NO_ERROR:
                                report("device_process_callbacks", report_code, True)
                            time.sleep(0.005)
                    finally:
                        report("head_unsubscribe", head_unsubscribe(device))
            finally:
                report("device_destroy", device_destroy(device))
        finally:
            report("api_destroy", api_destroy(api))
    finally:
        directory.close()
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dll", default=DLL_PATH)
    parser.add_argument("--seconds", type=float, default=2.0)
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if not math.isfinite(args.seconds) or not 0 <= args.seconds <= 30:
        parser.error("seconds must be in [0, 30]")
    if args.worker:
        try:
            return worker(args)
        except Exception as exc:
            emit("error", message=str(exc))
            return 1
    command = [sys.executable, str(Path(__file__).resolve()), "--worker", "--dll", args.dll,
               "--seconds", str(args.seconds)]
    try:
        return subprocess.run(command, timeout=args.seconds + 30).returncode
    except subprocess.TimeoutExpired:
        emit("error", message="Native probe exceeded deadline; child killed; graceful cleanup unconfirmed")
        return 124


if __name__ == "__main__":
    raise SystemExit(main())
