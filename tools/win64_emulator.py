"""Minimal Win64 user-mode emulation of a hash-bound PE image in Unicorn.

Maps the image at its preferred base, points every import address table entry
at a private stub, and answers the few kernel32 calls a codec needs (thread
local storage, one-time initialization, process heap) in Python. Any other
import stops the emulation with its name, so a run never silently depends on
an unanswered Windows call. Python callables can be handed to native code as
function pointers through `callback`.
"""
import hashlib
from pathlib import Path
import struct

from unicorn import Uc, UC_ARCH_X86, UC_MODE_64, UC_HOOK_CODE
from unicorn.x86_const import (UC_X86_REG_RAX, UC_X86_REG_RCX, UC_X86_REG_RDX, UC_X86_REG_R8,
                               UC_X86_REG_R9, UC_X86_REG_RSP, UC_X86_REG_RIP)

STACK, STACK_SIZE = 0x60000000, 0x100000
HEAP, HEAP_SIZE = 0x70000000, 0x1000000
STUBS, STUB_SIZE = 0x7f000000, 0x10000
STOP = STUBS
ARGUMENT_REGISTERS = (UC_X86_REG_RCX, UC_X86_REG_RDX, UC_X86_REG_R8, UC_X86_REG_R9)


class UnansweredImport(RuntimeError):
    pass


def _sections(binary):
    pe = struct.unpack_from('<I', binary, 0x3c)[0]
    count = struct.unpack_from('<H', binary, pe + 6)[0]
    optional_size = struct.unpack_from('<H', binary, pe + 20)[0]
    optional = pe + 24
    table = optional + optional_size
    return pe, optional, [struct.unpack_from('<8sIIII', binary, table + 40*i) for i in range(count)]


def _file_offset(sections, rva):
    for _, virtual_size, address, raw_size, raw in sections:
        if address <= rva < address + max(virtual_size, raw_size):
            return raw + rva - address
    raise ValueError(f'RVA {rva:#x} is not mapped')


def _imports(binary, sections, optional):
    directory = struct.unpack_from('<I', binary, optional + 112 + 8)[0]
    offset = _file_offset(sections, directory)

    def c_string(rva):
        start = _file_offset(sections, rva)
        return binary[start:binary.index(b'\0', start)].decode()

    while True:
        lookup, _, _, name, address = struct.unpack_from('<IIIII', binary, offset)
        if not name:
            return
        table = _file_offset(sections, lookup or address)
        for index in range(1 << 16):
            entry = struct.unpack_from('<Q', binary, table + 8*index)[0]
            if not entry:
                break
            symbol = f'#{entry & 0xffff}' if entry >> 63 else c_string((entry & 0xffffffff) + 2)
            yield symbol, address + 8*index
        offset += 20


class Win64Emulator:
    def __init__(self, path, sha256):
        binary = Path(path).read_bytes()
        if hashlib.sha256(binary).hexdigest() != sha256:
            raise ValueError(f'{path} is not the researched build')
        pe, optional, sections = _sections(binary)
        self.base = struct.unpack_from('<Q', binary, optional + 24)[0]
        image_size = struct.unpack_from('<I', binary, optional + 56)[0]
        self.vm = Uc(UC_ARCH_X86, UC_MODE_64)
        self.vm.mem_map(self.base, image_size + 0xfff & ~0xfff)
        self.vm.mem_write(self.base, binary[:0x1000])
        for _, _, rva, raw_size, raw in sections:
            if raw_size:
                self.vm.mem_write(self.base + rva, binary[raw:raw + raw_size])
        for address, size in ((STACK, STACK_SIZE), (HEAP, HEAP_SIZE), (STUBS, STUB_SIZE)):
            self.vm.mem_map(address, size)
        self.vm.mem_write(STUBS, b'\xc3' * STUB_SIZE)
        self.handlers = {}
        self.next_stub = STUBS + 0x10
        self.heap_next = HEAP
        self.heap_sizes = {}
        self.tls = {}
        answers = {
            'GetProcessHeap': lambda args: 0x1000,
            'HeapAlloc': lambda args: self.allocate(args[2], zero=bool(args[1] & 8)),
            'HeapFree': lambda args: 1,
            'HeapReAlloc': lambda args: self.reallocate(args[2], args[3]),
            'HeapSize': lambda args: self.heap_sizes.get(args[2], 0),
            'TlsAlloc': lambda args: 1,
            'TlsSetValue': lambda args: self.tls.__setitem__(args[0], args[1]) or 1,
            'TlsGetValue': lambda args: self.tls.get(args[0], 0),
            'InitOnceExecuteOnce': self._init_once,
        }
        for symbol, rva in _imports(binary, sections, optional):
            stub = self._stub(answers.get(symbol, self._unanswered(symbol)))
            self.vm.mem_write(self.base + rva, struct.pack('<Q', stub))
        self.vm.hook_add(UC_HOOK_CODE, self._on_stub, begin=STUBS + 1, end=STUBS + STUB_SIZE - 1)

    @staticmethod
    def _unanswered(symbol):
        def fail(args):
            raise UnansweredImport(symbol)
        return fail

    def _stub(self, handler):
        address = self.next_stub
        self.next_stub += 0x10
        self.handlers[address] = handler
        return address

    def callback(self, function):
        """Function pointer that runs `function(args)` and returns its result."""
        return self._stub(lambda args: function(args) or 0)

    def _arguments(self):
        rsp = self.vm.reg_read(UC_X86_REG_RSP)
        stacked = struct.unpack('<8Q', self.vm.mem_read(rsp + 0x28, 64))
        return [self.vm.reg_read(register) for register in ARGUMENT_REGISTERS] + list(stacked)

    def _on_stub(self, vm, address, size, data):
        handler = self.handlers.get(address)
        if handler is None:
            return
        try:
            result = handler(self._arguments())
        except Exception as error:
            self.failure = error
            vm.emu_stop()
            return
        if result is not None:
            vm.reg_write(UC_X86_REG_RAX, result & 0xffffffffffffffff)

    def _init_once(self, args):
        once, function, parameter, context = args[:4]
        if self.vm.mem_read(once, 8) != b'\0' * 8:
            return 1
        self.vm.mem_write(once, struct.pack('<Q', 2))
        for register, value in ((UC_X86_REG_RCX, once), (UC_X86_REG_RDX, parameter),
                                (UC_X86_REG_R8, context), (UC_X86_REG_RIP, function)):
            self.vm.reg_write(register, value)
        return None

    def allocate(self, size, zero=True):
        address = self.heap_next
        self.heap_next += size + 0x2f & ~0xf
        if self.heap_next > HEAP + HEAP_SIZE:
            raise MemoryError('emulated heap exhausted')
        if zero:
            self.vm.mem_write(address, b'\0' * size)
        self.heap_sizes[address] = size
        return address

    def reallocate(self, address, size):
        moved = self.allocate(size)
        if address:
            old = self.heap_sizes.get(address, 0)
            self.vm.mem_write(moved, bytes(self.vm.mem_read(address, min(old, size))))
        return moved

    def put(self, raw):
        address = self.allocate(len(raw) or 1, zero=False)
        self.vm.mem_write(address, raw)
        return address

    def read(self, address, size):
        return bytes(self.vm.mem_read(address, size))

    def call(self, entry, *args):
        rsp = STACK + STACK_SIZE - 0x1000
        self.vm.mem_write(rsp, struct.pack('<Q', STOP))
        for index, value in enumerate(args[4:]):
            self.vm.mem_write(rsp + 0x28 + 8*index, struct.pack('<Q', value & 0xffffffffffffffff))
        for register, value in zip(ARGUMENT_REGISTERS, args):
            self.vm.reg_write(register, value & 0xffffffffffffffff)
        self.vm.reg_write(UC_X86_REG_RSP, rsp)
        self.failure = None
        self.vm.emu_start(entry, STOP, timeout=20_000_000, count=5_000_000)
        if self.failure:
            raise self.failure
        if self.vm.reg_read(UC_X86_REG_RIP) != STOP:
            raise RuntimeError(f'{entry:#x} did not return')
        return self.vm.reg_read(UC_X86_REG_RAX)
