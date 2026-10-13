#!/usr/bin/env python3
"""Exploratory full image-warp probe; not a passing parity check."""
from pathlib import Path
import struct
from unicorn import UC_HOOK_MEM_INVALID
from unicorn.x86_const import UC_X86_REG_RIP
from vsmat_oracle import MatrixMachine
ROOT=Path(__file__).resolve().parents[1]
m=MatrixMachine((ROOT/'evidence/private/binaries/platform_runtime_IS5LEYETRACKER5_service.exe').read_bytes())
m.call(0x1400011b0)  # Original initializer for aligned interpolation table storage.
print('warp worker',hex(struct.unpack('<Q',m.vm.mem_read(0x141135468,8))[0]),flush=True)
def u8matrix(rows,cols,pixels):
    data=m.allocate(len(pixels));m.vm.mem_write(data,pixels)
    header=m.allocate(40);m.vm.mem_write(header,struct.pack('<iiQiiQii',0x42424000,cols,0,0,0,data,rows,cols))
    return header,data
pixels=bytes((x+3*y)%256 for y in range(120) for x in range(160))
src,_=u8matrix(120,160,pixels);dst,data=u8matrix(96,96,bytes(96*96))
t,_=m.matrix(2,3,[1,0,0,0,1,0]);border=m.allocate(32)
def invalid(vm,access,address,size,value,_):
    print('fault',hex(vm.reg_read(UC_X86_REG_RIP)),hex(address),access,flush=True);return False
m.vm.hook_add(UC_HOOK_MEM_INVALID,invalid)
m.call(0x1400b45b0,src,dst,t,0,border)
result=bytes(m.vm.mem_read(data,96*96));expected=b''.join(pixels[y*160:y*160+96] for y in range(96))
print('identity match',result==expected,'range',min(result),max(result),'calls',m.calls)
