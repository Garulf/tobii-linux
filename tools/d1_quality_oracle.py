#!/usr/bin/env python3
"""Original D1 alias filling, quality mean and caller branch comparisons."""
import json
from pathlib import Path
import struct
import numpy as np
from unicorn import UC_HOOK_CODE
from unicorn.x86_const import UC_X86_REG_XMM0,UC_X86_REG_RDI,UC_X86_REG_RIP
from vsmat_oracle import MatrixMachine
from ttp_encoder_oracle import SHA256,STOP
from tobii_linux.face_assets import read_landmark_ids
from tobii_linux.d1_quality import d1_quality_score,passes_d1_quality
ROOT=Path(__file__).resolve().parents[1]


def main():
    binary=(ROOT/'evidence/private/binaries/platform_runtime_IS5LEYETRACKER5_service.exe').read_bytes()
    ids=read_landmark_ids(ROOT/'models/tobii-et5/support/bdtsdata/NN/vnn_nir/landmarks.bin')
    cases=[]
    for name,values in [('zero',np.zeros(57)),('half',np.full(57,.5)),('ramp',np.linspace(0,1,57)),
                        ('negative',np.full(57,-1)),('mixed',np.linspace(-1,2,57))]:
        quality=np.asarray(values,dtype=np.float32)
        m=MatrixMachine(binary);fdp=m.allocate(0x1a0);groups={}
        counts=struct.unpack('<14i',m.vm.mem_read(0x14112f050,56))
        for group,count in enumerate(counts,2):
            groups[group]=m.allocate(count*72);m.vm.mem_write(fdp+0x120+group*8,struct.pack('<Q',groups[group]))
            for index in range(count):
                m.vm.mem_write(groups[group]+index*72+0x14,struct.pack('<f',-1))
                m.vm.mem_write(groups[group]+index*72+0x38,struct.pack('<Q',15))
        for k,(group,index) in enumerate(ids):
            record=groups[group]+(index-1)*72
            m.vm.mem_write(record,struct.pack('<fffIIf',.2,.3,-1,1,1,quality[k]))
        m.call(0x140055a20,fdp)
        present=[]
        for group,count in enumerate(counts,2):
            for index in range(1,count+1):
                r=groups[group]+(index-1)*72
                if struct.unpack('<I',m.vm.mem_read(r+0xc,4))[0]:present.append((group,index))
        assert len(present)==61
        m.call(0x14007a880,fdp)
        actual=struct.unpack('<f',struct.pack('<I',m.vm.reg_read(UC_X86_REG_XMM0)&0xffffffff))[0]
        expected=d1_quality_score(quality,ids)
        np.testing.assert_array_equal(actual,expected)
        print(name,actual,'exact',flush=True)
        cases.append({'input':name,'record_count':len(present),'score':actual if np.isfinite(actual) else None,'byte_exact':True})
    gate=[]
    for score in (.35,float(np.nextafter(np.float32(.35),np.float32(1))),.5,float(np.nextafter(np.float32(.5),np.float32(1))),float('nan')):
        for mode in (0,1,2,3):
            m=MatrixMachine(binary);context=m.allocate(0x40);m.vm.mem_write(context+0x38,struct.pack('<I',mode))
            m.vm.reg_write(UC_X86_REG_RDI,context);m.vm.reg_write(UC_X86_REG_XMM0,struct.unpack('<I',struct.pack('<f',score))[0])
            result=[]
            def stop(vm,address,size,_):
                result.append(address==0x1400495fb);vm.reg_write(UC_X86_REG_RIP,STOP)
            for address in (0x1400495fb,0x1400497ab):m.vm.hook_add(UC_HOOK_CODE,stop,begin=address,end=address)
            m.call(0x1400495d4)
            assert result==[passes_d1_quality(score,mode)]
            gate.append({'score':score if np.isfinite(score) else None,'mode':mode,'accepted':result[0]})
    (ROOT/'evidence/private/d1-quality-oracle.json').write_text(json.dumps({'runtime_sha256':SHA256,
        'entries':['0x140055a20','0x14007a880'],'gate_instruction_range':'0x1400495d4 to success 0x1400495fb / failure 0x1400497ab',
        'cases':cases,'gate_cases':gate,'limitations':'Complete D1 records, default absent others; alias and aggregate routines execute fully. Gate executes only the original branch range, not full caller. Caller mode meanings, selection, actual fitting mode chain and tracking validity remain unresolved.'},indent=2)+'\n')
if __name__=='__main__':main()
