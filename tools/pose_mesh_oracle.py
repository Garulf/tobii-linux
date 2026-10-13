#!/usr/bin/env python3
"""Compare original full centering/rescaling helpers on installed model arrays."""
import json
from pathlib import Path
import struct
import numpy as np
from unicorn.x86_const import UC_X86_REG_XMM1
from vsmat_oracle import MatrixMachine
from ttp_encoder_oracle import SHA256
from tobii_linux.face_assets import read_mesh,read_fdp
from tobii_linux.pose_mesh import dense_deformations,normalize_mesh,rescale_mesh
ROOT=Path(__file__).resolve().parents[1]
IDS=((3,7),(3,11),(3,8),(3,12))


def original(binary,vertices,first,second,indices,target,rescale_to=None):
    m=MatrixMachine(binary);obj=m.allocate(0x168);mapping=m.allocate(0x1a0);group=m.allocate(14*0x48)
    m.vm.mem_write(obj+0xd0,struct.pack('<Q',mapping));m.vm.mem_write(mapping+0x138,struct.pack('<Q',group))
    m.vm.mem_write(obj+0xc8,struct.pack('<I',len(vertices)));m.vm.mem_write(obj+0x10c,struct.pack('<f',target))
    for (_,index),vertex in zip(IDS,indices):
        rec=group+(index-1)*0x48;m.vm.mem_write(rec+0xc,struct.pack('<I',1));m.vm.mem_write(rec+0x18,struct.pack('<i',vertex));m.vm.mem_write(rec+0x38,struct.pack('<Q',15))
    data=[]
    for offset,array in ((0,vertices),(8,vertices),(0x10,first),(0x18,second)):
        rows=len(vertices)*3;columns=array.size//rows
        mat,p=m.matrix(rows,columns,array.flatten());m.vm.mem_write(obj+offset,struct.pack('<Q',mat));data.append(p)
    m.call(0x140053f50,obj)
    if rescale_to is not None:
        m.vm.reg_write(UC_X86_REG_XMM1,struct.unpack('<I',struct.pack('<f',rescale_to))[0]);m.call(0x140052bc0,obj)
    actual=[np.asarray(m.floats(p,a.size),dtype=np.float32).reshape(a.shape) for p,a in zip(data[1:],(vertices,first,second))]
    np.testing.assert_array_equal(m.floats(data[0],vertices.size),actual[0].flatten())
    return actual,m.calls


def main():
    support=ROOT/'models/tobii-et5/support';mesh=read_mesh(support/'jk_300.wfm');mapping=read_fdp(support/'jk_300_rigid.fdp')
    indices=[mapping[i]['vertex'] for i in IDS]
    vertices=np.asarray(mesh.vertices,dtype=np.float32);first,second=dense_deformations(mesh)
    binary=(ROOT/'evidence/private/binaries/platform_runtime_IS5LEYETRACKER5_service.exe').read_bytes();cases=[]
    for name,points,target,after in [('installed',vertices,.065,None),('translated',vertices+np.array([2,-4,9],dtype=np.float32),.065,None),
                                   ('alternate-distance',vertices,.072,None),('rescaled',vertices,.065,.072)]:
        expected=normalize_mesh(points,first,second,indices,target)
        if after is not None:expected=rescale_mesh(*expected,target,after)
        actual,calls=original(binary,points,first,second,indices,target,after)
        errors=[float(np.max(np.abs(a-e))) for a,e in zip(actual,expected)]
        print(name,errors,flush=True)
        for a,e in zip(actual,expected):np.testing.assert_array_equal(a,e)
        cases.append({'input':name,'target_distance':target,'rescaled_distance':after,'byte_exact':True,'max_errors':errors,'allocator_calls':calls})
    report={'runtime_sha256':SHA256,'entries':['0x140053f50','0x140052bc0'],'cases':cases,
            'limitations':'Installed raw vertex/deformation arrays supplied to original helpers. Does not validate the original text loader, disabled deformation masks or public/device coordinate conversion.'}
    (ROOT/'evidence/private/pose-mesh-oracle.json').write_text(json.dumps(report,indent=2)+'\n')
if __name__=='__main__':main()
