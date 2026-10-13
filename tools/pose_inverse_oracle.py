#!/usr/bin/env python3
"""Check runtime inversion, including pivot swaps and singular matrices."""
import json
from pathlib import Path
import numpy as np
from vsmat_oracle import MatrixMachine
from ttp_encoder_oracle import SHA256
from tobii_linux.pose_linalg import inverse_float32
ROOT=Path(__file__).resolve().parents[1]


def main():
    binary=(ROOT/'evidence/private/binaries/platform_runtime_IS5LEYETRACKER5_service.exe').read_bytes()
    rng=np.random.default_rng(69)
    inputs=[]
    for n in (4,6,11,69):
        a=rng.uniform(-1,1,(n,n)).astype(np.float32)
        inputs.append((f'random-{n}',a))
    inputs.extend([('pivot-swap',np.roll(np.eye(6,dtype=np.float32),2,axis=0)),
        ('singular',np.ones((6,6),dtype=np.float32)),
        ('small-pivot',np.diag(np.array([1e-8,1,1,1],dtype=np.float32))),
        ('epsilon-pivot',np.diag(np.array([np.finfo(np.float32).eps,1,1,1],dtype=np.float32)))])
    cases=[]
    for name,a in inputs:
        m=MatrixMachine(binary);source,_=m.matrix(len(a),len(a),a.flatten());out,data=m.matrix(len(a),len(a),[123]*a.size)
        m.call(0x140085510,source,out,instruction_limit=30_000_000)
        actual=np.asarray(m.floats(data,a.size),dtype=np.float32).reshape(a.shape)
        expected=inverse_float32(a)
        np.testing.assert_array_equal(actual,expected)
        print(name,'byte-exact',flush=True)
        cases.append({'input':name,'order':len(a),'byte_exact':True})
    (ROOT/'evidence/private/pose-inverse-oracle.json').write_text(json.dumps({
        'runtime_sha256':SHA256,'entries':['0x140085510','0x14007fba0','0x140085b70'],'cases':cases,
        'limitations':'Finite float32 matrices of order at least four. Small closed-form paths, double precision and nonfinite inputs not implemented.'},indent=2)+'\n')
if __name__=='__main__':main()
