#!/usr/bin/env python3
"""Compare D1 reference normalization against unchanged matrix instructions."""
import json
from pathlib import Path
import numpy as np
from ttp_encoder_oracle import SHA256
from vsmat_oracle import MatrixMachine
from tobii_linux.face_assets import read_shapes
from tobii_linux.d1 import normalize_reference_shape
ROOT=Path(__file__).resolve().parents[1]


def main():
    binary=(ROOT/'evidence/private/binaries/platform_runtime_IS5LEYETRACKER5_service.exe').read_bytes()
    _,values=read_shapes(ROOT/'models/tobii-et5/support/bdtsdata/NN/vnn_nir/init_shape.bin')
    installed=np.array(values,dtype=np.float32).reshape(57,2)
    inputs=[('installed',installed),('translated-scaled',installed*np.float32(1.7)+np.array([200,-45],dtype=np.float32)),
            ('wide',installed*np.array([3,0.2],dtype=np.float32)),
            ('synthetic',np.random.default_rng(57).normal(size=(57,2)).astype(np.float32))]
    cases=[]
    for name,points in inputs:
        machine=MatrixMachine(binary);mat,data=machine.matrix(57,2,points.flatten())
        machine.call(0x1400ac560,0,mat,96)
        actual=np.asarray(machine.floats(data,114),dtype=np.float32).reshape(57,2)
        expected=np.asarray(normalize_reference_shape(points),dtype=np.float32)
        error=float(np.max(np.abs(actual-expected)))
        assert np.allclose(actual,expected,rtol=1e-6,atol=2e-5),(name,error)
        cases.append({'input':name,'max_absolute_error':error,'passes':True,'allocator_calls':machine.calls})
    report={'runtime_sha256':SHA256,'entry':'0x1400ac560','cases':cases,
            'limitations':'Only allocation/free are stubbed; synthetic GS stack limit; original reductions/minmax/scale execute; numerical tolerance not bit parity.'}
    (ROOT/'evidence/private/d1-geometry-oracle.json').write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report,indent=2))
if __name__=='__main__':main()
