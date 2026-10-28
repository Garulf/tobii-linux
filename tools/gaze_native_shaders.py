#!/usr/bin/env python3
"""Extract the compiled D3D11 shaders embedded in GazeNative.dll.

Finds every DXBC container by its magic, takes its length from the total-size
field of the DXBC header, and reads the shader kind from the program-type
field of the SHDR or SHEX chunk. Writes each blob as <index>-<kind>.dxbc in
file order and prints its file offset, virtual address, size and SHA-256.
The virtual address is what GazeNative passes to CreateVertexShader or
CreatePixelShader (docs/gaze-preview.md). Nothing is executed.
"""
import argparse
import hashlib
from pathlib import Path
import struct

ROOT = Path(__file__).resolve().parents[1]
DLL = ROOT/'evidence/private/experience/portal/app/Tobii.EyeTracking.Portal.WPF/GazeNative.dll'
OUTPUT = ROOT/'evidence/private/experience/shaders'

MAGIC = b'DXBC'
HEADER = struct.Struct('<4s16sIII')
PROGRAM_KINDS = {0: 'ps', 1: 'vs', 2: 'gs', 3: 'hs', 4: 'ds', 5: 'cs'}


def section_table(data):
    pe = struct.unpack_from('<I', data, 0x3c)[0]
    count = struct.unpack_from('<H', data, pe + 6)[0]
    optional_size = struct.unpack_from('<H', data, pe + 20)[0]
    base = struct.unpack_from('<Q', data, pe + 48)[0]
    table = pe + 24 + optional_size
    sections = [struct.unpack_from('<8xIIII', data, table + 40*i) for i in range(count)]
    return base, sections


def virtual_address(data, offset):
    base, sections = section_table(data)
    for _, rva, raw_size, raw in sections:
        if raw <= offset < raw + raw_size:
            return base + rva + offset - raw
    return None


def shader_kind(blob):
    chunk_count = HEADER.unpack_from(blob)[4]
    for index in range(chunk_count):
        chunk = struct.unpack_from('<I', blob, HEADER.size + 4*index)[0]
        tag = blob[chunk:chunk + 4]
        if tag in (b'SHDR', b'SHEX'):
            version = struct.unpack_from('<I', blob, chunk + 8)[0]
            return PROGRAM_KINDS.get(version >> 16, f'type{version >> 16}')
    return 'unknown'


def dxbc_blobs(data):
    start = data.find(MAGIC)
    while start != -1:
        magic, _, _, size, _ = HEADER.unpack_from(data, start)
        blob = data[start:start + size]
        yield start, blob
        start = data.find(MAGIC, start + max(size, len(MAGIC)))


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('dll', nargs='?', type=Path, default=DLL)
    parser.add_argument('output', nargs='?', type=Path, default=OUTPUT)
    args = parser.parse_args()
    data = args.dll.read_bytes()
    args.output.mkdir(parents=True, exist_ok=True)
    for index, (offset, blob) in enumerate(dxbc_blobs(data)):
        name = f'{index}-{shader_kind(blob)}.dxbc'
        (args.output/name).write_bytes(blob)
        address = virtual_address(data, offset)
        where = f'{address:#x}' if address is not None else 'unmapped'
        print(f'{name}  offset {offset:#x}  va {where}  size {len(blob):#x}  sha256 {hashlib.sha256(blob).hexdigest()}')


if __name__ == '__main__':
    main()
