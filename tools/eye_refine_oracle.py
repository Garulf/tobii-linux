#!/usr/bin/env python3
"""Check the 0x14007d270 eye refinement control flow and the 0x140011900 mirror map."""
import json
from pathlib import Path
import struct
import numpy as np
from unicorn import UC_HOOK_CODE
from unicorn.x86_const import (UC_X86_REG_RAX,UC_X86_REG_RCX,UC_X86_REG_RDX,UC_X86_REG_R8,UC_X86_REG_R9,
    UC_X86_REG_RIP,UC_X86_REG_RSP,UC_X86_REG_XMM3)
from vsmat_oracle import MatrixMachine
from ttp_encoder_oracle import SHA256
from tobii_linux.eye_refine import MIRRORED_INDEX,refine_eye_landmarks
from tobii_linux.lbf import read_lbf
ROOT=Path(__file__).resolve().parents[1]
WIDTH,HEIGHT,STRIDE=240,200,256


def signed(value):
    return struct.unpack('<i',struct.pack('<I',value&0xffffffff))[0]


def check_mirror_map(m):
    counts=struct.unpack('<14i',m.vm.mem_read(0x14112f050,56));mismatches=[]
    for group,count in enumerate(counts,2):
        if len(MIRRORED_INDEX[group])!=count:mismatches.append((group,'count'))
        for index in range(1,count+1):
            m.call(0x140011900,group,index)
            if signed(m.vm.reg_read(UC_X86_REG_RAX))!=MIRRORED_INDEX[group][index-1]:mismatches.append((group,index))
    return sum(counts),mismatches


def shift(xs,ys):
    """Deterministic stand-in for the four LBF stages, applied identically on both sides."""
    return ([np.float32(x+np.float32(2+.5*i)) for i,x in enumerate(xs)],
            [np.float32(y+np.float32(1+i)) for i,y in enumerate(ys)])


class OriginalRefinement:
    def __init__(self,binary,point_ids):
        m=self.m=MatrixMachine(binary);self.point_ids=point_ids;n=len(point_ids)
        table=m.allocate(0x48)
        groups=m.allocate(4*n);indices=m.allocate(4*n)
        m.vm.mem_write(groups,struct.pack(f'<{n}i',*[g for g,_ in point_ids]))
        m.vm.mem_write(indices,struct.pack(f'<{n}i',*[i for _,i in point_ids]))
        m.vm.mem_write(table+0x18,struct.pack('<QQQ',groups,groups+4*n,groups+4*n))
        m.vm.mem_write(table+0x30,struct.pack('<QQQ',indices,indices+4*n,indices+4*n))
        self.models={}
        for name in ('fa','fc'):
            model=self.models[name]=m.allocate(0x48)
            m.vm.mem_write(model+0x18,struct.pack('<Q',table));m.vm.mem_write(model+0x40,struct.pack('<i',n))
        self.tracker=m.allocate(0x28)
        m.vm.mem_write(self.tracker+4,struct.pack('<iQQ',n,self.models['fa'],self.models['fc']))
        self.pixels=m.allocate(STRIDE*HEIGHT)
        self.image=m.allocate(0x70);self.roi=m.allocate(0x14)
        m.vm.mem_write(self.image+0x28,struct.pack('<ii',WIDTH,HEIGHT))
        m.vm.mem_write(self.image+0x58,struct.pack('<Qi',self.pixels,STRIDE))
        self.counts=struct.unpack('<14i',m.vm.mem_read(0x14112f050,56))
        self.records=m.allocate(0x1a0);self.groups={}
        for group,count in enumerate(self.counts,2):
            self.groups[group]=m.allocate(count*72)
            m.vm.mem_write(self.records+0x120+group*8,struct.pack('<Q',self.groups[group]))
            for i in range(count):m.vm.mem_write(self.groups[group]+i*72+0x38,struct.pack('<Q',15))
        m.vm.hook_add(UC_HOOK_CODE,self.refinement,begin=0x1400acb80,end=0x1400acb80)

    def refinement(self,vm,address,size,_):
        n=len(self.point_ids);stack=vm.reg_read(UC_X86_REG_RSP)
        model,ys_pointer,xs_pointer=(vm.reg_read(r) for r in (UC_X86_REG_RCX,UC_X86_REG_RDX,UC_X86_REG_R8))
        pixels,rows,columns,stride,flag=struct.unpack('<QQQQQ',vm.mem_read(stack+0x28,40))
        xs=list(np.frombuffer(bytes(vm.mem_read(xs_pointer,4*n)),dtype='<f4'))
        ys=list(np.frombuffer(bytes(vm.mem_read(ys_pointer,4*n)),dtype='<f4'))
        name=[k for k,v in self.models.items() if v==model][0]
        self.call={'model':name,'mirrored':bool(flag&0xff),'scale':struct.unpack('<f',struct.pack('<I',vm.reg_read(UC_X86_REG_XMM3)&0xffffffff))[0],
            'xs':[float(x) for x in xs],'ys':[float(y) for y in ys],'pixel_offset':pixels-self.pixels,
            'rows':rows&0xffffffff,'columns':columns&0xffffffff,'stride':stride&0xffffffff}
        xs,ys=shift(xs,ys)
        vm.mem_write(xs_pointer,np.asarray(xs,dtype='<f4').tobytes());vm.mem_write(ys_pointer,np.asarray(ys,dtype='<f4').tobytes())
        ret=struct.unpack('<Q',vm.mem_read(stack,8))[0]
        vm.reg_write(UC_X86_REG_RSP,stack+8);vm.reg_write(UC_X86_REG_RIP,ret)

    def run(self,records,scale,yaw,roi):
        m=self.m
        for (group,index),(x,y) in records.items():
            m.vm.mem_write(self.groups[group]+(index-1)*72,struct.pack('<fffIIf',x,y,-1,1,1,.5))
        before=bytes(m.vm.mem_read(self.groups[10],self.counts[8]*72))
        if roi is None:m.vm.mem_write(self.image+0x30,struct.pack('<Q',0))
        else:
            m.vm.mem_write(self.roi,struct.pack('<5i',0,*roi));m.vm.mem_write(self.image+0x30,struct.pack('<Q',self.roi))
        self.call=None
        m.vm.reg_write(UC_X86_REG_XMM3,int.from_bytes(struct.pack('<f',scale),'little'))
        m.call(0x14007d270,self.tracker,self.image,self.records,0,int.from_bytes(struct.pack('<f',yaw),'little'))
        valid=bool(m.vm.reg_read(UC_X86_REG_RAX)&0xff)
        after=bytes(m.vm.mem_read(self.groups[10],self.counts[8]*72));written={}
        for i in range(self.counts[8]):
            if before[i*72:i*72+24]!=after[i*72:i*72+24]:
                written[(10,i+1)]=struct.unpack('<fffIIf',after[i*72:i*72+24])
        return valid,written,self.call


def main():
    binary=(ROOT/'evidence/private/binaries/platform_runtime_IS5LEYETRACKER5_service.exe').read_bytes()
    mirror_count,mirror_mismatches=check_mirror_map(MatrixMachine(binary))
    installed=[tuple(p['id']) for p in read_lbf(ROOT/'models/tobii-et5/support/bdtsdata/NN/fa.lbf')['points']]
    synthetic=installed[:4]+[(10,1),(10,2),(10,7),(10,8)]
    rng=np.random.default_rng(5)
    records={(g,i):tuple(np.float32(rng.uniform(.2,.8,2))) for g,c in enumerate(struct.unpack('<14i',binary_counts(binary)),2)
             for i in range(1,c+1)}
    cases=[];failures=[]
    for table_name,point_ids in (('installed-fa',installed),('synthetic-group-10',synthetic)):
        original=OriginalRefinement(binary,point_ids)
        for label,yaw,roi in (('frontal-fa',0.0,None),('mirrored-fc',20.0,None),('unmirrored-fc',-20.0,None),
                ('threshold-fc',15.0,None),('nan-fa',float('nan'),None),('roi-inside',20.0,(4,6,200,180)),
                ('roi-outside',20.0,(50,6,200,180)),('roi-negative',0.0,(-1,0,100,100))):
            valid,written,call=original.run(records,87.5,yaw,roi)
            seen={}
            def refine(name,xs,ys,scale,mirrored,region):
                seen.update({'model':name,'mirrored':mirrored,'scale':float(scale),'xs':[float(x) for x in xs],'ys':[float(y) for y in ys]})
                return shift(xs,ys)
            native_valid,native_written=refine_eye_landmarks(point_ids,records,WIDTH,HEIGHT,np.float32(87.5),yaw,refine,roi)
            native_written={k:tuple(float(x) for x in v) for k,v in native_written.items()}
            original_written={k:tuple(float(x) for x in v) for k,v in written.items()}
            agree=(valid==native_valid and original_written==native_written
                and (call is None)==(not seen)
                and (call is None or all(call[k]==seen[k] for k in ('model','mirrored','scale','xs','ys'))))
            if call is not None and roi is not None:
                agree&=call['pixel_offset']==roi[1]*STRIDE+roi[0] and (call['columns'],call['rows'])==roi[2:]
            elif call is not None:
                agree&=call['pixel_offset']==0 and (call['columns'],call['rows'],call['stride'])==(WIDTH,HEIGHT,STRIDE)
            print(table_name,label,agree,valid,call and (call['model'],call['mirrored']),sorted(original_written),flush=True)
            cases.append({'point_table':table_name,'case':label,'yaw_degrees':None if yaw!=yaw else yaw,'roi':roi,
                'valid':valid,'refinement_call':None if call is None else {k:call[k] for k in ('model','mirrored','scale','pixel_offset','rows','columns','stride')},
                'written_group_10':{f'{g}.{i}':v for (g,i),v in original_written.items()},'agrees':agree})
            if not agree:failures.append((table_name,label))
    report={'runtime_sha256':SHA256,'entries':['0x14007d270','0x1400ac9c0','0x140011900'],
        'mirror_map':{'fdp_ids_checked':mirror_count,'mismatches':mirror_mismatches},
        'cases':cases,'passes':not failures and not mirror_mismatches,
        'limitations':('0x1400acb80 (the four LBF stages, checked separately by lbf_predictor_oracle.py and '
            'lbf_gather_oracle.py) is replaced by a deterministic coordinate shift applied identically on both sides; '
            'the original 0x14007d270 and 0x1400ac9c0 run unchanged with allocator/free stubs. Model objects carry only '
            'the point table and count. Synthetic records and image; no tracker frame loop.')}
    (ROOT/'evidence/private/eye-refine-oracle.json').write_text(json.dumps(report,indent=2,default=float)+'\n')
    assert report['passes'],(failures,mirror_mismatches)


def binary_counts(binary):
    m=MatrixMachine(binary)
    return bytes(m.vm.mem_read(0x14112f050,56))
if __name__=='__main__':main()
