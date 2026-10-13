#!/usr/bin/env python3
"""Check unusual original deformation-column math instead of assuming a gradient."""
import json
from pathlib import Path
import struct
import numpy as np
from pose_projection_oracle import make_model,SHA256
from tobii_linux.pose_projection import model_jacobian
ROOT=Path(__file__).resolve().parents[1]


def main():
    binary=(ROOT/'evidence/private/binaries/platform_runtime_IS5LEYETRACKER5_service.exe').read_bytes()
    rng=np.random.default_rng(57);points=rng.uniform(-1,1,(12,3)).astype(np.float32)
    first=rng.uniform(-.3,.3,(12,3,3)).astype(np.float32);second=rng.uniform(-.3,.3,(12,3,2)).astype(np.float32)
    r1=np.array([[-1,1]]*3,dtype=np.float32);r2=np.array([[-.2,.7],[-2,1]],dtype=np.float32)
    cases=[]
    for name,q1,q2,mask1,mask2 in [('zero',[0,0,0],[0,0],[1,1,1],[1,1]),
       ('nonzero',[.2,-.4,.6],[.3,-.7],[1,1,1],[1,1]),('clamped',[-10,0,10],[9,-9],[1,1,1],[1,1]),
       ('masked',[.2,-.4,.6],[.3,-.7],[1,0,1],[0,1])]:
        args=(points,[.3,-.4,.6],[.2,-.3,6],1.7,.8)
        deformation=(first,second,q1,q2,r1,r2)
        m,obj,state,out,data,_=make_model(binary,*args,deformation)
        for offset,mask in ((0x70,mask1),(0x48,mask2)):
            mat=struct.unpack('<Q',m.vm.mem_read(obj+offset,8))[0];ptr=struct.unpack('<Q',m.vm.mem_read(mat+0x18,8))[0]
            m.vm.mem_write(ptr,struct.pack('<'+'i'*len(mask),*mask))
        jac,jdata=m.matrix(24,11,[0]*(24*11));m.call(0x1400e07a0,obj,jac,out,state)
        actual=np.asarray(m.floats(jdata,24*11)).reshape(24,11)
        expected,prediction=model_jacobian(*args,first,q1,r1,second,q2,r2,mask1,mask2)
        error=float(np.max(np.abs(actual-expected)));print(name,error,flush=True)
        np.testing.assert_allclose(actual,expected,rtol=1e-4,atol=1e-5)
        np.testing.assert_allclose(np.asarray(m.floats(data,24)).reshape(12,2),prediction,rtol=1e-5,atol=1e-5)
        cases.append({'input':name,'max_absolute_error':error,'passes':True,'allocator_calls':m.calls})
    report={'runtime_sha256':SHA256,'entry':'0x1400e07a0','cases':cases,
        'limitations':'Synthetic bases and full 11-column update matrix. Preserves observed tanh-of-basis behavior; does not assert this matrix is the mathematical derivative of projection.'}
    (ROOT/'evidence/private/pose-deformation-jacobian-oracle.json').write_text(json.dumps(report,indent=2)+'\n')
if __name__=='__main__':main()
