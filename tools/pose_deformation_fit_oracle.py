#!/usr/bin/env python3
"""Original complete fitting update with enabled bounded deformation states."""
import json
from pathlib import Path
import struct
import numpy as np
from pose_projection_oracle import make_model,SHA256
from tobii_linux.pose_projection import model_jacobian
from tobii_linux.pose_fit import deformation_step
ROOT=Path(__file__).resolve().parents[1]


def main():
    binary=(ROOT/'evidence/private/binaries/platform_runtime_IS5LEYETRACKER5_service.exe').read_bytes()
    rng=np.random.default_rng(57);points=rng.uniform(-1,1,(12,3)).astype(np.float32)
    first=rng.uniform(-.3,.3,(12,3,3)).astype(np.float32);second=rng.uniform(-.3,.3,(12,3,2)).astype(np.float32)
    r1=np.array([[-1,1]]*3,dtype=np.float32);r2=np.array([[-.2,.7],[-2,1]],dtype=np.float32)
    _,observations=model_jacobian(points,[.15,-.2,.1],[.1,-.2,5.5],1.7,.8,first,[.2,-.3,.1],r1,second,[.3,-.2],r2)
    cases=[]
    for name,q1,q2 in [('neutral',[0,0,0],[0,0]),('clamped',[-10,0,10],[9,-9])]:
        start=np.array([0,0,0,0,0,6]+q1+q2,dtype=np.float32)
        m,obj,_,_,_,_=make_model(binary,points,start[:3],start[3:6],1.7,.8,(first,second,q1,q2,r1,r2))
        fitter=m.allocate(0x98);m.call(0x1400df430,fitter,obj)
        smat=struct.unpack('<Q',m.vm.mem_read(fitter+0x10,8))[0];data=struct.unpack('<Q',m.vm.mem_read(smat+0x18,8))[0]
        m.vm.mem_write(data,start.tobytes());m.vm.mem_write(fitter+9,b'\1')
        obs,_=m.matrix(24,1,observations.flatten())
        for iteration in range(3):
            before=np.asarray(m.floats(data,11),dtype=np.float32)
            expected=deformation_step(points,observations,before,1.7,.8,first,r1,second,r2)
            m.call(0x1400df6b0,fitter,obs)
            actual=np.asarray(m.floats(data,11),dtype=np.float32)
            error=float(np.max(np.abs(actual-expected)));print(name,iteration,error,flush=True)
            np.testing.assert_allclose(actual,expected,rtol=1e-5,atol=1e-5)
            cases.append({'input':name,'iteration':iteration,'max_absolute_error':error,'passes':True})
    report={'runtime_sha256':SHA256,'entry':'0x1400df6b0','cases':cases,
        'limitations':'Full 11-state update on two synthetic bases. Original nonlinear decode, Jacobian, inversion and matrix operations execute. No live image/pose or outer retry policy.'}
    (ROOT/'evidence/private/pose-deformation-fit-oracle.json').write_text(json.dumps(report,indent=2)+'\n')
if __name__=='__main__':main()
