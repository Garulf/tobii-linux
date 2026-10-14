"""Command line of the guided setup: serve the page, open the browser, run the flow, exit when it ends."""
import argparse
from contextlib import contextmanager
import json
import logging
from pathlib import Path
import sys
import tempfile
import time

from .. import display_setup as ds
from ..browser import open_page
from .flow import SetupFlow, SetupOptions, tracker_session
from .server import PageChannel, SetupServer

DEFAULT_PORT = 8777
FINAL_SCREEN_S = 3.0
EXIT_CODES = {'finished': 0, 'cancelled': 2, 'failed': 1}


def build_parser(prog='tobii-linux setup'):
    parser = argparse.ArgumentParser(
        prog=prog,
        description='Set up the Eye Tracker 5 on this display the way Tobii\'s setup does: line up the '
                    'display (when needed), check where you sit, calibrate, and save the result as a profile.')
    parser.add_argument('--display', choices=('auto', 'always', 'never'), default='auto',
                        help='Run the display setup: auto when the tracker has none yet (default), always, never')
    parser.add_argument('--no-calibration', dest='calibrate', action='store_false',
                        help='Stop after the display setup')
    parser.add_argument('--save-display', action='store_true',
                        help='Send a changed display setup to the tracker without asking on the page')
    parser.add_argument('--dry-run', action='store_true',
                        help='Read from the tracker but only report what would be written')
    parser.add_argument('--profile', help='Calibration profile to recalibrate (default: the current one; '
                                          'without one, the page asks for a name)')
    parser.add_argument('--profiles-directory', type=Path, help='Profile store (default $XDG_DATA_HOME/tobii-linux/profiles)')
    parser.add_argument('--backups-directory', type=Path,
                        help='Where the previous calibration and display setup are kept '
                             '(default $XDG_DATA_HOME/tobii-linux/backups)')
    size = parser.add_mutually_exclusive_group()
    size.add_argument('--edid', type=Path, help='EDID file whose size is used as the screen size hint')
    size.add_argument('--connector', help='DRM connector (e.g. card0-DP-1) whose EDID gives the size hint')
    parser.add_argument('--host', default='127.0.0.1', help='Address to listen on (default 127.0.0.1)')
    parser.add_argument('--port', type=int, default=DEFAULT_PORT, help=f'HTTP port (default {DEFAULT_PORT})')
    parser.add_argument('--no-browser', dest='browser', action='store_false',
                        help='Only print the address; open it yourself, fullscreen on the tracker\'s display')
    parser.add_argument('--index', type=int, default=0, help='Device index from probe')
    parser.add_argument('--channel', type=int, default=0, help='USB channel carrying TTP')
    parser.add_argument('--simulate', choices=('mapped', 'unmapped'),
                        help='Use a scripted tracker and user instead of the device, to try the pages')
    parser.add_argument('--log', type=Path, help='Write the steps as JSON lines to this file')
    return parser


def size_hint(args):
    if args.edid:
        return ds.edid_size_mm(args.edid.read_bytes())
    if args.connector:
        monitor = next((m for m in ds.drm_monitors() if m.connector == args.connector), None)
        if monitor is None:
            raise ValueError(f'No EDID for connector {args.connector}')
        return monitor.size_mm
    return None


def session_opener(args):
    if not args.simulate:
        return lambda: tracker_session(args.index, args.channel)

    @contextmanager
    def simulated():
        from ..driver import TrackerSession
        from ..simulated_tracker import SimulatedTracker
        yield TrackerSession(SimulatedTracker(mapped=args.simulate == 'mapped'))
    return simulated


def json_reporter(path):
    if path is None:
        return lambda kind, **values: None
    stream = open(path, 'a')

    def report(kind, **values):
        stream.write(json.dumps({'kind': kind, 'time': time.time(), **values}, default=repr) + '\n')
        stream.flush()
    return report


def options_from(args):
    """SetupOptions; a simulation keeps its profiles and backups in a temporary directory unless told otherwise."""
    profiles_directory, backups_directory = args.profiles_directory, args.backups_directory
    if args.simulate:
        scratch = Path(tempfile.mkdtemp(prefix='tobii-linux-setup-simulation-'))
        profiles_directory = profiles_directory or scratch/'profiles'
        backups_directory = backups_directory or scratch/'backups'
    return SetupOptions(display=args.display, calibrate=args.calibrate, save_display_without_asking=args.save_display,
                        dry_run=args.dry_run, profile=args.profile, profiles_directory=profiles_directory,
                        backups_directory=backups_directory, size_hint_mm=size_hint(args))


def wait_for_final_screen(channel, seconds=FINAL_SCREEN_S):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline and channel.acked < channel.screen['seq']:
        time.sleep(0.05)


def main(argv=None, prog='tobii-linux setup'):
    args = build_parser(prog).parse_args(argv)
    logging.basicConfig(level=logging.WARNING, format='%(message)s')
    try:
        options = options_from(args)
    except (OSError, ValueError) as error:
        print(f'error: {error}', file=sys.stderr)
        return 1
    channel = PageChannel()
    try:
        server = SetupServer(channel, args.host, args.port).start()
    except OSError as error:
        print(f'error: cannot listen on {args.host}:{args.port}: {error}', file=sys.stderr)
        return 1
    try:
        print(f'Setup page: {server.url}', flush=True)
        if args.browser and open_page(server.url) is None:
            print('No browser launcher found; open the address above yourself.', flush=True)
        flow = SetupFlow(channel, session_opener(args), options, report=json_reporter(args.log))
        outcome = flow.run()
        wait_for_final_screen(channel)
    finally:
        server.close()
    print(f'{outcome.status}: {outcome.message}', flush=True)
    for path in outcome.backups:
        print(f'backup: {path}', flush=True)
    return EXIT_CODES[outcome.status]


if __name__ == '__main__':
    raise SystemExit(main())
