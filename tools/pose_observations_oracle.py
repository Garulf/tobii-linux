#!/usr/bin/env python3
"""Complete original observation preparation for populated D1 landmark records."""
import json
from pathlib import Path
import struct
import numpy as np
from vsmat_oracle import MatrixMachine
from ttp_encoder_oracle import SHA256
from tobii_linux.face_assets import read_landmark_ids
from tobii_linux.pose_fit import prepare_observations
ROOT=Path(__file__).resolve().parents[1]


def original(binary,points,ids,width,height):
    m=MatrixMachine(binary)
    obj=m.allocate(0x80);parent=m.allocate(0x180);model=m.allocate(0xb8)
    m.vm.mem_write(obj+0x20,struct.pack('<Q',parent));m.vm.mem_write(obj+0x18,struct.pack('<Q',model))
    m.vm.mem_write(parent+0x5c,struct.pack('<f',np.float32(np.pi)));m.vm.mem_write(parent+0x6c,struct.pack('<f',6))
    counts=struct.unpack('<14i',m.vm.mem_read(0x14112f050,56))
    def fdp():
        header=m.allocate(0x1a0);groups={}
        for g,n in enumerate(counts,2):
            block=m.allocate(n*0x48);groups[g]=block
            m.vm.mem_write(header+0x120+8*g,struct.pack('<Q',block))
        return header,groups
    observed,groups=fdp();mapped,mapgroups=fdp()
    m.vm.mem_write(parent+0xd0,struct.pack('<Q',mapped))
    for i,(g,index) in enumerate(ids):
        record=groups[g]+(index-1)*0x48
        m.vm.mem_write(record,struct.pack('<fffI',*points[i],-1,1))
        target=mapgroups[g]+(index-1)*0x48
        m.vm.mem_write(target+0xc,struct.pack('<I',1));m.vm.mem_write(target+0x18,struct.pack('<i',i))
        m.vm.mem_write(target+0x38,struct.pack('<Q',15))
    sentinel=m.allocate(0x28);m.vm.mem_write(sentinel+0x19,b'\1')
    def tree(items,parent):
        if not items:return sentinel
        mid=len(items)//2;identifier,slot=items[mid];node=m.allocate(0x28)
        left=tree(items[:mid],node);right=tree(items[mid+1:],node)
        m.vm.mem_write(node,struct.pack('<QQQ',left,parent,right))
        m.vm.mem_write(node+0x1c,struct.pack('<iii',*identifier,slot));return node
    root=tree(sorted((identifier,i) for i,identifier in enumerate(ids)),sentinel)
    m.vm.mem_write(sentinel,struct.pack('<QQQ',sentinel,root,sentinel))
    m.vm.mem_write(obj+0x68,struct.pack('<QQ',sentinel,len(ids)))
    obs,data=m.matrix(2*len(ids),1,[0]*(2*len(ids)));m.vm.mem_write(obj+0x30,struct.pack('<Q',obs))
    weights,wd=m.matrix(2*len(ids),1,[1]*(2*len(ids)));m.vm.mem_write(model+0x98,struct.pack('<Q',weights))
    m.call(0x1400dc730,obj,observed,width,height)
    return np.array(m.floats(data,2*len(ids))).reshape(-1,2),np.array(m.floats(wd,2*len(ids))),m.calls


def main():
    binary=(ROOT/'evidence/private/binaries/platform_runtime_IS5LEYETRACKER5_service.exe').read_bytes()
    ids=read_landmark_ids(ROOT/'models/tobii-et5/support/bdtsdata/NN/vnn_nir/landmarks.bin')
    cases=[]
    for name,points in [('center',np.full((57,2),.5,dtype=np.float32)),('spread',np.random.default_rng(57).uniform(-.2,1.2,(57,2)).astype(np.float32))]:
        for width,height in ((640,480),(480,640),(512,512)):
            actual,weights,calls=original(binary,points,ids,width,height)
            expected,expected_weights=prepare_observations(points,ids,width,height)
            np.testing.assert_array_equal(actual,expected);np.testing.assert_array_equal(weights,expected_weights)
            print(name,width,height,'exact',flush=True)
            cases.append({'input':name,'width':width,'height':height,'exact':True,'allocator_calls':calls})
    report={'runtime_sha256':SHA256,'entry':'0x1400dc730','cases':cases,
            'limitations':'All 57 D1 records present/valid and mapped, neutral facing state, prebuilt lookup tree. No missing records, group10/13 view-dependent weights or live selection.'}
    (ROOT/'evidence/private/pose-observations-oracle.json').write_text(json.dumps(report,indent=2)+'\n')
if __name__=='__main__':main()
