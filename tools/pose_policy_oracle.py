#!/usr/bin/env python3
"""Exercise original outer fitting loop with complete D1 landmark records."""
import json
from dataclasses import replace
from pathlib import Path
import struct
import numpy as np
from unicorn import UC_HOOK_CODE
from pose_projection_oracle import make_model, SHA256
from tobii_linux.pose_model import load_pose_model
from tobii_linux.pose_projection import model_jacobian, bounded_coefficients
from tobii_linux.pose_policy import fit_landmarks
ROOT=Path(__file__).resolve().parents[1]


def original(binary,model,points,start,width,height,iterations):
    m,obj,state,_,_,_=make_model(binary,model.vertices,start[:3],start[3:6],model.focal_factor,1,
        (model.shape_basis,model.animation_basis,start[6:46],start[46:],model.shape_ranges,model.animation_ranges))
    def put_matrix(owner,offset,rows,cols,values):
        mat,data=m.matrix(rows,cols,np.asarray(values).flatten());m.vm.mem_write(owner+offset,struct.pack('<Q',mat));return mat,data
    for offset,mask in ((0x70,model.shape_mask),(0x48,model.animation_mask)):
        _,data=put_matrix(obj,offset,len(mask),1,np.zeros(len(mask)));m.vm.mem_write(data,np.asarray(mask,dtype='<i4').tobytes())
    put_matrix(obj,0x90,69,1,model.state_sensitivity)
    outer=m.allocate(0x80);parent=m.allocate(0x168);fitter=m.allocate(0x98)
    m.vm.mem_write(outer+0x10,struct.pack('<QQQ',fitter,obj,parent));m.vm.mem_write(outer+0x28,struct.pack('<I',57))
    obs,_=put_matrix(outer,0x30,114,1,np.zeros(114))
    m.vm.mem_write(parent+0x50,struct.pack('<ff',model.focal_factor,1));m.vm.mem_write(parent+0x58,start[:6].tobytes())
    m.vm.mem_write(parent+0xc8,struct.pack('<I',57))
    shape,_=bounded_coefficients(start[6:46],model.shape_ranges);animation,_=bounded_coefficients(start[46:],model.animation_ranges)
    for offset,rows,cols,values in ((0,171,1,model.vertices),(8,171,1,model.vertices),
        (0x10,171,40,model.shape_basis),(0x18,171,23,model.animation_basis),
        (0x90,40,1,shape),(0x98,23,1,animation),(0xa0,171,1,np.zeros(171)),
        (0xa8,171,1,np.zeros(171)),(0xb8,114,1,np.zeros(114)),
        (0x140,6,1,model.rotation_limits),(0x148,6,1,model.translation_limits)):
        put_matrix(parent,offset,rows,cols,values)
    counts=struct.unpack('<14i',m.vm.mem_read(0x14112f050,56))
    def fdp():
        header=m.allocate(0x1a0);groups={}
        for g,n in enumerate(counts,2):
            groups[g]=m.allocate(n*0x48);m.vm.mem_write(header+0x120+8*g,struct.pack('<Q',groups[g]))
        return header,groups
    source,groups=fdp();mapped,mapgroups=fdp();m.vm.mem_write(parent+0xd0,struct.pack('<Q',mapped))
    for i,(g,index) in enumerate(model.landmark_ids):
        m.vm.mem_write(groups[g]+(index-1)*0x48,struct.pack('<fffI',*points[i],-1,1))
        r=mapgroups[g]+(index-1)*0x48
        m.vm.mem_write(r+0xc,struct.pack('<I',1));m.vm.mem_write(r+0x18,struct.pack('<i',i));m.vm.mem_write(r+0x38,struct.pack('<Q',15))
    sentinel=m.allocate(0x28);m.vm.mem_write(sentinel+0x19,b'\1')
    def tree(items,parent_node):
        if not items:return sentinel
        mid=len(items)//2;identifier,slot=items[mid];node=m.allocate(0x28)
        left=tree(items[:mid],node);right=tree(items[mid+1:],node)
        m.vm.mem_write(node,struct.pack('<QQQ',left,parent_node,right));m.vm.mem_write(node+0x1c,struct.pack('<iii',*identifier,slot));return node
    root=tree(sorted((identifier,i) for i,identifier in enumerate(model.landmark_ids)),sentinel)
    m.vm.mem_write(sentinel,struct.pack('<QQQ',sentinel,root,sentinel));m.vm.mem_write(outer+0x68,struct.pack('<QQ',sentinel,57))
    m.call(0x1400df430,fitter,obj)
    smat=struct.unpack('<Q',m.vm.mem_read(fitter+0x10,8))[0];data=struct.unpack('<Q',m.vm.mem_read(smat+0x18,8))[0]
    m.vm.mem_write(data,start.tobytes());m.vm.mem_write(fitter+9,b'\1')
    calls={'updates':0,'resets':0,'finalizes':0}
    def count(vm,address,size,_):
        key={0x1400df6b0:'updates',0x140052d70:'resets',0x140052710:'finalizes'}[address]
        calls[key]+=1
    for address in (0x1400df6b0,0x140052d70,0x140052710):m.vm.hook_add(UC_HOOK_CODE,count,begin=address,end=address)
    m.call(0x1400dc3b0,outer,source,width,height,0,iterations,instruction_limit=100_000_000,timeout_us=30_000_000)
    actual=np.asarray(m.floats(data,69),dtype=np.float32)
    return actual,calls


def main():
    model=load_pose_model(ROOT/'models/tobii-et5/support')
    binary=(ROOT/'evidence/private/binaries/platform_runtime_IS5LEYETRACKER5_service.exe').read_bytes()
    target=model.initial_state([.025,np.pi-.03,.02,.008,-.005,.63])
    _,observed=model_jacobian(model.vertices,target[:3],target[3:6],model.focal_factor,1,
        model.shape_basis,target[6:46],model.shape_ranges,model.animation_basis,target[46:],model.animation_ranges)
    points=(np.float32(1)-observed/np.array([640/480,1],dtype=np.float32))*np.float32(.5)
    cases=[]
    for name,pose,iterations in [('normal',[0,np.pi,0,0,0,.65],2),
            ('retry',[2,np.pi,0,0,0,.65],0),('retry-deformed',[2,np.pi,0,0,0,.65],0),
            ('retry-still-invalid',[2,np.pi,0,0,0,.65],0)]:
        configured=model
        if name=='retry-still-invalid':
            limits=model.rotation_limits.copy();limits[0]=[10,11]
            configured=replace(model,rotation_limits=limits)
        start=configured.initial_state(pose)
        if name=='retry-deformed':start[6:10]=[.1,-.2,.3,-.1]
        actual,calls=original(binary,configured,points,start,640,480,iterations)
        expected=fit_landmarks(configured,points,640,480,start,iterations)
        error=float(np.max(np.abs(actual-expected['state'])))
        print(name,error,calls,flush=True)
        np.testing.assert_allclose(actual,expected['state'],atol=1e-5,rtol=1e-5)
        assert calls=={'updates':expected['updates'],'resets':expected['attempts']-1,'finalizes':int(expected['valid'])}
        cases.append({'input':name,'max_absolute_error':error,'calls':calls,'passes':True})
    (ROOT/'evidence/private/pose-policy-oracle.json').write_text(json.dumps({'runtime_sha256':SHA256,
        'entry':'0x1400dc3b0','cases':cases,'limitations':'Complete 57-point records, installed coefficients, no external pose hint. Structures built by harness. No original loader, live selection, quality gating or public coordinate conversion.'},indent=2)+'\n')
if __name__=='__main__':main()
