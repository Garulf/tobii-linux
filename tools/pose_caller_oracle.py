#!/usr/bin/env python3
"""Original no-hint single-parent fitting caller through its output boundary."""
import json
from pathlib import Path
import struct
import numpy as np
from unicorn import UC_HOOK_CODE
from unicorn.x86_const import UC_X86_REG_RCX,UC_X86_REG_RIP
from vsmat_oracle import MatrixMachine
from ttp_encoder_oracle import SHA256,STOP
from tobii_linux.pose_model import load_pose_model
from tobii_linux.pose_projection import model_jacobian
from tobii_linux.pose_policy import fit_staged_landmarks
ROOT=Path(__file__).resolve().parents[1]


def original(binary,model,points,width,height):
    m=MatrixMachine(binary);parent=m.allocate(0x168);caller=m.allocate(0x200)
    m.vm.mem_write(caller+8,struct.pack('<Q',parent))
    m.vm.mem_write(parent+0x50,struct.pack('<ff',model.focal_factor,1));m.vm.mem_write(parent+0xc8,struct.pack('<I',57))
    pointers={}
    def put(offset,rows,cols,values,integer=False):
        mat,data=m.matrix(rows,cols,np.asarray(values).flatten())
        if integer:m.vm.mem_write(data,np.asarray(values,dtype='<i4').tobytes())
        m.vm.mem_write(parent+offset,struct.pack('<Q',mat));pointers[offset]=data
    for offset,rows,cols,values in ((0,171,1,model.vertices),(8,171,1,model.vertices),
        (0x10,171,40,model.shape_basis),(0x18,171,23,model.animation_basis),
        (0x90,40,1,np.zeros(40)),(0x98,23,1,np.zeros(23)),
        (0xa0,171,1,np.zeros(171)),(0xa8,171,1,np.zeros(171)),(0xb8,114,1,np.zeros(114)),
        (0x110,6,1,model.state_sensitivity[:6]),(0x118,23,1,model.state_sensitivity[46:]),
        (0x120,40,1,model.state_sensitivity[6:46]),(0x140,6,1,model.rotation_limits),
        (0x148,6,1,model.translation_limits),(0x150,46,1,model.animation_ranges)):
        put(offset,rows,cols,values)
    put(0x30,23,1,model.animation_mask,True);put(0x38,40,1,model.shape_mask,True)
    counts=struct.unpack('<14i',m.vm.mem_read(0x14112f050,56))
    def fdp(header=None):
        header=m.allocate(0x1a0) if header is None else header;groups={}
        for group,count in enumerate(counts,2):
            groups[group]=m.allocate(count*72);m.vm.mem_write(header+0x120+group*8,struct.pack('<Q',groups[group]))
            for i in range(count):m.vm.mem_write(groups[group]+i*72+0x38,struct.pack('<Q',15))
        return header,groups
    source,groups=fdp();mapped,mapgroups=fdp();fdp(caller+0x60)
    m.vm.mem_write(parent+0xd0,struct.pack('<Q',mapped))
    for i,(group,index) in enumerate(model.landmark_ids):
        r=groups[group]+(index-1)*72;m.vm.mem_write(r,struct.pack('<fffIIf',*points[i],-1,1,1,1))
        r=mapgroups[group]+(index-1)*72;m.vm.mem_write(r+0xc,struct.pack('<I',1));m.vm.mem_write(r+0x18,struct.pack('<i',i))
    m.call(0x14007e270,caller)
    reached=[];modes=[]
    def boundary(vm,address,size,_):
        reached.append(True);vm.reg_write(UC_X86_REG_RIP,STOP)
    def count(vm,address,size,_):
        fitter=vm.reg_read(UC_X86_REG_RCX);obj=struct.unpack('<Q',vm.mem_read(fitter,8))[0]
        modes.append(struct.unpack('<I',vm.mem_read(obj+0xc,4))[0])
        if len(modes)==1:
            arrays={}
            for owner,offset,name in ((obj,0x28,'vertices'),(obj,0x60,'shape_basis'),(obj,0x38,'animation_basis'),
                (obj,0x90,'sensitivity'),(obj,0x98,'weights'),(obj,0x68,'shape'),(obj,0x40,'animation'),
                (fitter,0x10,'state')):
                header=struct.unpack('<Q',vm.mem_read(owner+offset,8))[0]
                data=struct.unpack('<Q',vm.mem_read(header+0x18,8))[0]
                rows,cols=struct.unpack('<ii',vm.mem_read(header+0x20,8))
                arrays[name]=np.asarray(m.floats(data,rows*cols),dtype=np.float32).reshape(rows,cols)
            arrays['focal_scale']=np.asarray(m.floats(obj+0x88,2),dtype=np.float32)
            np.savez(ROOT/'evidence/private/pose-caller-first-update.npz',**arrays)

    m.vm.hook_add(UC_HOOK_CODE,boundary,begin=0x140055470,end=0x140055470)
    m.vm.hook_add(UC_HOOK_CODE,count,begin=0x1400df6b0,end=0x1400df6b0)
    m.call(0x14007e840,caller,source,width,height,0,instruction_limit=100_000_000,timeout_us=30_000_000)
    assert reached,'Caller failed before output boundary'
    return {'rigid_state':np.asarray(m.floats(parent+0x58,6),dtype=np.float32),
        'shape_coefficients':np.asarray(m.floats(pointers[0x90],40),dtype=np.float32),
        'animation_coefficients':np.asarray(m.floats(pointers[0x98],23),dtype=np.float32)},modes


def main():
    model=load_pose_model(ROOT/'models/tobii-et5/support');binary=(ROOT/'evidence/private/binaries/platform_runtime_IS5LEYETRACKER5_service.exe').read_bytes();cases=[]
    for name,angles,translation in [('neutral',[0,np.pi,0],[0,0,.65]),('offset',[.025,np.pi-.03,.02],[.008,-.005,.63])]:
        target=model.initial_state([*angles,*translation])
        _,projected=model_jacobian(model.vertices,target[:3],target[3:6],model.focal_factor,1,
            model.shape_basis,target[6:46],model.shape_ranges,model.animation_basis,target[46:],model.animation_ranges)
        points=(np.float32(1)-projected/np.array([640/480,1],dtype=np.float32))*np.float32(.5)
        actual,modes=original(binary,model,points,640,480)
        expected=fit_staged_landmarks(model,points,640,480)
        errors={key:float(np.max(np.abs(actual[key]-expected[key]))) for key in actual}
        print(name,modes,errors,flush=True)
        for key in actual:np.testing.assert_allclose(actual[key],expected[key],atol=1e-5,rtol=1e-5)
        assert modes==[s['mode'] for s in expected['stages'] for _ in range(s['updates'])]
        cases.append({'input':name,'update_modes':modes,'max_errors':errors,'passes':True})
    (ROOT/'evidence/private/pose-caller-oracle.json').write_text(json.dumps({'runtime_sha256':SHA256,
        'entry':'0x14007e840','stop_before':'0x140055470','cases':cases,
        'limitations':'No-hint single-parent branch through constructor, selection, mode switching, fitting and mesh finalization. Supplied 57-point parent model semantically remaps selected vertex indices. Execution stops before output conversion/auxiliary postprocessing. No live frames, other models, temporal caller or final API validation.'},indent=2)+'\n')
if __name__=='__main__':main()
