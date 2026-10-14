"""Command line and lifecycle of the browser demo: one process owns the tracker and serves the page."""
import argparse
import asyncio
import json
import logging
from pathlib import Path
import signal
import sys

from ..cli import DEFAULT_MODELS
from .pipeline import GAZE_SOURCES, DemoPipeline
from .server import DemoServer, EventHub

DEFAULT_PORT = 8765
ALBEDO_DISPLAY = (-296.62, 326.0, 111.82, 300.38, 326.0, 111.82, -296.62, 10.27, -3.1)
STATUS_INTERVAL_S = 1.0
SHUTDOWN_TIMEOUT_S = 5.0


def build_parser(prog='python3 -m tobii_linux.demo'):
    parser = argparse.ArgumentParser(
        prog=prog,
        description='Live browser view of the tracker: camera, head pose, gaze and status.')
    parser.add_argument('--host', default='127.0.0.1', help='Address to listen on (default 127.0.0.1)')
    parser.add_argument('--port', type=int, default=DEFAULT_PORT, help=f'HTTP port (default {DEFAULT_PORT})')
    parser.add_argument('--display', type=float, nargs=9, default=ALBEDO_DISPLAY,
                        metavar=('TLX', 'TLY', 'TLZ', 'TRX', 'TRY', 'TRZ', 'BLX', 'BLY', 'BLZ'),
                        help='Display area corners in millimetres: top-left, top-right, bottom-left')
    parser.add_argument('--models', type=Path, default=DEFAULT_MODELS)
    parser.add_argument('--channel', type=int, default=0, help='USB channel carrying TTP')
    parser.add_argument('--index', type=int, default=0, help='Device index from probe')
    parser.add_argument('--image-rate', type=float, default=15.0,
                        help='Camera images sent to the browser per second (default 15)')
    parser.add_argument('--gaze-source', choices=sorted(GAZE_SOURCES), default='device')
    parser.add_argument('--no-headpose', action='store_true', help='Skip face tracking and head pose')
    replay = parser.add_argument_group('replay', 'Play recorded data instead of opening the tracker')
    replay.add_argument('--replay-frames', type=Path, metavar='LOG',
                        help='JSON-lines log of tools/linux_session_probe.py --until frames')
    replay.add_argument('--replay-frames-dir', type=Path, metavar='DIR',
                        help="Directory holding that log's frames (default: frames/ next to the log)")
    replay.add_argument('--replay-gaze', type=Path, metavar='LOG',
                        help='JSON-lines log of tools/linux_stream_capture.py with gaze messages')
    return parser


def display_area(values):
    from ..head_pose_pipeline import DisplayArea
    return DisplayArea(*(tuple(values[i:i + 3]) for i in (0, 3, 6)))


def replaying(args):
    return args.replay_frames is not None or args.replay_gaze is not None


def make_source(args, pipeline):
    from .sources import LiveSource, ReplaySource
    if replaying(args):
        return ReplaySource(pipeline, args.replay_frames, args.replay_frames_dir, args.replay_gaze)
    return LiveSource(pipeline, args.index, args.channel)


async def publish_status(hub, pipeline):
    while True:
        hub.publish('status', pipeline.status(), retain=True)
        await asyncio.sleep(STATUS_INTERVAL_S)


def stop_on_signals(loop, stop):
    for signum in (signal.SIGINT, signal.SIGTERM):
        try:
            loop.add_signal_handler(signum, stop.set)
        except (NotImplementedError, RuntimeError):
            signal.signal(signum, lambda *_: loop.call_soon_threadsafe(stop.set))


async def run(args, ready=None, stop=None):
    """Serve until `stop` is set or SIGINT/SIGTERM arrives; `ready(url)` runs once the port is open."""
    loop = asyncio.get_running_loop()
    hub = EventHub()
    hub.attach(loop)
    pipeline = DemoPipeline(hub.publish_threadsafe, display_area(args.display), args.models,
                            image_rate=args.image_rate, gaze_source=GAZE_SOURCES[args.gaze_source](),
                            mode='replay' if replaying(args) else 'live', head_pose=not args.no_headpose)
    source = make_source(args, pipeline)
    server = await DemoServer(hub, args.host, args.port).start()
    url = f'http://{"localhost" if args.host in ("127.0.0.1", "::1") else args.host}:{server.port}/'
    stop = stop if stop is not None else asyncio.Event()
    if ready is None:
        stop_on_signals(loop, stop)
        print(f'Demo running at {url} (Ctrl+C to stop)', flush=True)
    else:
        ready(url)
    pipeline.start()
    source.start()
    status = asyncio.create_task(publish_status(hub, pipeline))
    try:
        await stop.wait()
    finally:
        status.cancel()
        source.stop()
        clean = await loop.run_in_executor(None, source.join, SHUTDOWN_TIMEOUT_S)
        await loop.run_in_executor(None, pipeline.stop, SHUTDOWN_TIMEOUT_S)
        await server.close()
        if not clean:
            print('The tracker thread did not stop in time; streams may still be subscribed', file=sys.stderr)


def main(argv=None, prog=None):
    args = (build_parser(prog) if prog else build_parser()).parse_args(argv)
    logging.basicConfig(level=logging.INFO, format='%(asctime)s %(message)s', datefmt='%H:%M:%S')
    try:
        asyncio.run(run(args))
    except (OSError, ValueError) as error:
        print(json.dumps({'error': str(error)}), file=sys.stderr)
        return 1
    print('Stopped', flush=True)
    return 0
