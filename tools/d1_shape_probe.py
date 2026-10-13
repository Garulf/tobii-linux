#!/usr/bin/env python3
"""Bounded exploratory matrix-helper emulation; failures are recorded as failures."""
import json
from pathlib import Path
import struct
from unicorn.x86_const import *
from ttp_encoder_oracle import machine,DATA,STACK,STOP,SHA256
from tobii_linux.face_assets import read_shapes
ROOT=Path(__file__).resolve().parents[1]


def main():
    binary=(ROOT/'evidence/private/binaries/platform_runtime_IS5LEYETRACKER5_service.exe').read_bytes()
    _,values=read_shapes(ROOT/'models/tobii-et5/support/bdtsdata/NN/vnn_nir/init_shape.bin')
    cases=[]
    for mode in (1,2,3):
        vm=machine(binary);vm.mem_map(0x64000000,0x1000);vm.reg_write(UC_X86_REG_GS_BASE,0x64000000)
        vm.mem_write(0x64000010,struct.pack('<Q',STACK))
        src,out=DATA,DATA+0x100;coords,target=DATA+0x1000,DATA+0x2000
        vm.mem_write(coords,struct.pack('<114f',*values))
        for ptr,data,rows in [(src,coords,57),(out,target,1)]:vm.mem_write(ptr,struct.pack('<iiQiiQii',0x42424005,8,0,0,0,data,rows,2))
        rsp=STACK+0xfef8;vm.mem_write(rsp,struct.pack('<Q',STOP))
        for reg,v in [(UC_X86_REG_RCX,src),(UC_X86_REG_RDX,out),(UC_X86_REG_R8,0),(UC_X86_REG_R9,mode),(UC_X86_REG_RSP,rsp)]:vm.reg_write(reg,v)
        item={'mode':mode}
        try:
            vm.emu_start(0x140026420,STOP,timeout=2000000,count=1000000)
            item.update({'returned':vm.reg_read(UC_X86_REG_RIP)==STOP,'output':struct.unpack('<ff',vm.mem_read(target,8))})
        except Exception as error:item.update({'returned':False,'error':str(error)})
        item['rip']=hex(vm.reg_read(UC_X86_REG_RIP));cases.append(item)
    report={'runtime_sha256':SHA256,'entry':'0x140026420','cases':cases,'environment':'Synthetic GS stack-limit field; no function stubs.'}
    (ROOT/'evidence/private/d1-shape-probe.json').write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report,indent=2))
if __name__=='__main__':main()
