#!/usr/bin/env python3
"""Run the native supplied-face-box component on a local raw grayscale frame."""
import argparse
import json
from pathlib import Path
from tobii_linux.headpose import NativeD1Pose


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('frame',type=Path,help='Packed width*height grayscale bytes, one byte per pixel')
    parser.add_argument('--width',type=int,required=True)
    parser.add_argument('--height',type=int,required=True)
    parser.add_argument('--face-box',type=int,nargs=4,required=True,metavar=('X','Y','WIDTH','HEIGHT'))
    parser.add_argument('--quality-mode',type=int,choices=(0,1,2),default=0)
    parser.add_argument('--roll-degrees',type=float,default=0)
    parser.add_argument('--models',type=Path,default=Path(__file__).resolve().parents[1]/'models/tobii-et5')
    args=parser.parse_args()
    if not 0<args.width<=32768 or not 0<args.height<=32768:
        parser.error('Frame dimensions must be in 1..32768')
    if args.frame.stat().st_size!=args.width*args.height:
        parser.error('Raw frame length must equal width*height')
    component=NativeD1Pose(args.models)
    result=component.predict_frame(args.frame.read_bytes(),args.width,args.height,tuple(args.face_box),
                                   args.quality_mode,args.roll_degrees)
    report={'quality_score':result.quality_score,'quality_accepted':result.quality_accepted,
            'coordinate_space':'internal fitting model; not the public Tobii/Star Citizen API',
            'fitting':None}
    if result.fitting_result is not None:
        report['fitting']={key:value.tolist() if hasattr(value,'tolist') else value
                           for key,value in result.fitting_result.items()}
    print(json.dumps(report,indent=2,allow_nan=False))
if __name__=='__main__':main()
