#!/usr/bin/env python3
"""Linux D1 crop→inference→image landmarks checks on synthetic frames."""
import hashlib
import json
from pathlib import Path
import time
import numpy as np
import openvino as ov
from tobii_linux.d1 import D1LandmarkModel
from tobii_linux.face_assets import read_shapes
ROOT=Path(__file__).resolve().parents[1]


def main():
    _,reference=read_shapes(ROOT/'models/tobii-et5/support/bdtsdata/NN/vnn_nir/init_shape.bin')
    model=D1LandmarkModel(ROOT/'models/tobii-et5/d1',np.asarray(reference).reshape(57,2))
    cases=[]
    for kind,pixels in [('zero',bytes(160*120)),('ramp',bytes((x+3*y)%256 for y in range(120) for x in range(160))),
                        ('noise',np.random.default_rng(57).integers(0,256,160*120,dtype=np.uint8).tobytes())]:
        for roll in (0,20,-32):
            start=time.monotonic();result=model.predict_frame(pixels,160,120,(30,20,65,80),roll)
            points=np.asarray(result.image_points);quality=np.asarray(result.quality)
            assert points.shape==(57,2) and quality.shape==(57,)
            assert np.isfinite(points).all() and np.isfinite(quality).all()
            assert ((quality>=0)&(quality<=1)).all()
            np.testing.assert_allclose(result.normalized_points,points/np.array([160,120]),rtol=1e-6,atol=1e-7)
            cases.append({'input':kind,'roll_degrees':roll,'input_sha256':hashlib.sha256(pixels).hexdigest(),
                          'landmark_count':57,'quality_min':float(quality.min()),'quality_max':float(quality.max()),
                          'all_finite':True,'seconds':time.monotonic()-start})
            print(kind,roll,'passed',flush=True)
    report={'openvino':ov.get_version(),'numpy':np.__version__,'device':'CPU','precision_hint':'f32','cases':cases,
            'limitations':'Synthetic supplied face boxes only. No original Windows inference comparison, real-image accuracy, face detection, temporal roll, pose or USB acquisition.'}
    (ROOT/'models/tobii-et5/d1-pipeline-validation.json').write_text(json.dumps(report,indent=2)+'\n')
if __name__=='__main__':main()
