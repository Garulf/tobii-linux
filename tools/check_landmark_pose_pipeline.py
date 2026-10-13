#!/usr/bin/env python3
"""Connect native crop/inference/landmarks/fitting on synthetic frames only."""
import hashlib
import json
from pathlib import Path
import time
import numpy as np
import openvino as ov
from tobii_linux.pose_policy import fit_staged_landmarks
from tobii_linux.headpose import NativeD1Pose
from tobii_linux.d1_quality import d1_quality_score, passes_d1_quality
ROOT=Path(__file__).resolve().parents[1]


def main():
    support=ROOT/'models/tobii-et5/support'
    component=NativeD1Pose(ROOT/'models/tobii-et5')
    model=component.pose_model
    cases=[]
    for name,pixels in [('zero',bytes(160*120)),('ramp',bytes((x+3*y)%256 for y in range(120) for x in range(160))),
                        ('noise',np.random.default_rng(57).integers(0,256,160*120,dtype=np.uint8).tobytes())]:
        started=time.monotonic()
        pipeline=component.predict_frame(pixels,160,120,(30,20,65,80))
        assert not pipeline.quality_accepted and pipeline.fitting_result is None
        result=pipeline.landmarks
        score=d1_quality_score(result.quality,model.landmark_ids)
        fitted=fit_staged_landmarks(model,result.normalized_points,160,120)
        assert fitted['rigid_state'].shape==(6,) and all(np.isfinite(fitted[k]).all() for k in ('rigid_state','shape_coefficients','animation_coefficients'))
        cases.append({'input':name,'input_sha256':hashlib.sha256(pixels).hexdigest(),
            'state_count':69,'all_finite':True,'pose_bounds_valid':fitted['valid'],
            'fitting_stages':fitted['stages'],'component_skips_rejected_fit':True,
            'quality_min':float(np.min(result.quality)),'quality_max':float(np.max(result.quality)),
            'd1_quality_score':score,'quality_gate_by_mode':{str(mode):passes_d1_quality(score,mode) for mode in (0,1,2)},
            'tracking_valid':False if not any(passes_d1_quality(score,mode) for mode in (0,1,2)) else None,'seconds':time.monotonic()-started})
        print(name,'completed',fitted['valid'],flush=True)
    report={'openvino':ov.get_version(),'numpy':np.__version__,'device':'CPU','cases':cases,
        'limitations':'Synthetic non-face images and supplied face box. Quality gate is evaluated separately; fitting is deliberately forced only to exercise components. Bounds-valid DOES NOT mean a face is present or tracking is valid. No USB frames, original neural inference parity, temporal policy or public head-pose conversion.'}
    (ROOT/'models/tobii-et5/landmark-pose-validation.json').write_text(json.dumps(report,indent=2)+'\n')
if __name__=='__main__':main()
