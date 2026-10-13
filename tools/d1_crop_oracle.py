#!/usr/bin/env python3
"""Execute complete original D1 preprocessing, stop immediately before inference."""
import json
from pathlib import Path
import struct
import numpy as np
from unicorn import UC_HOOK_CODE
from unicorn.x86_const import UC_X86_REG_RAX,UC_X86_REG_RDX,UC_X86_REG_R15,UC_X86_REG_RIP
from ttp_encoder_oracle import SHA256,STOP
from vsmat_oracle import MatrixMachine
from tobii_linux.face_assets import read_shapes
from tobii_linux.d1 import normalize_reference_shape,crop_transform,warp_grayscale
ROOT=Path(__file__).resolve().parents[1]


def original(binary,reference,pixels,box,roll,enabled=True):
    m=MatrixMachine(binary);m.call(0x1400011b0)
    ref,_=m.matrix(57,2,reference.flatten())
    dep=m.allocate(0x80);m.vm.mem_write(dep+4,struct.pack('<iii',96,1,57));m.vm.mem_write(dep+0x40,struct.pack('<Q',ref))
    wrapper=m.allocate(0x40);runner=m.allocate(16);vt=m.allocate(16)
    m.vm.mem_write(runner,struct.pack('<Q',vt))
    m.vm.mem_write(wrapper,struct.pack('<QQ',runner,dep));m.vm.mem_write(wrapper+0x14,struct.pack('<fB',roll,enabled))
    m.call(0x1400482e0,(120<<32)|160,8,1)
    image=m.vm.reg_read(UC_X86_REG_RAX);m.call(0x1400464e0,image)
    image_data=struct.unpack('<Q',m.vm.mem_read(image+0x58,8))[0];m.vm.mem_write(image_data,pixels)
    bbox=m.allocate(16);m.vm.mem_write(bbox,struct.pack('<iiii',*box))
    captured={}
    def capture(vm,address,size,_):
        desc=vm.reg_read(UC_X86_REG_RDX)
        data,rows,cols,channels,layout=struct.unpack('<Qiiii',vm.mem_read(desc,24))
        mat=vm.reg_read(UC_X86_REG_R15);matrix_data=struct.unpack('<Q',vm.mem_read(mat+0x18,8))[0]
        captured.update(crop=bytes(vm.mem_read(data,rows*cols*channels)),transform=np.array(m.floats(matrix_data,6)).reshape(2,3),descriptor=[rows,cols,channels,layout])
        vm.reg_write(UC_X86_REG_RIP,STOP)
    m.vm.hook_add(UC_HOOK_CODE,capture,begin=0x14007dfb9,end=0x14007dfb9)
    m.call(0x14007dc50,wrapper,image,bbox,0)
    if not captured:raise RuntimeError('Inference boundary was not reached')
    return captured,m.calls


def main():
    binary=(ROOT/'evidence/private/binaries/platform_runtime_IS5LEYETRACKER5_service.exe').read_bytes()
    _,values=read_shapes(ROOT/'models/tobii-et5/support/bdtsdata/NN/vnn_nir/init_shape.bin')
    ref=np.asarray(normalize_reference_shape(np.array(values).reshape(57,2)),dtype=np.float32)
    pixels=np.random.default_rng(57).integers(0,256,160*120,dtype=np.uint8).tobytes()
    cases=[]
    for box,roll,enabled in [((30,20,65,80),0,True),((0,0,160,120),5,True),((-15,10,80,90),-5,True),
                             ((30,20,65,80),20,True),((30,20,65,80),-32,True),((30,20,65,80),20,False)]:
        actual,calls=original(binary,ref,pixels,box,roll,enabled)
        transform=crop_transform(ref,box,roll,enabled);expected=warp_grayscale(pixels,160,120,transform)
        error=float(np.max(np.abs(actual['transform']-transform)))
        mismatch=sum(a!=b for a,b in zip(actual['crop'],expected))
        print(box,roll,enabled,error,mismatch,actual['descriptor'],flush=True)
        np.testing.assert_allclose(actual['transform'],transform,rtol=1e-6,atol=2e-5)
        assert actual['crop']==expected
        cases.append({'box':box,'roll_degrees':roll,'compensate_roll':enabled,'transform_max_absolute_error':error,'crop_byte_exact':True,'descriptor':actual['descriptor'],'allocator_calls':calls})
    report={'runtime_sha256':SHA256,'entry':'0x14007dc50','stop_before':'0x14007dfb9','cases':cases,
            'limitations':'Original preprocessing executes, allocator/free stubs only, synthetic GS stack limit. Stops before inference; no neural output or temporal state check.'}
    (ROOT/'evidence/private/d1-crop-oracle.json').write_text(json.dumps(report,indent=2)+'\n')
if __name__=='__main__':main()
