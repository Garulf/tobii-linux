#!/usr/bin/env python3
"""Run original 0x14007e440 over persistent synthetic frame sequences."""
from dataclasses import replace
import json
from pathlib import Path
import struct
import numpy as np
from unicorn import UC_HOOK_CODE
from unicorn.x86_const import UC_X86_REG_RAX,UC_X86_REG_RCX,UC_X86_REG_RSP,UC_X86_REG_XMM0,UC_X86_REG_XMM6
from vsmat_oracle import MatrixMachine
from ttp_encoder_oracle import SHA256
from tobii_linux.pose_model import load_pose_model
from tobii_linux.pose_projection import model_jacobian
from tobii_linux.pose_temporal import FLOAT_PI,TemporalPoseFitter
ROOT=Path(__file__).resolve().parents[1]
HEAP,HEAP_END=0x66000000,0x6c000000
FIT_RETURNS={0x14007ed53:'initial',0x14007ef57:'temporal'}


class LargeHeapMachine(MatrixMachine):
    """Persistent frames allocate far more matrices than one fitting call."""
    def __init__(self,binary):
        super().__init__(binary)
        self.vm.mem_map(HEAP,HEAP_END-HEAP)
        self.cursor=HEAP

    def allocate(self,size):
        pointer=self.cursor
        self.cursor+=(size+63)&~63
        if size<0 or self.cursor>=HEAP_END:
            raise RuntimeError('Matrix allocation bound exceeded')
        return pointer


def low_float(value):
    return struct.unpack('<f',struct.pack('<I',value&0xffffffff))[0]


class OriginalTemporalCaller:
    def __init__(self,binary,model):
        m=self.m=LargeHeapMachine(binary);self.model=model
        parent=self.parent=m.allocate(0x168);caller=self.caller=m.allocate(0x200)
        m.vm.mem_write(caller+8,struct.pack('<Q',parent))
        m.vm.mem_write(parent+0x50,struct.pack('<ff',model.focal_factor,1));m.vm.mem_write(parent+0xc8,struct.pack('<I',57))
        self.pointers={}
        for offset,rows,cols,values in ((0,171,1,model.vertices),(8,171,1,model.vertices),
            (0x10,171,40,model.shape_basis),(0x18,171,23,model.animation_basis),
            (0x90,40,1,np.zeros(40)),(0x98,23,1,np.zeros(23)),
            (0xa0,171,1,np.zeros(171)),(0xa8,171,1,np.zeros(171)),(0xb8,114,1,np.zeros(114)),
            (0x110,6,1,model.state_sensitivity[:6]),(0x118,23,1,model.state_sensitivity[46:]),
            (0x120,40,1,model.state_sensitivity[6:46]),(0x140,6,1,model.rotation_limits),
            (0x148,6,1,model.translation_limits),(0x150,46,1,model.animation_ranges)):
            self.put(offset,rows,cols,values)
        self.put(0x30,23,1,model.animation_mask,True);self.put(0x38,40,1,model.shape_mask,True)
        self.counts=struct.unpack('<14i',m.vm.mem_read(0x14112f050,56))
        self.source,self.groups=self.fdp();mapped,mapgroups=self.fdp();self.fdp(caller+0x60)
        m.vm.mem_write(parent+0xd0,struct.pack('<Q',mapped))
        for i,(group,index) in enumerate(model.landmark_ids):
            r=mapgroups[group]+(index-1)*72;m.vm.mem_write(r+0xc,struct.pack('<I',1));m.vm.mem_write(r+0x18,struct.pack('<i',i))
        self.output=m.allocate(0x100)
        for offset in (0x88,0x90,0x98):m.vm.mem_write(self.output+offset,struct.pack('<Q',self.fdp()[0]))
        m.vm.mem_write(self.output+0xdc,struct.pack('<f',model.focal_factor))
        m.call(0x14007e270,caller)
        self.trace=[];self.updates=[]
        m.vm.hook_add(UC_HOOK_CODE,self.update,begin=0x1400df6b0,end=0x1400df6b0)
        for address in FIT_RETURNS:m.vm.hook_add(UC_HOOK_CODE,self.fit_return,begin=address,end=address)
        m.vm.hook_add(UC_HOOK_CODE,self.facing,begin=0x14007ef33,end=0x14007ef33)
        m.vm.hook_add(UC_HOOK_CODE,self.rectangle,begin=0x14007ef6c,end=0x14007ef6c)
        m.vm.hook_add(UC_HOOK_CODE,self.error,begin=0x14007ef8e,end=0x14007ef8e)

    def put(self,offset,rows,cols,values,integer=False):
        mat,data=self.m.matrix(rows,cols,np.asarray(values).flatten())
        if integer:self.m.vm.mem_write(data,np.asarray(values,dtype='<i4').tobytes())
        self.m.vm.mem_write(self.parent+offset,struct.pack('<Q',mat));self.pointers[offset]=data

    def fdp(self,header=None):
        m=self.m;header=m.allocate(0x1a0) if header is None else header;groups={}
        for group,count in enumerate(self.counts,2):
            groups[group]=m.allocate(count*72);m.vm.mem_write(header+0x120+group*8,struct.pack('<Q',groups[group]))
            for i in range(count):m.vm.mem_write(groups[group]+i*72+0x38,struct.pack('<Q',15))
        return header,groups

    def update(self,vm,address,size,_):
        fitter=vm.reg_read(UC_X86_REG_RCX);obj=struct.unpack('<Q',vm.mem_read(fitter,8))[0]
        self.updates.append(struct.unpack('<I',vm.mem_read(obj+0xc,4))[0])

    def fit_return(self,vm,address,size,_):
        modes=sorted(set(self.updates))
        self.trace.append({'mode':3 if FIT_RETURNS[address]=='initial' else 5,'valid':bool(vm.reg_read(UC_X86_REG_RAX)&0xff),
                           'updates':len(self.updates),'update_modes':modes})
        self.updates=[]

    def facing(self,vm,address,size,_):
        self.pending_facing=low_float(vm.reg_read(UC_X86_REG_XMM6))

    def rectangle(self,vm,address,size,_):
        self.pending_rectangle=struct.unpack('<4i',vm.mem_read(vm.reg_read(UC_X86_REG_RSP)+0x30,16))

    def error(self,vm,address,size,_):
        self.trace[-1].update({'facing':self.pending_facing,'error':low_float(vm.reg_read(UC_X86_REG_XMM0)),
                               'rectangle':self.pending_rectangle})

    def state(self):
        m=self.m;f=lambda address,count:np.asarray(m.floats(address,count),dtype=np.float32)
        flags=m.vm.mem_read(self.caller+0x38,0x22)
        return {'rigid_state':f(self.parent+0x58,6),'shape_coefficients':f(self.pointers[0x90],40),
            'animation_coefficients':f(self.pointers[0x98],23),'rotation':f(self.output+0x28,3),
            'translation':f(self.output+0x10,3),'initialised':bool(flags[0x21]),'frontal':bool(flags[1])}

    def lose_pose(self):
        """State 0x14007e440 leaves after a failed temporal fit whose recovery refit succeeded."""
        self.m.vm.mem_write(self.parent+0x58,np.array([0,FLOAT_PI,0,0,0,0],dtype=np.float32).tobytes())
        self.m.vm.mem_write(self.caller+0x59,b'\1')

    def process(self,points,width,height):
        for i,(group,index) in enumerate(self.model.landmark_ids):
            self.m.vm.mem_write(self.groups[group]+(index-1)*72,struct.pack('<fffIIf',*points[i],-1,1,1,1))
        self.trace=[];self.updates=[]
        self.m.call(0x14007e440,self.caller,self.source,width,height,self.output,1,
                    instruction_limit=600_000_000,timeout_us=600_000_000)
        return {'valid':bool(self.m.vm.reg_read(UC_X86_REG_RAX)&0xff),**self.state(),'trace':self.trace}


def landmarks(model,angles,translation,noise=0,seed=0):
    target=model.initial_state([*angles,*translation])
    _,projected=model_jacobian(model.vertices,target[:3],target[3:6],model.focal_factor,1,
        model.shape_basis,target[6:46],model.shape_ranges,model.animation_basis,target[46:],model.animation_ranges)
    points=(np.float32(1)-projected/np.array([640/480,1],dtype=np.float32))*np.float32(.5)
    if noise:points=points+np.random.default_rng(seed).normal(0,noise,points.shape).astype(np.float32)
    return points.astype(np.float32)


PI=np.pi
SEQUENCES={
    'tracking':(None,[
        ('first-frame',[.02,PI+.3,.01],[.01,0,.65],0),
        ('steady',[.02,PI+.32,.01],[.012,0,.65],0),
        ('steady-turn',[.02,PI+.18,.01],[.01,0,.65],0),
        ('frontal-shape-refit',[0,PI+.04,0],[.01,0,.65],0),
        ('error-rejection-reinitialise',[0,PI+.04,0],[.01,0,.65],.04),
        ('steady-wide-yaw',[0,PI+.45,0],[0,0,.65],0),
        ('steady-wider-yaw',[0,PI+.6,0],[0,0,.65],0),
        ('error-rejection-facing-reset',[0,PI+.62,0],[0,0,.65],.04),
        ('steady-after-reset',[0,PI+.3,0],[0,0,.65],0)]),
    'failure-recovery':([[-4,4],[-3,3],[.3,1]],[
        ('first-frame',[.02,PI+.3,.01],[.01,0,.65],0),
        ('out-of-bounds-failure',[.02,PI+.3,.01],[.01,0,1.6],0),
        ('reinitialised-after-failure',[.02,PI+.28,.01],[.01,0,.65],0),
        ('steady',[.02,PI+.26,.01],[.01,0,.66],0)]),
    'lost-pose-injected':(None,[
        ('first-frame',[.02,PI+.3,.01],[.01,0,.65],0),
        ('steady',[.02,PI+.28,.01],[.01,0,.65],0),
        ('after-injected-pose-loss',[.02,PI+.26,.01],[.01,0,.65],0,'lose-pose'),
        ('error-rejection-after-loss',[.02,PI+.26,.01],[.01,0,.65],.04),
        ('steady',[.02,PI+.24,.01],[.01,0,.65],0)])}
DECISION_KEYS=('valid','initialised','frontal')
TOLERANCES={'rigid_state':1e-5,'rotation':1e-5,'translation':1e-5,'shape_coefficients':1e-4,
    'animation_coefficients':1e-4,'error':1e-5,'facing':1e-3}
KEYS=('rigid_state','shape_coefficients','animation_coefficients','rotation','translation')


def compare(actual,expected,sensitivity):
    errors={key:float(np.max(np.abs(actual[key]-expected[key]))) for key in KEYS}
    measured=[x for x in actual['trace'] if 'error' in x];predicted=[x for x in expected['trace'] if 'error' in x]
    errors['error']=max([abs(a['error']-b['error']) for a,b in zip(measured,predicted)],default=0.0)
    errors['facing']=max([abs(a['facing']-b['facing']) for a,b in zip(measured,predicted)],default=0.0)
    decisions=(all(actual[key]==expected[key] for key in DECISION_KEYS)
        and [(x['mode'],x['valid']) for x in actual['trace']]==[(x['mode'],x['valid']) for x in expected['trace']]
        and len(measured)==len(predicted)
        and all(tuple(a['rectangle'])==tuple(b['rectangle']) for a,b in zip(measured,predicted)))
    fixed=all(errors[key]<=limit for key,limit in TOLERANCES.items())
    sensitive=all(errors[key]<=max(limit,2*sensitivity.get(key,0)) for key,limit in TOLERANCES.items())
    return decisions,fixed,sensitive,errors


def main():
    base=load_pose_model(ROOT/'models/tobii-et5/support')
    binary=(ROOT/'evidence/private/binaries/platform_runtime_IS5LEYETRACKER5_service.exe').read_bytes()
    report={'runtime_sha256':SHA256,'entry':'0x14007e440','last_argument':1,'sequences':{}}
    failures=[]
    for name,(translation_limits,frames) in SEQUENCES.items():
        model=base if translation_limits is None else replace(base,translation_limits=np.asarray(translation_limits,dtype=np.float32))
        original=OriginalTemporalCaller(binary,model);native=TemporalPoseFitter(model);variant=TemporalPoseFitter(model,accumulation='float32');rows=[]
        for seed,(label,angles,translation,noise,*event) in enumerate(frames):
            points=landmarks(model,angles,translation,noise,seed)
            if event==['lose-pose']:
                original.lose_pose()
                for fitter in (native,variant):
                    fitter.rigid_state=np.array([0,FLOAT_PI,0,0,0,0],dtype=np.float32);fitter.initialised=True
            actual=original.process(points,640,480);expected=native.process(points,640,480)
            alternative=variant.process(points,640,480)
            sensitivity={key:float(np.max(np.abs(expected[key]-alternative[key]))) for key in KEYS}
            decisions,fixed,numeric,errors=compare(actual,expected,sensitivity)
            row={'frame':label,'event':event[0] if event else None,'decisions_agree':decisions,
                'within_fixed_tolerance':fixed,'within_twice_arithmetic_sensitivity':numeric,
                'valid':actual['valid'],'initialised':actual['initialised'],'frontal':actual['frontal'],
                'original_fits':actual['trace'],
                'native_fits':[{k:(list(v) if isinstance(v,tuple) else v) for k,v in x.items()} for x in expected['trace']],
                'max_errors':errors,'native_float32_accumulation_change':sensitivity,
                'original_rotation':actual['rotation'].tolist(),'original_translation':actual['translation'].tolist()}
            print(name,label,decisions,fixed,numeric,actual['valid'],actual['initialised'],actual['frontal'],
                  [(x['mode'],x['valid'],x.get('error')) for x in actual['trace']],errors,sensitivity,flush=True)
            rows.append(row)
            if not (decisions and numeric):failures.append((name,label))
        report['sequences'][name]={'translation_limits':translation_limits,'frames':rows}
    report['tolerances']=TOLERANCES
    report['tolerance_basis']=('Branch decisions, flags, fit traces and rectangles must match exactly. A frame passes '
        'when every state difference is within the fixed tolerance or within twice the change caused by replacing '
        'float64 with float32 accumulation in the native update (native_float32_accumulation_change), which measures '
        'how sensitive that frame is to arithmetic order. within_fixed_tolerance records the stricter test alone.')
    report['limitations']=('Synthetic projected landmarks (plus Gaussian noise where noted) at 640x480; complete D1 records; '
        'single parent model built from the 57 selected vertices; no external pose hint, extra parent models, '
        'LBF eye refinement or neural inference. Allocator/free are the only stubs; heap enlarged for persistent frames.')
    report['passes']=not failures
    (ROOT/'evidence/private/pose-temporal-oracle.json').write_text(json.dumps(report,indent=2,default=float)+'\n')
    assert not failures,failures
if __name__=='__main__':main()
