#!/usr/bin/env python3
"""Static navigation helpers for the hash-bound Windows runtime, no Ghidra analysis.

  function ADDR...          containing function from .pdata (follows chained unwind)
  callers ADDR...           direct E8/E9 rel32 callers and raw qword references
  strings TEXT...           string addresses and rip-relative lea references
  peek {s,f,d,i,q} ADDR...  read a C string, 4 floats, 2 doubles, 4 uint32 or 8 qwords
  rtti COL                  class name for a complete-object locator address
"""
import bisect
import re
import struct
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
BINARY = ROOT/'evidence/private/binaries/platform_runtime_IS5LEYETRACKER5_service.exe'


class Image:
    def __init__(self, path=BINARY):
        self.data = path.read_bytes()
        pe = struct.unpack_from('<I', self.data, 0x3c)[0]
        count = struct.unpack_from('<H', self.data, pe + 6)[0]
        optional_size = struct.unpack_from('<H', self.data, pe + 20)[0]
        self.base = struct.unpack_from('<Q', self.data, pe + 48)[0]
        table = pe + 24 + optional_size
        self.sections = []
        for index in range(count):
            name, virtual_size, rva, raw_size, raw = struct.unpack_from('<8sIIII', self.data, table + 40*index)
            self.sections.append((name.rstrip(b'\0').decode(), virtual_size, rva, raw_size, raw))
        exception_rva, exception_size = struct.unpack_from('<II', self.data, pe + 24 + 112 + 3*8)
        offset = self.offset_of_rva(exception_rva)
        self.unwind = sorted(struct.unpack_from('<III', self.data, offset + 12*i)
                             for i in range(exception_size // 12))
        self.starts = [entry[0] for entry in self.unwind]

    def offset_of_rva(self, rva):
        for _, virtual_size, start, raw_size, raw in self.sections:
            if start <= rva < start + max(virtual_size, raw_size):
                return raw + rva - start
        raise ValueError(f'RVA {rva:#x} is not mapped')

    def offset(self, address):
        return self.offset_of_rva(address - self.base)

    def section(self, wanted):
        for name, _, rva, raw_size, raw in self.sections:
            if name == wanted:
                return self.base + rva, self.data[raw:raw + raw_size]
        raise KeyError(wanted)

    def function(self, address):
        rva = address - self.base
        index = bisect.bisect_right(self.starts, rva) - 1
        begin, end, unwind = self.unwind[index]
        if not begin <= rva < end:
            return None
        while True:
            info = self.offset_of_rva(unwind)
            codes = self.data[info + 2]
            if not self.data[info] >> 3 & 4:
                return self.base + begin
            begin, end, unwind = struct.unpack_from('<III', self.data, info + 4 + ((codes + 1) & ~1)*2)


def main(argv):
    if len(argv) < 2:
        raise SystemExit(__doc__)
    image, command, values = Image(), argv[0], argv[1:]
    if command == 'function':
        for value in values:
            found = image.function(int(value, 16))
            print(value, '->', hex(found) if found else 'no pdata')
    elif command == 'callers':
        targets = {int(value, 16) for value in values}
        text_base, text = image.section('.text')
        for i in range(len(text) - 5):
            if text[i] in (0xe8, 0xe9):
                source = text_base + i
                target = source + 5 + struct.unpack_from('<i', text, i + 1)[0]
                if target in targets:
                    print(hex(target), '<-', hex(source), 'call' if text[i] == 0xe8 else 'jmp',
                          'in', hex(image.function(source) or 0))
        for name, _, rva, raw_size, raw in image.sections:
            blob = image.data[raw:raw + raw_size]
            for target in targets:
                for match in re.finditer(re.escape(struct.pack('<Q', target)), blob):
                    print(hex(target), 'qword in', name, hex(image.base + rva + match.start()))
    elif command == 'strings':
        text_base, text = image.section('.text')
        for value in values:
            for match in re.finditer(re.escape(value.encode()), image.data):
                start = match.start()
                while start and image.data[start - 1]:
                    start -= 1
                rva = next(r + start - raw for _, _, r, size, raw in image.sections if raw <= start < raw + size)
                address = image.base + rva
                print(repr(image.data[start:image.data.index(b'\0', start)][:100]), hex(address))
                for i in range(len(text) - 7):
                    if text[i + 1] == 0x8d and text[i] in (0x48, 0x4c) and text[i + 2] & 0xc7 == 5:
                        source = text_base + i
                        if source + 7 + struct.unpack_from('<i', text, i + 3)[0] == address:
                            print('   lea at', hex(source), 'in', hex(image.function(source) or 0))
    elif command == 'peek':
        kind, formats = values[0], {'f': '<4f', 'd': '<2d', 'i': '<4I', 'q': '<8Q'}
        for value in values[1:]:
            offset = image.offset(int(value, 16))
            if kind == 's':
                print(value, repr(image.data[offset:image.data.index(b'\0', offset)][:200]))
            else:
                print(value, [hex(v) if kind == 'q' else v for v in struct.unpack_from(formats[kind], image.data, offset)])
    elif command == 'rtti':
        locator = image.offset(int(values[0], 16))
        type_rva = struct.unpack_from('<6I', image.data, locator)[3]
        name = image.offset_of_rva(type_rva) + 16
        print(image.data[name:image.data.index(b'\0', name)].decode())
    else:
        raise SystemExit(__doc__)


if __name__ == '__main__':
    main(sys.argv[1:])
