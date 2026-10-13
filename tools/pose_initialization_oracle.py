#!/usr/bin/env python3
"""Original reset initialization with explicit four-landmark model/image pairs."""
import json
from pathlib import Path
import struct
import numpy as np
from unicorn.x86_const import UC_X86_REG_XMM2,UC_X86_REG_XMM3
from vsmat_oracle import MatrixMachine
from ttp_encoder_oracle import SHA256
from tobii_linux.face_assets import read_mesh,read_fdp,read_shapes,read_landmark_ids,landmark_vertices
from tobii_linux.pose_fit import initial_rigid_state
ROOT=Path(__file__).resolve().parents[1]
IDS=((3,7),(3,11),(3,8),(3,12))


def original(binary,model,image,width,height,focal):
    m=MatrixMachine(binary);obj=m.allocate(0x180);source=m.allocate(0x1a0);mapping=m.allocate(0x1a0)
    srcgroup=m.allocate(14*0x48);mapgroup=m.allocate(14*0x48)
    m.vm.mem_write(source+0x138,struct.pack('<Q',srcgroup));m.vm.mem_write(mapping+0x138,struct.pack('<Q',mapgroup))
    m.vm.mem_write(obj+0xd0,struct.pack('<Q',mapping));m.vm.mem_write(obj+0x50,struct.pack('<f',focal))
    mat,_=m.matrix(4,3,model.flatten());m.vm.mem_write(obj+8,struct.pack('<Q',mat))
    for i,(_,index) in enumerate(IDS):
        r=srcgroup+(index-1)*0x48;m.vm.mem_write(r,struct.pack('<ff',*image[i]))
        r=mapgroup+(index-1)*0x48;m.vm.mem_write(r+0xc,struct.pack('<I',1));m.vm.mem_write(r+0x18,struct.pack('<i',i));m.vm.mem_write(r+0x38,struct.pack('<Q',15))
    for reg,value in ((UC_X86_REG_XMM2,np.float32(width)/np.float32(height)),(UC_X86_REG_XMM3,np.float32(np.pi))):
        m.vm.reg_write(reg,struct.unpack('<I',struct.pack('<f',value))[0])
    m.call(0x140052d70,obj,source)
    return np.asarray(m.floats(obj+0x58,6)),m.calls


def main():
    binary=(ROOT/'evidence/private/binaries/platform_runtime_IS5LEYETRACKER5_service.exe').read_bytes()
    support=ROOT/'models/tobii-et5/support'
    model=np.asarray(landmark_vertices(read_mesh(support/'jk_300.wfm'),read_fdp(support/'jk_300_rigid.fdp'),IDS),dtype=np.float32)
    _,shape=read_shapes(support/'bdtsdata/NN/vnn_nir/init_shape.bin')
    allids=read_landmark_ids(support/'bdtsdata/NN/vnn_nir/landmarks.bin')
    reference=np.asarray(shape,dtype=np.float32).reshape(57,2)/np.float32(96)
    image=np.asarray([reference[allids.index(identifier)] for identifier in IDS])
    cases=[]
    for name,points in [('reference',image),('translated',image+np.array([.1,-.2],dtype=np.float32)),('scaled',image*np.float32(.7))]:
        for width,height in ((640,480),(480,640),(512,512)):
            actual,calls=original(binary,model,points,width,height,2.6)
            expected=initial_rigid_state(model,points,width,height,2.6)
            error=float(np.max(np.abs(actual-expected)));print(name,width,height,error,flush=True)
            np.testing.assert_allclose(actual,expected,rtol=1e-6,atol=2e-5)
            cases.append({'input':name,'width':width,'height':height,'max_absolute_error':error,'passes':True,'allocator_calls':calls})
    report={'runtime_sha256':SHA256,'entry':'0x140052d70','cases':cases,
            'limitations':'Four supplied, valid mapped landmark pairs. Raw mesh/reference files serve only as numeric fixtures; no runtime mesh normalization or camera-parameter selection is established.'}
    (ROOT/'evidence/private/pose-initialization-oracle.json').write_text(json.dumps(report,indent=2)+'\n')
if __name__=='__main__':main()
