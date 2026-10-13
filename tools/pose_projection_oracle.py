#!/usr/bin/env python3
"""Compare complete FaceModelNoVel projection on neutral mesh and pose states."""
import json
from pathlib import Path
import struct
import numpy as np
from ttp_encoder_oracle import SHA256
from vsmat_oracle import MatrixMachine
from tobii_linux.face_assets import read_mesh,read_fdp,read_landmark_ids,landmark_vertices
from tobii_linux.pose_projection import project_points,bounded_coefficients,deform_vertices
ROOT=Path(__file__).resolve().parents[1]


def make_model(binary,points,angles,translation,focal,scale,deformations=None):
    m=MatrixMachine(binary);obj=m.allocate(0xb8)
    m.vm.mem_write(obj,struct.pack('<Q',0x141135ad8))
    m.vm.mem_write(obj+8,struct.pack('<BI',1,0))
    m.vm.mem_write(obj+0xc,struct.pack('<IIII',1,6,6,6))
    m.vm.mem_write(obj+0x30,struct.pack('<I',len(points)))
    m.vm.mem_write(obj+0x1c,struct.pack('<I',2*len(points)))
    m.vm.mem_write(obj+0x88,struct.pack('<ff',focal,scale))
    def matrix_at(offset,rows,cols,values):
        mat,data=m.matrix(rows,cols,values);m.vm.mem_write(obj+offset,struct.pack('<Q',mat));return data
    n=len(points)*3
    matrix_at(0x28,n,1,points.flatten())
    matrix_at(0x38,n,1,[0]*n);matrix_at(0x40,1,1,[0])
    matrix_at(0x60,n,1,[0]*n);matrix_at(0x68,1,1,[0])
    matrix_at(0x90,6,1,[1]*6)
    matrix_at(0x98,2*len(points),1,[1]*(2*len(points)))
    matrix_at(0xa0,n,1,[0]*n);matrix_at(0xa8,n,1,[0]*n)
    transformed=matrix_at(0xb0,n,1,[0]*n)
    state_values=list(angles)+list(translation)
    if deformations is not None:
        first,second,qfirst,qsecond,boundsfirst,boundssecond=deformations
        nf,ns=len(qfirst),len(qsecond)
        m.vm.mem_write(obj+0xc,struct.pack('<IIII',7,6,6+nf,6+nf+ns))
        m.vm.mem_write(obj+0x50,struct.pack('<I',ns));m.vm.mem_write(obj+0x78,struct.pack('<I',nf))
        matrix_at(0x60,n,nf,first.flatten());matrix_at(0x68,nf,1,[0]*nf)
        matrix_at(0x38,n,ns,second.flatten());matrix_at(0x40,ns,1,[0]*ns)
        for offset,bounds in ((0x80,boundsfirst),(0x58,boundssecond)):
            bounds=np.asarray(bounds,dtype=np.float32)
            intervals=np.column_stack(((bounds[:,1]-bounds[:,0])*np.float32(.5),(bounds[:,1]+bounds[:,0])*np.float32(.5)))
            matrix_at(offset,len(bounds)*2,1,intervals.flatten())
        state_values+=list(qfirst)+list(qsecond)
        matrix_at(0x90,len(state_values),1,[1]*len(state_values))
        for offset,count in ((0x70,nf),(0x48,ns)):
            flag_data=matrix_at(offset,count,1,[0]*count)
            m.vm.mem_write(flag_data,struct.pack('<'+'i'*count,*([1]*count)))
    state,_=m.matrix(len(state_values),1,state_values);out,data=m.matrix(2*len(points),1,[0]*(2*len(points)))
    return m,obj,state,out,data,transformed


def original(binary,points,angles,translation,focal,scale,deformations=None):
    m,obj,state,out,data,transformed=make_model(binary,points,angles,translation,focal,scale,deformations)
    n=len(points)*3
    m.call(0x1400e03d0,obj,out,state)
    return np.array(m.floats(data,2*len(points))).reshape(-1,2),np.array(m.floats(transformed,n)).reshape(-1,3),m.calls


def main():
    support=ROOT/'models/tobii-et5/support'
    points=np.asarray(landmark_vertices(read_mesh(support/'jk_300.wfm'),read_fdp(support/'jk_300_rigid.fdp'),read_landmark_ids(support/'bdtsdata/NN/vnn_nir/landmarks.bin')),dtype=np.float32)
    binary=(ROOT/'evidence/private/binaries/platform_runtime_IS5LEYETRACKER5_service.exe').read_bytes()
    cases=[]
    for name,angles,t,focal,scale in [('neutral',[0,0,0],[0,0,4],1,1),('pitch',[.3,0,0],[0,0,4],1,1),
        ('yaw',[0,-.4,0],[0,0,4],1,1),('roll',[0,0,.6],[0,0,4],1,1),('combined',[.3,-.4,.6],[.2,-.3,6],1.7,.8)]:
        actual,camera,calls=original(binary,points,angles,t,focal,scale)
        expected,expected_camera=project_points(points,angles,t,focal,scale)
        error=float(np.max(np.abs(actual-expected)));camera_error=float(np.max(np.abs(camera-expected_camera)))
        print(name,error,camera_error,flush=True)
        np.testing.assert_allclose(actual,expected,rtol=1e-5,atol=1e-5)
        np.testing.assert_allclose(camera,expected_camera,rtol=1e-5,atol=1e-5)
        cases.append({'input':name,'max_projection_error':error,'max_camera_error':camera_error,'passes':True,'allocator_calls':calls})
    rng=np.random.default_rng(57)
    first=rng.uniform(-.1,.1,(*points.shape,3)).astype(np.float32)
    second=rng.uniform(-.1,.1,(*points.shape,2)).astype(np.float32)
    boundsfirst=np.array([[-1,1],[-1,1],[-1,1]],dtype=np.float32)
    boundssecond=np.array([[-.2,.7],[-2,1]],dtype=np.float32)
    for name,qfirst,qsecond in [('deformed',[.2,-.4,.6],[.3,-.7]),('clamped',[-10,0,10],[9,-9])]:
        cfirst,_=bounded_coefficients(qfirst,boundsfirst);csecond,_=bounded_coefficients(qsecond,boundssecond)
        deformed=deform_vertices(points,first,cfirst,second,csecond)
        actual,camera,calls=original(binary,points,[.3,-.4,.6],[.2,-.3,6],1.7,.8,
                                   (first,second,qfirst,qsecond,boundsfirst,boundssecond))
        expected,expected_camera=project_points(deformed,[.3,-.4,.6],[.2,-.3,6],1.7,.8)
        error=float(np.max(np.abs(actual-expected)));camera_error=float(np.max(np.abs(camera-expected_camera)))
        print(name,error,camera_error,flush=True)
        np.testing.assert_allclose(actual,expected,rtol=1e-5,atol=1e-5)
        np.testing.assert_allclose(camera,expected_camera,rtol=1e-5,atol=1e-5)
        cases.append({'input':name,'max_projection_error':error,'max_camera_error':camera_error,'passes':True,'allocator_calls':calls})
    report={'runtime_sha256':SHA256,'entry':'0x1400e03d0','cases':cases,
      'limitations':'Installed mesh landmark vertices with synthetic pose/calibration and two synthetic deformation bases. Original state decode, trigonometry, matrix operations and projection execute; allocator/free stubs available. No pose estimation, deformation fit or live camera calibration.'}
    (ROOT/'evidence/private/pose-projection-oracle.json').write_text(json.dumps(report,indent=2)+'\n')
if __name__=='__main__':main()
