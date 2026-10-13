#!/usr/bin/env python3
"""Query standard descriptors on the explicitly selected installed ET5 interface.

No vendor requests, endpoint transfers, reset, alternate-setting changes, service
control, or claim stealing. WinUsb_Initialize itself resets interface policy
settings to defaults (Microsoft documentation), so this is not a zero-effect
snapshot. Run only after reviewing that behavior for the active device.

Sources:
https://learn.microsoft.com/en-us/windows/win32/api/winusb/nf-winusb-winusb_initialize
https://learn.microsoft.com/en-us/windows/win32/api/winusb/nf-winusb-winusb_getdescriptor
https://learn.microsoft.com/en-us/windows/win32/api/winusb/nf-winusb-winusb_querypipe
https://learn.microsoft.com/en-us/windows/win32/api/winusbio/ns-winusbio-winusb_pipe_information
"""
import argparse
import ctypes as C
import hashlib
import json
import struct
import subprocess
import sys
from pathlib import Path

EXPECTED_PATH = r"\\?\usb#vid_2104&pid_0313&mi_00#a&1090bf2&0&0000#{85c0f97c-e2b1-422a-92a9-5f96072e79d8}"
U8, U16, U32, BOOL, HANDLE = C.c_uint8, C.c_uint16, C.c_uint32, C.c_int32, C.c_void_p


class InterfaceDescriptor(C.Structure):
    _pack_ = 1
    _fields_ = [(name, U8) for name in (
        "bLength", "bDescriptorType", "bInterfaceNumber", "bAlternateSetting",
        "bNumEndpoints", "bInterfaceClass", "bInterfaceSubClass", "bInterfaceProtocol", "iInterface")]


class PipeInformation(C.Structure):
    _fields_ = [("PipeType", C.c_int32), ("PipeId", U8),
                ("MaximumPacketSize", U16), ("Interval", U8)]


def emit(kind, **data):
    print(json.dumps({"kind": kind, **data}), flush=True)


def fields(value):
    return {name: getattr(value, name) for name, _ in value._fields_}


def parse_configuration(data):
    """Preserve every standard/unknown descriptor; decode interfaces/endpoints."""
    if len(data) < 9 or data[1] != 2:
        raise ValueError("Invalid configuration descriptor header")
    total = struct.unpack_from("<H", data, 2)[0]
    if total != len(data):
        raise ValueError(f"Configuration length mismatch: declared {total}, returned {len(data)}")
    result = []
    offset = 0
    current_interface = None
    while offset < total:
        if offset + 2 > total:
            raise ValueError("Incomplete descriptor header")
        length, kind = data[offset:offset + 2]
        if length < 2 or offset + length > total:
            raise ValueError("Invalid descriptor extent")
        raw = data[offset:offset + length]
        entry = {"offset": offset, "type": kind, "raw_hex": raw.hex()}
        if kind == 4:
            if length != 9:
                raise ValueError("Unexpected interface descriptor size")
            descriptor = InterfaceDescriptor.from_buffer_copy(raw)
            entry.update(fields(descriptor))
            current_interface = {"number": descriptor.bInterfaceNumber,
                                 "alternate_setting": descriptor.bAlternateSetting}
        elif kind == 5:
            if length < 7:
                raise ValueError("Short endpoint descriptor")
            entry.update(interface=current_interface, address=raw[2], attributes=raw[3],
                         maximum_packet_size=struct.unpack_from("<H", raw, 4)[0],
                         interval=raw[6], direction="in" if raw[2] & 0x80 else "out")
        result.append(entry)
        offset += length
    return result


def worker(path):
    if sys.platform != "win32":
        raise RuntimeError("This probe requires Windows")
    if path.casefold() != EXPECTED_PATH.casefold():
        raise ValueError("Device path differs from the exact ET5 interface selected for this investigation")
    if C.sizeof(InterfaceDescriptor) != 9 or C.sizeof(PipeInformation) != 12:
        raise RuntimeError("Unexpected ctypes structure ABI")
    emit("scope", device_path=path, access="GENERIC_READ; shared read/write", operations="standard descriptors and current-interface pipe queries",
         initialization_effect="WinUsb_Initialize resets interface policies to defaults")
    kernel = C.WinDLL("kernel32", use_last_error=True)
    usb = C.WinDLL("winusb", use_last_error=True)

    def bind(lib, name, args, result=BOOL):
        fn = getattr(lib, name)
        fn.argtypes, fn.restype = args, result
        return fn

    create = bind(kernel, "CreateFileW", [C.c_wchar_p, U32, U32, C.c_void_p, U32, U32, HANDLE], HANDLE)
    close = bind(kernel, "CloseHandle", [HANDLE])
    initialize = bind(usb, "WinUsb_Initialize", [HANDLE, C.POINTER(HANDLE)])
    free = bind(usb, "WinUsb_Free", [HANDLE])
    descriptor = bind(usb, "WinUsb_GetDescriptor", [HANDLE, U8, U8, U16, C.POINTER(U8), U32, C.POINTER(U32)])
    current_alt = bind(usb, "WinUsb_GetCurrentAlternateSetting", [HANDLE, C.POINTER(U8)])
    interface = bind(usb, "WinUsb_QueryInterfaceSettings", [HANDLE, U8, C.POINTER(InterfaceDescriptor)])
    pipe = bind(usb, "WinUsb_QueryPipe", [HANDLE, U8, U8, C.POINTER(PipeInformation)])

    def check(name, ok, fatal=True):
        code = 0 if ok else C.get_last_error()
        emit("result", operation=name, success=bool(ok), windows_error=code,
             message=None if ok else C.FormatError(code).strip())
        if not ok and fatal:
            raise OSError(code, f"{name} failed; no access escalation or service intervention attempted")
        return bool(ok)

    handle = create(path, 0x80000000, 3, None, 3, 0x40000000, None)
    check("CreateFileW", handle not in (None, C.c_void_p(-1).value))
    usb_handle = HANDLE()
    try:
        check("WinUsb_Initialize", initialize(handle, C.byref(usb_handle)))
        try:
            def get_descriptor(kind, length):
                buffer = (U8 * length)()
                transferred = U32()
                check(f"WinUsb_GetDescriptor({kind})", descriptor(usb_handle, kind, 0, 0,
                                                                buffer, length, C.byref(transferred)))
                if transferred.value > length:
                    raise ValueError("Invalid returned descriptor length")
                data = bytes(buffer[:transferred.value])
                emit("raw_descriptor", descriptor_type=kind, length=len(data),
                     sha256=hashlib.sha256(data).hexdigest(), hex=data.hex())
                return data

            device = get_descriptor(1, 18)
            if len(device) != 18 or device[:2] != b"\x12\x01":
                raise ValueError("Invalid device descriptor")
            vendor, product = struct.unpack_from("<HH", device, 8)
            emit("device", vendor_id=vendor, product_id=product,
                 usb_version_bcd=struct.unpack_from("<H", device, 2)[0],
                 device_version_bcd=struct.unpack_from("<H", device, 12)[0],
                 device_class=device[4], configurations=device[17])
            if (vendor, product) != (0x2104, 0x0313):
                raise ValueError("Device descriptor does not identify the selected ET5")
            # Full maximum descriptor buffer avoids partial-header GetDescriptor behavior.
            configuration = get_descriptor(2, 65535)
            for entry in parse_configuration(configuration):
                emit("configuration_entry", **entry)
            alternate = U8()
            check("WinUsb_GetCurrentAlternateSetting", current_alt(usb_handle, C.byref(alternate)))
            desc = InterfaceDescriptor()
            check("WinUsb_QueryInterfaceSettings", interface(usb_handle, alternate.value, C.byref(desc)))
            emit("current_interface", **fields(desc))
            if desc.bInterfaceNumber != 0:
                raise ValueError("WinUSB returned a different interface than MI_00")
            for index in range(desc.bNumEndpoints):
                info = PipeInformation()
                check(f"WinUsb_QueryPipe({index})", pipe(usb_handle, alternate.value, index, C.byref(info)))
                emit("pipe", index=index, alternate_setting=alternate.value, **fields(info))
        finally:
            check("WinUsb_Free", free(usb_handle), fatal=False)
    finally:
        check("CloseHandle", close(handle), fatal=False)
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--device-path", required=True, help="Exact ET5 device interface symbolic link")
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.worker:
        try:
            return worker(args.device_path)
        except Exception as exc:
            emit("error", message=str(exc))
            return 1
    try:
        return subprocess.run([sys.executable, str(Path(__file__).resolve()), "--worker",
                               "--device-path", args.device_path], timeout=30).returncode
    except subprocess.TimeoutExpired:
        emit("error", message="Descriptor probe timed out; child killed; graceful cleanup unconfirmed")
        return 124


if __name__ == "__main__":
    raise SystemExit(main())
