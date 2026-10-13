"""Shared offline matrix emulation with explicit allocator stubs and GS stack limit."""
import struct
from unicorn import UC_HOOK_CODE
from unicorn.x86_const import *
from ttp_encoder_oracle import machine, DATA, STACK, STOP


class MatrixMachine:
    def __init__(self, binary):
        self.vm = machine(binary)
        self.vm.mem_map(0x64000000, 0x1000)
        self.vm.reg_write(UC_X86_REG_GS_BASE, 0x64000000)
        self.vm.mem_write(0x64000010, struct.pack('<Q', STACK))
        self.vm.mem_map(0x65000000, 0x1000000)
        self.cursor = 0x65000000
        self.calls = {}
        for address in (0x1403e53e8, 0x1403e4f1c, 0x1403b6030, 0x1403b6808, 0x1403b59c4):
            self.vm.hook_add(UC_HOOK_CODE, self.stub, begin=address, end=address)

    def allocate(self, size):
        pointer = self.cursor
        self.cursor += (size + 63) & ~63
        if size < 0 or self.cursor >= 0x66000000:
            raise RuntimeError('Matrix allocation bound exceeded')
        return pointer

    def stub(self, vm, address, size, _):
        self.calls[hex(address)] = self.calls.get(hex(address), 0) + 1
        result = 0
        if address not in (0x1403e4f1c, 0x1403b59c4):
            result = self.allocate(vm.reg_read(UC_X86_REG_RCX))
        stack = vm.reg_read(UC_X86_REG_RSP)
        ret = struct.unpack('<Q', vm.mem_read(stack, 8))[0]
        vm.reg_write(UC_X86_REG_RAX, result)
        vm.reg_write(UC_X86_REG_RSP, stack + 8)
        vm.reg_write(UC_X86_REG_RIP, ret)

    def matrix(self, rows, columns, values):
        if len(values) != rows * columns:
            raise ValueError('Matrix extent mismatch')
        data = self.allocate(len(values) * 4)
        self.vm.mem_write(data, struct.pack('<'+'f'*len(values), *values))
        header = self.allocate(40)
        self.vm.mem_write(header, struct.pack('<iiQiiQii', 0x42424005, columns*4, 0, 0, 0, data, rows, columns))
        return header, data

    def call(self, address, *arguments, instruction_limit=5_000_000, timeout_us=10_000_000):
        stack = STACK + 0xfef8
        self.vm.mem_write(stack, struct.pack('<Q', STOP))
        self.vm.reg_write(UC_X86_REG_RSP, stack)
        for i, value in enumerate(arguments):
            if i < 4:
                self.vm.reg_write((UC_X86_REG_RCX, UC_X86_REG_RDX, UC_X86_REG_R8, UC_X86_REG_R9)[i], value)
            else:
                self.vm.mem_write(stack + 0x28 + (i-4)*8, struct.pack('<Q', value))
        self.vm.emu_start(address, STOP, timeout=timeout_us, count=instruction_limit)
        if self.vm.reg_read(UC_X86_REG_RIP) != STOP:
            raise RuntimeError(f'Matrix function did not return: RIP={self.vm.reg_read(UC_X86_REG_RIP):#x}, instruction_limit={instruction_limit}')

    def floats(self, data, count):
        return struct.unpack('<'+'f'*count, self.vm.mem_read(data, count*4))
