#!/usr/bin/env python3
"""Run recovered LBF refinement on explicit synthetic image/landmark inputs."""
import hashlib
import json
from pathlib import Path
import numpy as np
from tobii_linux.lbf import read_lbf
from tobii_linux.lbf_predictor import refine_landmarks
ROOT=Path(__file__).resolve().parents[1]


def main():
    rows,columns,stride=96,128,132
    images={'zero':bytes(rows*stride),
            'ramp':bytes(i%256 for i in range(rows*stride)),
            'pattern':bytes((i*73+i*i)&255 for i in range(rows*stride))}
    cases=[]
    for name in ('fa','fc'):
        path=ROOT/f'models/tobii-et5/support/bdtsdata/NN/{name}.lbf'
        model=read_lbf(path)
        centers=[(30+(i*7)%36+.3,40+(i*11)%48+.7) for i in range(35)]
        for image_name,pixels in images.items():
            for mirrored in (False,True):
                result=np.asarray(refine_landmarks(model,pixels,rows,columns,stride,centers,95.7,mirrored),dtype='<f4')
                if result.shape!=(35,2) or not np.isfinite(result).all():raise AssertionError('Invalid refinement output')
                cases.append({'model':name,'source_sha256':hashlib.sha256(path.read_bytes()).hexdigest(),
                              'image':image_name,'mirrored':mirrored,'output_shape':list(result.shape),
                              'output_sha256':hashlib.sha256(result.tobytes()).hexdigest(),
                              'minimum':float(result.min()),'maximum':float(result.max()),'passes':True})
    report={'kind':'native-linux-synthetic-lbf-refinement','numpy':np.__version__,'cases':cases,
            'input_geometry':{'rows':rows,'columns':columns,'stride':stride,'scale':95.7},
            'limitations':'Synthetic caller-supplied landmarks and image geometry; shape/finite checks; not real camera initialization, head pose, or 1:1 parity.'}
    (ROOT/'models/tobii-et5/lbf-validation.json').write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps(report,indent=2))
if __name__=='__main__':main()
