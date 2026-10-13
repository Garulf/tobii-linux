#!/usr/bin/env python3
"""Bounded static RTTI lookup for the installed runtime's face-model classes."""
import hashlib
import json
from pathlib import Path
import struct
ROOT=Path(__file__).resolve().parents[1]
SHA='0f13b1f1cfa4ca9bb6f02580e7c3780eb455057ab210fcac186ac61fe82dc469'
b=(ROOT/'evidence/private/binaries/platform_runtime_IS5LEYETRACKER5_service.exe').read_bytes()
assert hashlib.sha256(b).hexdigest()==SHA
pe=struct.unpack_from('<I',b,0x3c)[0];coff=pe+4;opt=coff+20
sections=[]
for i in range(struct.unpack_from('<H',b,coff+2)[0]):
    off=opt+struct.unpack_from('<H',b,coff+16)[0]+40*i
    name=b[off:off+8].split(b'\0')[0].decode()
    size,rva,raw_size,raw=struct.unpack_from('<IIII',b,off+8)
    sections.append((name,rva,size,raw,raw_size))
def rva(raw):
    for _,v,_,p,n in sections:
        if p<=raw<p+n:return v+raw-p
    raise ValueError(raw)
def hits(pattern):
    start=0
    while (found:=b.find(pattern,start))>=0:
        yield found;start=found+1
base=0x140000000
classes=[]
for name in ('FaceModelNoVel','FaceModel','SystemModel'):
    name_offset=b.index(('.?AV'+name+'@VisionSDK@@').encode())
    type_rva=rva(name_offset-16)
    entries=[]
    for hit in hits(struct.pack('<I',type_rva)):
        start=hit-12
        sig,offset,cd,typeptr,hierarchy,selfptr=struct.unpack_from('<IIIIII',b,start)
        if sig!=1 or selfptr!=rva(start):continue
        col=base+rva(start)
        for ref in hits(struct.pack('<Q',col)):
            vtable=base+rva(ref+8);functions=[]
            for j in range(32):
                pointer=struct.unpack_from('<Q',b,ref+8+j*8)[0]
                if not base+0x1000<=pointer<base+0x43c800:break
                functions.append(hex(pointer))
            entries.append({'locator':hex(col),'object_offset':offset,'vtable':hex(vtable),'functions':functions})
    classes.append({'class':name,'type_rva':hex(type_rva),'entries':entries})
report={'runtime_sha256':SHA,'classes':classes,'limitations':'Static RTTI links; method semantics and live selection unresolved.'}
(ROOT/'evidence/private/pose-rtti.json').write_text(json.dumps(report,indent=2)+'\n')
print(json.dumps(report,indent=2))
