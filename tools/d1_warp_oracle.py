#!/usr/bin/env python3
"""Compare recovered byte warp against complete original wrapper and worker."""
import json
from pathlib import Path
import struct
import numpy as np
from ttp_encoder_oracle import SHA256
from vsmat_oracle import MatrixMachine
from tobii_linux.d1 import warp_grayscale
ROOT=Path(__file__).resolve().parents[1]


def original(binary,pixels,width,height,transform):
    m=MatrixMachine(binary)
    m.call(0x1400011b0)  # Original aligned interpolation-storage initializer.
    def u8matrix(rows,cols,values):
        data=m.allocate(len(values));m.vm.mem_write(data,values)
        header=m.allocate(40)
        m.vm.mem_write(header,struct.pack('<iiQiiQii',0x42424000,cols,0,0,0,data,rows,cols))
        return header,data
    src,_=u8matrix(height,width,pixels);dst,data=u8matrix(96,96,bytes(96*96))
    mat,_=m.matrix(2,3,np.asarray(transform,dtype=np.float32).flatten());border=m.allocate(32)
    m.call(0x1400b45b0,src,dst,mat,0,border)
    return bytes(m.vm.mem_read(data,96*96)),m.calls


def main():
    binary=(ROOT/'evidence/private/binaries/platform_runtime_IS5LEYETRACKER5_service.exe').read_bytes()
    cases=[]
    fixtures=[('identity',[[1,0,0],[0,1,0]]),('integer',[[1,0,-15],[0,1,-21]]),
              ('fractional',[[1,0,-0.37],[0,1,-0.61]]),('border',[[1,0,30.25],[0,1,10.75]]),
              ('rotate-scale',[[0.7,-0.4,25],[0.4,0.7,-15]]),('small-scale',[[0.3,0.05,20],[-0.05,0.3,15]])]
    for kind in ('ramp','noise'):
        pixels=(bytes((x+3*y)%256 for y in range(120) for x in range(160)) if kind=='ramp'
                else np.random.default_rng(57).integers(0,256,120*160,dtype=np.uint8).tobytes())
        for name,transform in fixtures:
            actual,calls=original(binary,pixels,160,120,transform)
            expected=warp_grayscale(pixels,160,120,transform)
            a=np.frombuffer(actual,dtype=np.uint8).astype(int);e=np.frombuffer(expected,dtype=np.uint8).astype(int)
            mismatches=int(np.sum(a!=e));error=int(np.max(np.abs(a-e)))
            print(kind,name,mismatches,error,flush=True)
            assert actual==expected,(kind,name,mismatches,error)
            cases.append({'input':kind,'transform':name,'byte_exact':True,'allocator_calls':calls})
    report={'runtime_sha256':SHA256,'initializer':'0x1400011b0','entry':'0x1400b45b0','cases':cases,
            'limitations':'Synthetic grayscale crops. Original initialization, warp, interpolation and border math run unchanged; allocator/free stubs and synthetic GS stack limit only.'}
    (ROOT/'evidence/private/d1-warp-oracle.json').write_text(json.dumps(report,indent=2)+'\n')
if __name__=='__main__':main()
