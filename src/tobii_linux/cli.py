"""Command-line entry point: descriptor probe and the experimental head-pose session."""
import argparse
import json
import os
import sys
from pathlib import Path
from .prp_service import DEFAULT_NAME, DEFAULT_PORT, WINE_CLOCKS
from .usb import USBContext, USBError

REPO_MODELS = Path(__file__).resolve().parents[2]/'models/tobii-et5'
DEFAULT_MODELS = Path(os.environ.get('TOBII_LINUX_MODELS') or (REPO_MODELS if REPO_MODELS.is_dir() else default_models_dir()))


def probe(_):
    with USBContext() as context:
        devices = context.probe()
    print(json.dumps({'status': 'descriptor probe only', 'devices': devices}, indent=2))
    return 0


def display_area(args):
    from .head_pose_pipeline import DisplayArea
    return DisplayArea(*(tuple(args.display[i:i + 3]) for i in (0, 3, 6)))


def tracker_samples(args, host_clock=None):
    """Head-pose samples from the selected tracker until it stops sending frames."""
    from .driver import head_pose_samples, usb_session
    with usb_session(args.index, args.channel) as session:
        yield from head_pose_samples(session, args.models, display_area(args), host_clock=host_clock)


def headpose(args):
    for sample in tracker_samples(args):
        print(json.dumps({'timestamp_us': sample.timestamp_us, 'valid': sample.valid,
                          'position_mm': sample.position_mm,
                          'rotation_rad': sample.rotation_rad}), flush=True)
    return 0


SHUTDOWN_TIMEOUT_S = 8.0


def service_link(service, loop, processor, clock, open_session):
    """TrackerLink feeding `service` (a PrpService on `loop`) from sessions of `open_session`.

The service accepts clients from the moment a session streams and drops them
whenever the link is not streaming.
"""
    import logging
    from .streams import ServiceFeed, SuspendWatch, serve_tracker
    from .supervisor import TrackerLink

    def soon(callback, *arguments):
        loop.call_soon_threadsafe(callback, *arguments)

    gaze_hub = service.gaze_hub
    feed = ServiceFeed(sample=lambda sample: soon(service.hub.publish, sample),
                       gaze=None if gaze_hub is None else (lambda frame: soon(gaze_hub.publish, frame)),
                       presence=lambda time_us, value: soon(service.publish_presence, time_us, value),
                       started=lambda calibration_id: soon(service.attach_tracker, calibration_id))

    def on_state(state, detail):
        soon(service.detach_tracker)
        logging.info('tracker %s: %s', state, detail)

    return TrackerLink(open_session,
                       lambda session, stopping: serve_tracker(session, processor, clock, feed, stopping,
                                                               SuspendWatch()),
                       on_state)


def serve(args):
    """Serve games until SIGINT or SIGTERM, starting the tracker again whenever it is lost.

While no tracker session runs, game clients are disconnected and refused, as
the Windows runtime shuts its server down when the device goes away; the
service process and the bridge's control connection stay up.
"""
    import asyncio
    import logging
    import signal
    import threading
    from .driver import HeadPoseProcessor, usb_session
    from .prp_service import GazeHub, HeadPoseHub, PrpService, wine_clock
    logging.basicConfig(level=logging.INFO, format='%(asctime)s %(message)s')
    clock = wine_clock(args.wine_clock)
    processor = HeadPoseProcessor(args.models, display_area(args))

    async def run():
        loop = asyncio.get_running_loop()
        stop = asyncio.Event()
        for signum in (signal.SIGINT, signal.SIGTERM):
            loop.add_signal_handler(signum, stop.set)
        hub = HeadPoseHub()
        gaze_hub = None if args.no_gaze else GazeHub()
        service = PrpService(hub, args.name, clock=clock, display_area=display_area(args),
                             display_id=args.display_id, gaze_hub=gaze_hub)
        server = await service.serve(args.host, args.port)
        logging.info('PRP service on %s:%d, pipes %s', args.host, args.port, service.pipe_names)

        link = service_link(service, loop, processor, clock,
                            lambda: usb_session(args.index, args.channel))
        worker = threading.Thread(target=link.run, name='tracker-link')
        worker.start()
        try:
            await stop.wait()
            logging.info('stopping')
        finally:
            link.stop()
            await loop.run_in_executor(None, worker.join, SHUTDOWN_TIMEOUT_S)
            await service.close(server)
        if worker.is_alive():
            raise RuntimeError('The tracker thread did not stop in time')
        logging.info('stopped: %d session(s), %d lost', link.sessions, link.failures)

    asyncio.run(run())
    return 0


def main():
    if sys.argv[1:2] == ['demo']:
        from .demo.app import main as demo
        return demo(sys.argv[2:], prog=f'{Path(sys.argv[0]).name} demo')
    parser = argparse.ArgumentParser(description='Native Eye Tracker 5 driver (experimental)')
    commands = parser.add_subparsers(dest='command', required=True)
    commands.add_parser('probe', help='Inspect standard USB descriptors without claiming interfaces')
    commands.add_parser('demo', help='Serve the live browser demo; see demo --help for its options')
    headpose_parser = commands.add_parser(
        'headpose', help='Start the tracker and print head-pose samples as JSON lines')
    serve_parser = commands.add_parser(
        'serve', help='Serve tracker head pose and gaze to Windows games through the Wine pipe bridge')
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
    serve_parser.add_argument('--display-id',
                              help="Monitor device instance id the game's window tracking matches "
                                   "(the part before '##'); needed only when the game tracks a window")
    serve_parser.add_argument('--wine-clock', choices=WINE_CLOCKS, default='auto',
                              help="Clock behind the game's Wine QueryPerformanceCounter: auto follows "
                                   'what the bridge reports (default), boottime is upstream Wine 10.17 '
                                   'and later, raw older Wine and every Proton')
    serve_parser.add_argument('--no-gaze', action='store_true',
                              help='Serve head pose only, without subscribing the gaze stream')
    args = parser.parse_args()
    try:
        return {'probe': probe, 'headpose': headpose, 'serve': serve}[args.command](args)
    except (USBError, OSError, RuntimeError, ValueError) as error:
        print(json.dumps({'error': str(error)}), file=sys.stderr)
        return 1


if __name__ == '__main__':
    raise SystemExit(main())
