#!/usr/bin/env python3
"""Check how original D1 output maps to FDP validity and quality fields."""
import json
from pathlib import Path
import struct
import numpy as np
from vsmat_oracle import MatrixMachine
from ttp_encoder_oracle import SHA256
from tobii_linux.face_assets import read_landmark_ids
ROOT=Path(__file__).resolve().parents[1]


def main():
    binary=(ROOT/'evidence/private/binaries/platform_runtime_IS5LEYETRACKER5_service.exe').read_bytes()
    ids=read_landmark_ids(ROOT/'models/tobii-et5/support/bdtsdata/NN/vnn_nir/landmarks.bin')
    cases=[]
    for name,quality in [('zero',np.zeros(57,dtype=np.float32)),('ramp',np.linspace(0,1,57,dtype=np.float32)),
                         ('out-of-range',np.linspace(-1,2,57,dtype=np.float32))]:
        m=MatrixMachine(binary);obj=m.allocate(0x20);settings=m.allocate(0x50);image=m.allocate(0x70);fdp=m.allocate(0x1a0)
        m.vm.mem_write(obj+8,struct.pack('<Q',settings));m.vm.mem_write(settings+0xc,struct.pack('<I',57))
        for offset,values in ((0x10,[g for g,_ in ids]),(0x28,[i for _,i in ids])):
            data=m.allocate(57*4);m.vm.mem_write(data,struct.pack('<57i',*values))
            m.vm.mem_write(settings+offset,struct.pack('<QQQ',data,data+57*4,data+57*4))
        m.vm.mem_write(image+0x28,struct.pack('<ii',640,480))
        counts=struct.unpack('<14i',m.vm.mem_read(0x14112f050,56));groups={}
        for group,count in enumerate(counts,2):
            groups[group]=m.allocate(count*72);m.vm.mem_write(fdp+0x120+8*group,struct.pack('<Q',groups[group]))
        points=np.random.default_rng(57).uniform(-100,700,(57,2)).astype(np.float32)
        pmat,_=m.matrix(57,2,points.flatten());q=m.allocate(57*4);m.vm.mem_write(q,quality.tobytes())
        m.call(0x14007d910,obj,pmat,fdp,image,q)
        expected=points/np.array([640,480],dtype=np.float32)
        for k,(group,index) in enumerate(ids):
            record=m.vm.mem_read(groups[group]+(index-1)*72,24)
            x,y,z,present,valid,score=struct.unpack('<fffIIf',record)
            np.testing.assert_array_equal([x,y],expected[k])
            assert z==-1 and present==1 and valid==1
            assert struct.pack('<f',score)==quality[k].tobytes()
        print(name,'57 records exact',flush=True)
        cases.append({'input':name,'record_count':57,'all_present_and_valid':True,'byte_exact_values':True})
    (ROOT/'evidence/private/d1-records-oracle.json').write_text(json.dumps({'runtime_sha256':SHA256,
        'entry':'0x14007d910','cases':cases,'finding':'D1 record writer copies quality without thresholding and sets both flags to one. Tracking confidence must be established by a later caller.',
        'limitations':'Synthetic output coordinates/qualities; no neural inference or later selection executed.'},indent=2)+'\n')
if __name__=='__main__':main()
