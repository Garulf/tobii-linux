#!/usr/bin/env python3
"""Execute original bounds and coefficient cleanup policy."""
import json
from pathlib import Path
import struct
import numpy as np
from unicorn.x86_const import UC_X86_REG_RAX
from vsmat_oracle import MatrixMachine
from ttp_encoder_oracle import SHA256
from tobii_linux.pose_model import load_pose_model
from tobii_linux.pose_policy import check_pose_bounds
ROOT=Path(__file__).resolve().parents[1]


def main():
    binary=(ROOT/'evidence/private/binaries/platform_runtime_IS5LEYETRACKER5_service.exe').read_bytes()
    model=load_pose_model(ROOT/'models/tobii-et5/support')
    neutral=np.array([0,np.pi,0,0,0,.65],dtype=np.float32)
    inputs=[('neutral',neutral,[0]*40,[0]*23)]
    limits=np.concatenate((model.rotation_limits,model.translation_limits))
    for axis in (0,1,3,4,5):
        for side in (0,1):
            state=neutral.copy();state[axis]=limits[axis,side]
            inputs.append((f'axis-{axis}-edge-{side}',state,[0]*40,[0]*23))
            state=state.copy();state[axis]+=np.float32(-.01 if side==0 else .01)
            inputs.append((f'axis-{axis}-outside-{side}',state,[0]*40,[0]*23))
    for roll in (-10,-3.3,3.3,10):
        state=neutral.copy();state[2]=roll;inputs.append((f'roll-{roll}',state,[0]*40,[0]*23))
    for axis in (0,1,3):
        state=neutral.copy();state[axis]=np.nan;inputs.append((f'nan-axis-{axis}',state,[0]*40,[0]*23))
    inputs.append(('bad-coefficients',neutral,[np.nan,np.inf,-np.inf]+[0]*37,[np.nan]+[0]*22))
    cases=[]
    for name,state,shape,animation in inputs:
        m=MatrixMachine(binary);outer=m.allocate(0x80);parent=m.allocate(0x168)
        m.vm.mem_write(outer+0x20,struct.pack('<Q',parent));m.vm.mem_write(parent+0x58,state.tobytes())
        data=[]
        for offset,values in ((0x140,model.rotation_limits.flatten()),(0x148,model.translation_limits.flatten()),(0x90,shape),(0x98,animation)):
            mat,p=m.matrix(len(values),1,values);m.vm.mem_write(parent+offset,struct.pack('<Q',mat));data.append(p)
        m.call(0x1400dcc30,outer)
        actual=bool(m.vm.reg_read(UC_X86_REG_RAX)&255)
        expected,s,a=check_pose_bounds(state,shape,animation,model.rotation_limits,model.translation_limits)
        assert actual==expected,(name,actual,expected)
        np.testing.assert_array_equal(m.floats(data[2],40),s);np.testing.assert_array_equal(m.floats(data[3],23),a)
        np.testing.assert_array_equal(np.frombuffer(m.vm.mem_read(parent+0x58,24),dtype='<f4'),state)
        print(name,actual,flush=True);cases.append({'input':name,'invalid':actual,'byte_exact_cleanup':True})
    (ROOT/'evidence/private/pose-bounds-oracle.json').write_text(json.dumps({'runtime_sha256':SHA256,
        'entry':'0x1400dcc30','cases':cases,'limitations':'Installed bounds, finite roll, selected NaN axes and invalid coefficients. CRT error handling for nonfinite roll not executed.'},indent=2)+'\n')
if __name__=='__main__':main()
