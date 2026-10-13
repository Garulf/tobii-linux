"""Command-line entry point: descriptor probe and the experimental head-pose session."""
import argparse
import json
import os
import sys
from pathlib import Path
from .prp_service import DEFAULT_NAME, DEFAULT_PORT, WINE_CLOCKS
from .usb import USBContext, USBError

DEFAULT_MODELS = Path(os.environ.get('TOBII_LINUX_MODELS', Path(__file__).resolve().parents[2]/'models/tobii-et5'))


def probe(_):
    with USBContext() as context:
        devices = context.probe()
    print(json.dumps({'status': 'descriptor probe only', 'devices': devices}, indent=2))
    return 0


def tracker_samples(args, host_clock=None):
    """Head-pose samples from the first matching tracker until it stops sending frames."""
    from .driver import TrackerSession, head_pose_samples
    from .head_pose_pipeline import DisplayArea
    from .usb_channel import USBChannel
    area = DisplayArea(*(tuple(args.display[i:i + 3]) for i in (0, 3, 6)))
    with USBContext() as context:
        devices = context.probe()
        if not devices:
            raise RuntimeError('No Eye Tracker 5 is visible to libusb')
        device = devices[args.index]
        with context.open_transport(bus=device['bus'], address=device['address']) as transport:
            with USBChannel(transport, args.channel) as channel:
                session = TrackerSession(channel)
                session.initialize()
                yield from head_pose_samples(session, args.models, area, host_clock=host_clock)


def headpose(args):
    for sample in tracker_samples(args):
        print(json.dumps({'timestamp_us': sample.timestamp_us, 'valid': sample.valid,
                          'position_mm': sample.position_mm,
                          'rotation_rad': sample.rotation_rad}), flush=True)
    return 0


def serve(args):
    import asyncio
    import logging
    from .prp_service import HeadPoseHub, PrpService, wine_clock
    logging.basicConfig(level=logging.INFO, format='%(asctime)s %(message)s')
    clock = wine_clock(args.wine_clock)

    async def run():
        hub = HeadPoseHub()
        service = PrpService(hub, args.name, clock=clock)
        server = await service.serve(args.host, args.port)
        logging.info('PRP service on %s:%d, pipes %s', args.host, args.port, service.pipe_names)
        async with server:
            await hub.feed_iterator(tracker_samples(args, host_clock=clock))
        raise RuntimeError('The tracker stopped sending frames')

    asyncio.run(run())
    return 0


def main():
    parser = argparse.ArgumentParser(description='Native Eye Tracker 5 driver (experimental)')
    commands = parser.add_subparsers(dest='command', required=True)
    commands.add_parser('probe', help='Inspect standard USB descriptors without claiming interfaces')
    headpose_parser = commands.add_parser(
        'headpose', help='Start the tracker and print head-pose samples as JSON lines')
    serve_parser = commands.add_parser(
        'serve', help='Serve tracker head pose to Windows games through the Wine pipe bridge')
    for session in (headpose_parser, serve_parser):
        session.add_argument('--display', type=float, nargs=9, required=True,
                             metavar=('TLX', 'TLY', 'TLZ', 'TRX', 'TRY', 'TRZ', 'BLX', 'BLY', 'BLZ'),
                             help='Display area corners in millimetres: top-left, top-right, bottom-left')
        session.add_argument('--models', type=Path, default=DEFAULT_MODELS)
        session.add_argument('--channel', type=int, default=0, help='USB channel carrying TTP')
        session.add_argument('--index', type=int, default=0, help='Device index from probe')
    serve_parser.add_argument('--host', default='127.0.0.1')
    serve_parser.add_argument('--port', type=int, default=DEFAULT_PORT, help='TCP port the bridge connects to')
    serve_parser.add_argument('--name', default=DEFAULT_NAME, help='Device name in tobii-prp://<name>')
    serve_parser.add_argument('--wine-clock', choices=WINE_CLOCKS, default='boottime',
                              help="Clock behind the game's Wine QueryPerformanceCounter: "
                                   'boottime for Wine 10.17 and later, raw for older')
    args = parser.parse_args()
    try:
        return {'probe': probe, 'headpose': headpose, 'serve': serve}[args.command](args)
    except (USBError, OSError, RuntimeError, ValueError) as error:
        print(json.dumps({'error': str(error)}), file=sys.stderr)
        return 1


if __name__ == '__main__':
    raise SystemExit(main())
