#!/usr/bin/env python3
"""Original LBF feature gathering, including selected forests and scale mutation."""
import json
from pathlib import Path
import struct
from unicorn import UC_HOOK_CODE
from unicorn.x86_const import *
from ttp_encoder_oracle import machine, DATA, STACK, STOP, SHA256
from tobii_linux.lbf import read_lbf, quantize_matrix
from tobii_linux.lbf_predictor import gather_features, refine_landmarks
ROOT=Path(__file__).resolve().parents[1]


def original(binary,model,stage,pixels,rows,columns,stride,centers,scale,mirrored,refine=False):
    vm=machine(binary);heap=0x63000000;vm.mem_map(heap,0x1000000);cursor=heap
    def alloc(data):
        nonlocal cursor
        ptr=cursor;vm.mem_write(ptr,data);cursor+=(len(data)+15)&~15;return ptr
    pointers=[]
    for point in model['points']:
        if not point['submodels']:
            pointers.append(0);continue
        headers=[]
        for first,third,trees in point['submodels']:
            descriptors=[]
            for _,word,depth,payload in trees:
                payload_ptr=alloc(payload);threshold=alloc(bytes(1<<depth))
                descriptors.append(struct.pack('<iiQQQ',word,depth,payload_ptr,threshold,0))
            table=alloc(b''.join(descriptors))
            headers.append(struct.pack('<iiQQ',third,first,len(trees),table))
        table=alloc(b''.join(headers));pointers.append(alloc(struct.pack('<QQ',len(headers),table)))
    table=alloc(struct.pack('<'+'Q'*len(pointers),*pointers))
    obj=alloc(struct.pack('<QQQ',table,table+len(pointers)*8,table+len(pointers)*8))
    a=alloc(struct.pack('<'+'f'*len(centers),*(p[0] for p in centers)))
    b=alloc(struct.pack('<'+'f'*len(centers),*(p[1] for p in centers)))
    scale_ptr=alloc(struct.pack('<f',scale));image=alloc(pixels);output=DATA
    rsp=STACK+0xfef8;vm.mem_write(rsp,struct.pack('<Q',STOP))
    for off,val in [(0x28,b),(0x30,scale_ptr),(0x38,image),(0x40,rows),(0x48,columns),(0x50,stride),(0x58,int(mirrored))]:vm.mem_write(rsp+off,struct.pack('<Q',val))
    for reg,val in [(UC_X86_REG_RCX,obj),(UC_X86_REG_RDX,output),(UC_X86_REG_R8,stage),(UC_X86_REG_R9,a),(UC_X86_REG_RSP,rsp)]:vm.reg_write(reg,val)
    entry=0x1400debc0
    if refine:
        stages=[]
        for matrices in model['stages']:
            descriptors=[]
            for i,matrix in enumerate(matrices):
                inputs,outputs=matrix['shape'];bias=alloc(matrix['biases'])
                if i==0:
                    qweights,scales=quantize_matrix(matrix)
                    floats,quantized,scales_ptr=0,alloc(qweights),alloc(scales)
                else:
                    floats,quantized,scales_ptr=alloc(matrix['weights']),0,0
                descriptors.append(struct.pack('<iiQQQQ',outputs,inputs,bias,floats,quantized,scales_ptr))
            table=alloc(b''.join(descriptors))
            stages.append(alloc(struct.pack('<QQQiii',table,table+len(matrices)*40,table+len(matrices)*40,len(matrices),1,1)))
        stage_table=alloc(struct.pack('<'+'Q'*len(stages),*stages))
        model_object=bytearray(72)
        struct.pack_into('<QQ',model_object,0,stage_table,stage_table+len(stages)*8)
        struct.pack_into('<Q',model_object,0x18,obj)
        struct.pack_into('<i',model_object,0x20,len(stages))
        struct.pack_into('<i',model_object,0x40,len(centers))
        model_ptr=alloc(bytes(model_object))
        def stub(vm,address,size,_):
            nonlocal cursor
            rcx=vm.reg_read(UC_X86_REG_RCX);result=0
            if address==0x1403b6808:
                result=cursor;cursor+=(rcx+15)&~15
                if cursor>=heap+0x1000000:raise RuntimeError('Allocation bound')
            elif address==0x1403da6c0:
                vm.mem_write(rcx,bytes([vm.reg_read(UC_X86_REG_RDX)&255])*vm.reg_read(UC_X86_REG_R8));result=rcx
            stack=vm.reg_read(UC_X86_REG_RSP);ret=struct.unpack('<Q',vm.mem_read(stack,8))[0]
            vm.reg_write(UC_X86_REG_RAX,result);vm.reg_write(UC_X86_REG_RSP,stack+8);vm.reg_write(UC_X86_REG_RIP,ret)
        for addr in (0x1403b6808,0x1403b59c4,0x1403da6c0):vm.hook_add(UC_HOOK_CODE,stub,begin=addr,end=addr)
        for off,val in [(0x28,image),(0x30,rows),(0x38,columns),(0x40,stride),(0x48,int(mirrored))]:vm.mem_write(rsp+off,struct.pack('<Q',val))
        for reg,val in [(UC_X86_REG_RCX,model_ptr),(UC_X86_REG_RDX,a),(UC_X86_REG_R8,b),(UC_X86_REG_XMM3,int.from_bytes(struct.pack('<f',scale),'little'))]:vm.reg_write(reg,val)
        entry=0x1400acb80
    vm.emu_start(entry,STOP,timeout=15000000,count=10000000)
    if vm.reg_read(UC_X86_REG_RIP)!=STOP:raise RuntimeError('Gather/refinement did not return')
    if refine:
        axis0=struct.unpack('<'+'f'*len(centers),vm.mem_read(a,len(centers)*4))
        axis1=struct.unpack('<'+'f'*len(centers),vm.mem_read(b,len(centers)*4))
        return tuple(zip(axis0,axis1))
    count=sum(len(p['submodels'][stage][2]) for p in model['points'] if p['submodels'])
    return struct.unpack('<'+'i'*count,vm.mem_read(output,count*4)),struct.unpack('<f',vm.mem_read(scale_ptr,4))[0]


def main():
    binary=(ROOT/'evidence/private/binaries/platform_runtime_IS5LEYETRACKER5_service.exe').read_bytes();cases=[]
    rows,columns,stride=96,128,132
    pixels=bytes((i*73+i*i)&255 for i in range(rows*stride))
    for name in ('fa','fc'):
        model=read_lbf(ROOT/f'models/tobii-et5/support/bdtsdata/NN/{name}.lbf')
        centers=[(i*13%96+.3,i*17%128+.7) for i in range(len(model['points']))]
        for stage in range(4):
            for mirrored in (False,True):
                args=(model,stage,pixels,rows,columns,stride,centers,95.7,mirrored)
                expected=gather_features(*args);actual=original(binary,*args)
                assert expected==actual,(name,stage,mirrored,expected[0][:4],actual[0][:4])
                cases.append({'model':name,'stage':stage,'mirrored':mirrored,'features':len(actual[0]),'stage_scale':actual[1],'matches':True})
        for mirrored in (False,True):
            expected=refine_landmarks(model,pixels,rows,columns,stride,centers,95.7,mirrored)
            actual=original(binary,model,0,pixels,rows,columns,stride,centers,95.7,mirrored,refine=True)
            error=max(abs(a-b) for p,q in zip(expected,actual) for a,b in zip(p,q))
            assert error<0.0001,(name,mirrored,error)
            cases.append({'model':name,'kind':'four-stage-refinement','mirrored':mirrored,'max_absolute_error':error,'passes':True})
    report={'runtime_sha256':SHA256,'entries':['0x1400debc0','0x1400acb80'],'cases':cases,'limitations':'Synthetic image and caller-supplied centers/scale; gathering has no import stubs, refinement uses allocator/free/memset stubs; no real preprocessing or pose.'}
    (ROOT/'evidence/private/lbf-gather-oracle.json').write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report,indent=2))
if __name__=='__main__':main()
