#!/usr/bin/env python3
"""Check reduced fitting modes on installed model and fixed inactive coefficients."""
import json
from pathlib import Path
import struct
import numpy as np
from pose_projection_oracle import make_model,SHA256
from tobii_linux.pose_model import load_pose_model,coefficient_latents
from tobii_linux.pose_projection import model_jacobian
from tobii_linux.pose_fit import mode_step,prepare_observations
ROOT=Path(__file__).resolve().parents[1]


def original(binary,model,observed,pose,shape,animation,mode,weights,retained=None):
    qs=coefficient_latents(shape,model.shape_ranges);qa=coefficient_latents(animation,model.animation_ranges)
    m,obj,_,_,_,_=make_model(binary,model.vertices,pose[:3],pose[3:],model.focal_factor,1,
        (model.shape_basis,model.animation_basis,qs,qa,model.shape_ranges,model.animation_ranges))
    active=np.r_[np.arange(6),np.arange(6,46) if mode==3 else np.arange(46,69)]
    state=np.concatenate((pose,qs,qa))[active] if retained is None else retained.copy()
    m.vm.mem_write(obj+0xc,struct.pack('<IIII',mode,6,46 if mode==3 else 6,len(active)))
    def write(offset,values,integers=False):
        mat,data=m.matrix(len(values),1,values)
        if integers:m.vm.mem_write(data,np.asarray(values,dtype='<i4').tobytes())
        m.vm.mem_write(obj+offset,struct.pack('<Q',mat));return mat,data
    write(0x68,shape);write(0x40,animation)
    write(0x90,model.state_sensitivity[active]);write(0x98,weights)
    write(0x70,model.shape_mask,True);write(0x48,model.animation_mask,True)
    fitter=m.allocate(0x98);m.call(0x1400df430,fitter,obj)
    smat=struct.unpack('<Q',m.vm.mem_read(fitter+0x10,8))[0];data=struct.unpack('<Q',m.vm.mem_read(smat+0x18,8))[0]
    m.vm.mem_write(data,state.tobytes());m.vm.mem_write(fitter+9,b'\1')
    obs,_=m.matrix(observed.size,1,observed.flatten())
    m.call(0x1400df6b0,fitter,obs,instruction_limit=30_000_000)
    rotation=m.allocate(12);translation=m.allocate(12)
    sm,sd=m.matrix(40,1,shape);am,ad=m.matrix(23,1,animation)
    m.call(0x1400dff70,obj,rotation,translation,sm,am,smat)
    return {'active_state':np.asarray(m.floats(data,len(active)),dtype=np.float32),
            'rigid_state':np.array([*m.floats(rotation,3),*m.floats(translation,3)],dtype=np.float32),
            'shape_coefficients':np.asarray(m.floats(sd,40),dtype=np.float32),
            'animation_coefficients':np.asarray(m.floats(ad,23),dtype=np.float32)}


def main():
    model=load_pose_model(ROOT/'models/tobii-et5/support');binary=(ROOT/'evidence/private/binaries/platform_runtime_IS5LEYETRACKER5_service.exe').read_bytes()
    target=model.initial_state([.025,np.pi-.03,.02,.008,-.005,.63])
    _,observed=model_jacobian(model.vertices,target[:3],target[3:6],model.focal_factor,1,
        model.shape_basis,target[6:46],model.shape_ranges,model.animation_basis,target[46:],model.animation_ranges)
    _,weights=prepare_observations(np.full((57,2),.5),model.landmark_ids,640,480)
    cases=[]
    for name,shape,animation in [('zero',np.zeros(40,dtype=np.float32),np.zeros(23,dtype=np.float32)),
        ('deformed',np.linspace(-.2,.2,40,dtype=np.float32),np.linspace(-.02,.02,23,dtype=np.float32))]:
        pose=np.array([0,np.pi,0,0,0,.65],dtype=np.float32)
        retained=None
        for stage,mode in enumerate((3,3,5)):
            if stage==2:retained=None
            actual=original(binary,model,observed,pose,shape,animation,mode,weights,retained)
            expected=mode_step(model,observed,pose,shape,animation,mode,weights,retained)
            errors={key:float(np.max(np.abs(actual[key]-expected[key]))) for key in actual}
            print(name,stage,mode,errors,flush=True)
            for key in actual:np.testing.assert_allclose(actual[key],expected[key],atol=1e-5,rtol=1e-5)
            fixed='animation_coefficients' if mode==3 else 'shape_coefficients'
            np.testing.assert_array_equal(actual[fixed],animation if mode==3 else shape)
            cases.append({'input':name,'stage':stage,'mode':mode,'max_errors':errors,'passes':True})
            retained=actual['active_state']
            pose=actual['rigid_state'];shape=actual['shape_coefficients'];animation=actual['animation_coefficients']
    (ROOT/'evidence/private/pose-modes-oracle.json').write_text(json.dumps({'runtime_sha256':SHA256,
        'entries':['0x1400df6b0','0x1400dff70'],'cases':cases,'limitations':'Installed 57-point model in reduced 46/29-state updates. Three stages are orchestrated by harness, not the original high-level mode selector. Actual selected point list, hints, other models and live frames remain unverified.'},indent=2)+'\n')
if __name__=='__main__':main()
