#!/usr/bin/env python3
"""Original rigid six-state Jacobian differential check."""
import json
from pathlib import Path
import numpy as np
from pose_projection_oracle import make_model,SHA256
from tobii_linux.pose_projection import rigid_jacobian
ROOT=Path(__file__).resolve().parents[1]


def main():
    binary=(ROOT/'evidence/private/binaries/platform_runtime_IS5LEYETRACKER5_service.exe').read_bytes()
    points=np.random.default_rng(57).uniform(-1,1,(57,3)).astype(np.float32)
    cases=[]
    for angles in ([0,0,0],[.3,0,0],[0,-.4,0],[0,0,.6],[.3,-.4,.6]):
        m,obj,state,out,_,_=make_model(binary,points,angles,[.2,-.3,6],1.7,.8)
        jac,data=m.matrix(114,6,[0]*(114*6))
        m.call(0x1400e07a0,obj,jac,out,state)
        actual=np.array(m.floats(data,114*6)).reshape(114,6)
        expected=rigid_jacobian(points,angles,[.2,-.3,6],1.7,.8)
        error=float(np.max(np.abs(actual-expected)));print(angles,error,flush=True)
        np.testing.assert_allclose(actual,expected,rtol=1e-4,atol=1e-5)
        cases.append({'rotation':angles,'max_absolute_error':error,'passes':True,'allocator_calls':m.calls})
    report={'runtime_sha256':SHA256,'entry':'0x1400e07a0','cases':cases,
            'limitations':'Six rigid pose columns, finite synthetic points, observation sensitivities set to one. No deformation columns or observation exclusion yet.'}
    (ROOT/'evidence/private/pose-jacobian-oracle.json').write_text(json.dumps(report,indent=2)+'\n')
if __name__=='__main__':main()
