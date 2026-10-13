"""Command-line entry point: descriptor probe and the experimental head-pose session."""
import argparse
import json
import os
import sys
from pathlib import Path
from .usb import USBContext, USBError

DEFAULT_MODELS = Path(os.environ.get('TOBII_LINUX_MODELS', Path(__file__).resolve().parents[2]/'models/tobii-et5'))


def probe(_):
    with USBContext() as context:
        devices = context.probe()
    print(json.dumps({'status': 'descriptor probe only', 'devices': devices}, indent=2))
    return 0


def headpose(args):
    from .driver import TrackerSession, head_pose_samples
    from .head_pose_pipeline import DisplayArea
    from .usb_channel import USBChannel
    corners = [tuple(args.display[i:i + 3]) for i in (0, 3, 6)]
    area = DisplayArea(*corners)
    with USBContext() as context:
        devices = context.probe()
        if not devices:
            raise RuntimeError('No Eye Tracker 5 is visible to libusb')
        device = devices[args.index]
        with context.open_transport(bus=device['bus'], address=device['address']) as transport:
            with USBChannel(transport, args.channel) as channel:
                session = TrackerSession(channel)
                session.initialize()
                for sample in head_pose_samples(session, args.models, area):
                    print(json.dumps({'timestamp_us': sample.timestamp_us, 'valid': sample.valid,
                                      'position_mm': sample.position_mm,
                                      'rotation_rad': sample.rotation_rad}), flush=True)
    return 0


def main():
    parser = argparse.ArgumentParser(description='Native Eye Tracker 5 driver (experimental)')
    commands = parser.add_subparsers(dest='command', required=True)
    commands.add_parser('probe', help='Inspect standard USB descriptors without claiming interfaces')
    session = commands.add_parser('headpose', help='Start the tracker and print head-pose samples as JSON lines')
    session.add_argument('--display', type=float, nargs=9, required=True,
                         metavar=('TLX', 'TLY', 'TLZ', 'TRX', 'TRY', 'TRZ', 'BLX', 'BLY', 'BLZ'),
                         help='Display area corners in millimetres: top-left, top-right, bottom-left')
    session.add_argument('--models', type=Path, default=DEFAULT_MODELS)
    session.add_argument('--channel', type=int, default=0, help='USB channel carrying TTP')
    session.add_argument('--index', type=int, default=0, help='Device index from probe')
    args = parser.parse_args()
    try:
        return {'probe': probe, 'headpose': headpose}[args.command](args)
    except (USBError, OSError, RuntimeError, ValueError) as error:
        print(json.dumps({'error': str(error)}), file=sys.stderr)
        return 1


if __name__ == '__main__':
    raise SystemExit(main())
