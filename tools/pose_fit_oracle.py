#!/usr/bin/env python3
"""Original complete six-state weighted fitting update comparison."""
import json
from pathlib import Path
import struct
import numpy as np
from pose_projection_oracle import make_model,SHA256
from tobii_linux.pose_projection import project_points
from tobii_linux.pose_fit import rigid_step
ROOT=Path(__file__).resolve().parents[1]


def main():
    binary=(ROOT/'evidence/private/binaries/platform_runtime_IS5LEYETRACKER5_service.exe').read_bytes()
    points=np.random.default_rng(57).uniform(-1,1,(12,3)).astype(np.float32)
    cases=[]
    for name,target,ss,obs_s in [('neutral',[0,0,0,0,0,6],[1]*6,[1]*24),
       ('offset',[.15,-.2,.1,.1,-.2,5.5],[1]*6,[1]*24),
       ('weighted',[.15,-.2,.1,.1,-.2,5.5],[.5,.9,2,1,1,1],np.linspace(.2,2,24)),
       ('excluded',[.15,-.2,.1,.1,-.2,5.5],[1]*6,[100001,.3]+[1]*22)]:
        start=np.array([0,0,0,0,0,6],dtype=np.float32)
        observations,_=project_points(points,target[:3],target[3:],1.7,.8)
        m,obj,state,_,_,_=make_model(binary,points,start[:3],start[3:],1.7,.8)
        for offset,values in ((0x90,ss),(0x98,obs_s)):
            mat=struct.unpack('<Q',m.vm.mem_read(obj+offset,8))[0]
            data=struct.unpack('<Q',m.vm.mem_read(mat+0x18,8))[0]
            m.vm.mem_write(data,struct.pack('<'+'f'*len(values),*values))
        fitter=m.allocate(0x98);m.call(0x1400df430,fitter,obj)
        state_matrix=struct.unpack('<Q',m.vm.mem_read(fitter+0x10,8))[0]
        state_data=struct.unpack('<Q',m.vm.mem_read(state_matrix+0x18,8))[0]
        m.vm.mem_write(state_data,start.tobytes());m.vm.mem_write(fitter+9,b'\1')
        observation,_=m.matrix(24,1,observations.flatten())
        for iteration in range(3):
            before=np.asarray(m.floats(state_data,6),dtype=np.float32)
            expected=rigid_step(points,observations,before,1.7,.8,ss,obs_s)
            m.call(0x1400df6b0,fitter,observation)
            actual=np.asarray(m.floats(state_data,6),dtype=np.float32)
            error=float(np.max(np.abs(actual-expected)));print(name,iteration,error,actual.tolist(),flush=True)
            np.testing.assert_allclose(actual,expected,rtol=1e-5,atol=1e-5)
            cases.append({'input':name,'iteration':iteration,'max_absolute_error':error,'passes':True,'state':actual.tolist()})
    report={'runtime_sha256':SHA256,'entry':'0x1400df6b0','cases':cases,
        'limitations':'Synthetic rigid states and observations; positive sensitivities. Original Jacobian, inversion and matrix update execute; allocator/free stubs only. No outer convergence/reinitialization policy or camera normalization.'}
    (ROOT/'evidence/private/pose-fit-oracle.json').write_text(json.dumps(report,indent=2)+'\n')
if __name__=='__main__':main()
