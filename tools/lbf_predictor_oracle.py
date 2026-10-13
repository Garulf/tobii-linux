#!/usr/bin/env python3
"""Compare recovered matrix stages to original x64 with allocator stubs only."""
import json
from pathlib import Path
import struct
import numpy as np
from unicorn import UC_HOOK_CODE
from unicorn.x86_const import *
from ttp_encoder_oracle import machine, DATA, STACK, STOP, SHA256
from tobii_linux.lbf import read_lbf, quantize_matrix
from tobii_linux.lbf_predictor import predict_stage
ROOT = Path(__file__).resolve().parents[1]


def original(binary, matrices, features):
    vm = machine(binary)
    heap = 0x63000000
    vm.mem_map(heap, 0x1000000)
    cursor = heap + 0x10000
    def alloc(data):
        nonlocal cursor
        ptr = cursor
        vm.mem_write(ptr, data)
        cursor += (len(data) + 15) & ~15
        return ptr
    obj, table, output = DATA, heap, DATA + 0x1000
    for i, matrix in enumerate(matrices):
        inputs, outputs = matrix['shape']
        bias = alloc(matrix['biases'])
        if i == 0:
            weights, scales = quantize_matrix(matrix)
            floats, quantized, scale = 0, alloc(weights), alloc(scales)
        else:
            floats, quantized, scale = alloc(matrix['weights']), 0, 0
        vm.mem_write(table + i*40, struct.pack('<iiQQQQ', outputs, inputs, bias, floats, quantized, scale))
    source = alloc(struct.pack('<'+'i'*len(features), *features))
    vm.mem_write(obj, struct.pack('<QQQiii', table, table + len(matrices)*40, table + len(matrices)*40, len(matrices), 1, 1))
    def stub(vm, address, size, _):
        nonlocal cursor
        rcx = vm.reg_read(UC_X86_REG_RCX)
        result = 0
        if address == 0x1403b6808:
            result = cursor
            cursor += (rcx + 15) & ~15
            if cursor >= heap + 0x1000000:
                raise RuntimeError('Allocation bound')
        elif address == 0x1403da6c0:
            vm.mem_write(rcx, bytes([vm.reg_read(UC_X86_REG_RDX)&255])*vm.reg_read(UC_X86_REG_R8))
            result = rcx
        stack = vm.reg_read(UC_X86_REG_RSP)
        ret = struct.unpack('<Q',vm.mem_read(stack,8))[0]
        vm.reg_write(UC_X86_REG_RAX,result);vm.reg_write(UC_X86_REG_RSP,stack+8);vm.reg_write(UC_X86_REG_RIP,ret)
    for addr in (0x1403b6808,0x1403b59c4,0x1403da6c0):
        vm.hook_add(UC_HOOK_CODE,stub,begin=addr,end=addr)
    rsp=STACK+0xfef8;vm.mem_write(rsp,struct.pack('<Q',STOP))
    for reg,val in [(UC_X86_REG_RCX,obj),(UC_X86_REG_RDX,source),(UC_X86_REG_R8,len(features)),(UC_X86_REG_R9,output),(UC_X86_REG_RSP,rsp)]:vm.reg_write(reg,val)
    vm.emu_start(0x1400dd410,STOP,timeout=10_000_000,count=3000000)
    if vm.reg_read(UC_X86_REG_RIP)!=STOP:raise RuntimeError('Predictor did not return')
    return np.frombuffer(bytes(vm.mem_read(output,matrices[-1]['shape'][1]*4)),dtype='<f4')


def main():
    binary=(ROOT/'evidence/private/binaries/platform_runtime_IS5LEYETRACKER5_service.exe').read_bytes()
    cases=[]
    for name in ('fa','fc'):
        model=read_lbf(ROOT/f'models/tobii-et5/support/bdtsdata/NN/{name}.lbf')
        for stage, matrices in enumerate(model['stages']):
            for pattern in (0,1):
                features=tuple(i*16+(i*7+pattern*11)%16 for i in range(matrices[0]['shape'][0]//16))
                expected=predict_stage(matrices,features)
                actual=original(binary,matrices,features)
                error=float(np.max(np.abs(expected-actual)))
                if not np.allclose(expected,actual,rtol=1e-5,atol=5e-6):raise AssertionError((name,stage,error))
                cases.append({'model':name,'stage':stage,'pattern':pattern,'max_absolute_error':error,'passes':True})
    report={'runtime_sha256':SHA256,'entry':'0x1400dd410','cases':cases,
            'limitations':'Allocator/free/memset stubs only; native activation executes; numerical tolerance not bit parity; no camera or pose.'}
    (ROOT/'evidence/private/lbf-predictor-oracle.json').write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps(report,indent=2))
if __name__=='__main__':main()
