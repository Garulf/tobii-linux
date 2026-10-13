#!/usr/bin/env python3
"""Compare original fitter construction and selected vertex/point ordering."""
import json
from pathlib import Path
import struct
import numpy as np
from vsmat_oracle import MatrixMachine
from ttp_encoder_oracle import SHA256
from tobii_linux.face_assets import read_mesh,read_fdp
from tobii_linux.pose_mesh import dense_deformations,normalize_mesh
from tobii_linux.pose_model import load_pose_model,select_pose_landmarks
ROOT=Path(__file__).resolve().parents[1]


def main():
    support=ROOT/'models/tobii-et5/support';model=load_pose_model(support)
    mesh=read_mesh(support/'jk_300.wfm');mapping=read_fdp(support/'jk_300_rigid.fdp')
    au,su=dense_deformations(mesh)
    vertices,_,_=normalize_mesh(mesh.vertices,su,au,[mapping[i]['vertex'] for i in ((3,7),(3,11),(3,8),(3,12))])
    binary=(ROOT/'evidence/private/binaries/platform_runtime_IS5LEYETRACKER5_service.exe').read_bytes();cases=[]
    for name,present in [('complete',model.landmark_ids),('aliases',(*model.landmark_ids,(2,1),(13,17),(14,21),(9,12))),
                         ('subset',model.landmark_ids[::2])]:
        m=MatrixMachine(binary);parent=m.allocate(0x168);outer=m.allocate(0x80)
        mat,_=m.matrix(len(vertices)*3,1,vertices.flatten());m.vm.mem_write(parent,struct.pack('<Q',mat))
        counts=struct.unpack('<14i',m.vm.mem_read(0x14112f050,56))
        def fdp():
            obj=m.allocate(0x1a0);groups={}
            for group,count in enumerate(counts,2):
                groups[group]=m.allocate(count*72);m.vm.mem_write(obj+0x120+group*8,struct.pack('<Q',groups[group]))
                for i in range(count):m.vm.mem_write(groups[group]+i*72+0x38,struct.pack('<Q',15))
            return obj,groups
        mapped,mg=fdp();observed,og=fdp();m.vm.mem_write(parent+0xd0,struct.pack('<Q',mapped))
        for (group,index),record in mapping.items():
            if group not in mg:continue
            address=mg[group]+(index-1)*72
            m.vm.mem_write(address+0xc,struct.pack('<I',1));m.vm.mem_write(address+0x18,struct.pack('<i',record['vertex']))
        for group,index in present:
            # Constructor selects the presence flag, even with flag +0x10 zero.
            m.vm.mem_write(og[group]+(index-1)*72+0xc,struct.pack('<I',1))
        m.call(0x1400db680,outer,parent,observed)
        count=struct.unpack('<I',m.vm.mem_read(outer+0x28,4))[0]
        begin,end=struct.unpack('<QQ',m.vm.mem_read(outer+0x38,16))
        points=np.asarray(m.floats(begin,(end-begin)//4),dtype=np.float32).reshape(-1,3)
        vi,ve=struct.unpack('<QQ',m.vm.mem_read(outer+0x50,16))
        actual_indices=struct.unpack('<'+'i'*((ve-vi)//4),m.vm.mem_read(vi,ve-vi))
        sentinel=struct.unpack('<Q',m.vm.mem_read(outer+0x68,8))[0]
        root=struct.unpack('<Q',m.vm.mem_read(sentinel+8,8))[0];ids=[None]*count
        def visit(node):
            if node==sentinel:return
            left,_,right=struct.unpack('<QQQ',m.vm.mem_read(node,24));g,i,slot=struct.unpack('<iii',m.vm.mem_read(node+0x1c,12))
            ids[slot]=(g,i);visit(left);visit(right)
        visit(root)
        expected,_=select_pose_landmarks(model,present)
        assert tuple(ids)==expected.landmark_ids and count==len(expected.landmark_ids)
        assert actual_indices==tuple(mapping[i]['vertex'] for i in expected.landmark_ids)
        np.testing.assert_array_equal(points,expected.vertices)
        print(name,count,'exact',flush=True);cases.append({'input':name,'selected_count':count,'ids':ids,'byte_exact_vertices':True})
    (ROOT/'evidence/private/pose-selection-oracle.json').write_text(json.dumps({'runtime_sha256':SHA256,
        'entry':'0x1400db680','cases':cases,'limitations':'Installed rigid FDP and normalized neutral mesh. Complete/alias/subset present flags. Does not establish dynamic caller model choice or missing-record retention after construction.'},indent=2)+'\n')
if __name__=='__main__':main()
