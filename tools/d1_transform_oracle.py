#!/usr/bin/env python3
"""Original-code comparisons for D1 shape fit, inverse and composition."""
import json
from pathlib import Path
import numpy as np
from ttp_encoder_oracle import SHA256
from vsmat_oracle import MatrixMachine
from tobii_linux.d1 import fit_similarity, inverse_transform_points, compose_affine
ROOT = Path(__file__).resolve().parents[1]


def main():
    binary=(ROOT/'evidence/private/binaries/platform_runtime_IS5LEYETRACKER5_service.exe').read_bytes()
    rng=np.random.default_rng(57)
    source=rng.uniform(-50,150,(57,2)).astype(np.float32)
    cases=[]
    transforms=[('identity',[[1,0,0],[0,1,0]]),('translate',[[1,0,201],[0,1,-41]]),
                ('rotate-scale',[[0.7,-0.4,90],[0.4,0.7,15]]),('small-scale',[[0.01,0.02,48],[-0.02,0.01,48]])]
    for label,t in transforms:
        t=np.asarray(t,dtype=np.float32)
        target=(source @ t[:,:2].T+t[:,2]).astype(np.float32)
        for name,entry,expected in [('fit',0x1400dac40,fit_similarity(source,target)),
                                    ('inverse',0x1400db010,inverse_transform_points(target,t)),
                                    ('compose',0x1400db330,compose_affine(t,[[1.2,0.3,12],[-0.3,1.2,-6]]))]:
            m=MatrixMachine(binary)
            if name=='fit':
                src,_=m.matrix(57,2,source.flatten());dst,_=m.matrix(57,2,target.flatten());out,data=m.matrix(2,3,[0]*6)
                m.call(entry,src,dst,out)
            elif name=='inverse':
                src,_=m.matrix(57,2,target.flatten());out,data=m.matrix(57,2,[0]*114);mat,_=m.matrix(2,3,t.flatten())
                m.call(entry,src,out,mat)
            else:
                a,_=m.matrix(2,3,t.flatten());b,_=m.matrix(2,3,[1.2,0.3,12,-0.3,1.2,-6]);out,data=m.matrix(2,3,[0]*6)
                m.call(entry,a,b,out)
            actual=np.asarray(m.floats(data,expected.size),dtype=np.float32).reshape(expected.shape)
            error=float(np.max(np.abs(actual-expected)))
            print(name,label,error,flush=True)
            np.testing.assert_allclose(actual,expected,rtol=1e-5,atol=1e-4)
            cases.append({'operation':name,'input':label,'entry':hex(entry),'max_absolute_error':error,'passes':True,'allocator_calls':m.calls})
    report={'runtime_sha256':SHA256,'cases':cases,'limitations':'Only allocator/free stubs, synthetic GS stack limit; numerical tolerance, not bit parity or real tracking.'}
    (ROOT/'evidence/private/d1-transform-oracle.json').write_text(json.dumps(report,indent=2)+'\n')
if __name__=='__main__':main()
