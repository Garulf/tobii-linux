#!/usr/bin/env python3
"""Compare the recovered D1 crop normalization with original VNN code."""
import hashlib
import json
from pathlib import Path
import struct
from unicorn.x86_const import *
from ttp_encoder_oracle import machine, DATA, STACK, STOP, SHA256
from tobii_linux.d1 import prepare_crop_tensor
ROOT=Path(__file__).resolve().parents[1]


def original(binary,pixels):
    vm=machine(binary);output=DATA+0x4000;vm.mem_write(DATA,pixels)
    rsp=STACK+0xfef8;vm.mem_write(rsp,struct.pack('<Q',STOP))
    for off,value in [(0x28,1),(0x30,1),(0x38,0)]:vm.mem_write(rsp+off,struct.pack('<Q',value))
    for reg,value in [(UC_X86_REG_RCX,DATA),(UC_X86_REG_RDX,output),(UC_X86_REG_R8,96),(UC_X86_REG_R9,96),(UC_X86_REG_RSP,rsp)]:vm.reg_write(reg,value)
    vm.emu_start(0x1402aa500,STOP,timeout=5000000,count=1000000)
    if vm.reg_read(UC_X86_REG_RIP)!=STOP or vm.reg_read(UC_X86_REG_RAX)&255!=1:raise RuntimeError('Conversion did not succeed')
    return bytes(vm.mem_read(output,len(pixels)*4))


def main():
    binary=(ROOT/'evidence/private/binaries/platform_runtime_IS5LEYETRACKER5_service.exe').read_bytes()
    cases=[]
    for name,pixels in [('zero',bytes(9216)),('ramp',bytes(i%256 for i in range(9216))),('pattern',bytes((i*73+i*i//7)&255 for i in range(9216)))]:
        expected=prepare_crop_tensor(pixels).tobytes();actual=original(binary,pixels)
        assert actual==expected,name
        cases.append({'input':name,'floats':9216,'matches':True,'output_sha256':hashlib.sha256(actual).hexdigest()})
    report={'runtime_sha256':SHA256,'entry':'0x1402aa500','cases':cases,
            'limitations':'No stubs; contiguous one-channel crop conversion only; no image warp, full runner/inference parity or real tracking.'}
    (ROOT/'evidence/private/d1-input-oracle.json').write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report,indent=2))
if __name__=='__main__':main()
