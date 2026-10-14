#!/usr/bin/env python3
"""Tell the Eye Tracker 5 where the monitor is: the display setup step of Tobii's configuration app.

Modes (JSON lines on stdout):

  show                       read the device's display area (1430) and mounting geometry (2110)
                             and the monitor size and tracker offset they imply; reads only
  compute SIZE [OFFSET]      print the display area for a monitor; offline, uses the Eye Tracker 5
                             mounting unless --mounting is given; --save FILE keeps it
  apply SIZE [OFFSET]        compute with the device's mounting and send it (1440); --dry-run
                             reports the request instead; --from-device rewrites what 1430 returns
                             (the runtime's own start-up write)
  apply --file FILE          send a saved display area
  args FILE                  print a saved area as the nine numbers `tobii-linux serve --display` takes
  setup [--apply]            the on-screen step of Tobii's display setup: serves a fullscreen page
                             (default http://localhost:8776/) where the user lines up two lines
                             with the marks on the tracker; reads 1430 and 2110, saves the result
                             (--output, .setpm and .json) and sends 1440 only with --apply;
                             --then-calibrate continues with tools/gaze_calibrate.py calibrate

SIZE is one of --edid FILE, --connector NAME (EDID from /sys/class/drm),
--size W H (mm) or --diagonal INCHES [--aspect W:H]. OFFSET is --offset MM
(tracker centre relative to the display centre, positive to the right) or
--lines LEFT RIGHT (where the tracker's ends sit on screen, 0 to 1 from the
left, as the app's moving lines). --portrait swaps width and height.

Saved files use the runtime's screenplane.setpm layout, so a copy of the
Windows file works too. See docs/display-setup.md.
"""
import argparse
from contextlib import ExitStack
import hashlib
import json
from pathlib import Path
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1]/'src'))
from tobii_linux import calibration as cal  # noqa: E402
from tobii_linux import display_setup as ds  # noqa: E402
from tobii_linux import gaze_device as gd  # noqa: E402
from tobii_linux.ttp_commands import Command  # noqa: E402

DISPLAY_READS = cal.READ_COMMANDS | {Command.GET_GEOMETRY_MOUNTING}


def emit(kind, **values):
    print(json.dumps({'kind': kind, 'time': time.time(), **values}, default=repr), flush=True)


def area_record(area):
    return {'top_left': area.top_left, 'top_right': area.top_right, 'bottom_left': area.bottom_left,
            'size_mm': [round(area.width_mm, 3), round(area.height_mm, 3)], 'display_args': list(area.floats)}


def mounting_record(mounting):
    return {'guides': mounting.guides, 'width_mm': mounting.width_mm, 'angle_deg': mounting.angle_deg,
            'external_offset_mm': mounting.external_offset_mm, 'internal_offset_mm': mounting.internal_offset_mm}


def setup_record(setup):
    return {'size_mm': [round(v, 4) for v in setup.size_mm], 'tracker_offset_mm': round(setup.tracker_offset_mm, 5),
            'extra_angle_deg': round(setup.extra_angle_deg, 6)}


def open_session(args, stack):
    """The read-only tools' startup: upgrade, unlock_ppu, realm 0x2712 with hardware info."""
    from tobii_linux import driver
    from tobii_linux.usb import USBContext
    from tobii_linux.usb_channel import USBChannel
    context = stack.enter_context(USBContext())
    device = context.probe()[args.index]
    transport = stack.enter_context(context.open_transport(bus=device['bus'], address=device['address']))
    channel = stack.enter_context(USBChannel(transport, args.channel))
    session = driver.TrackerSession(channel)
    session.initialize()
    session.camera_calibration()
    return session


def monitor_size(args):
    if args.edid:
        edid = Path(args.edid).read_bytes()
        size, source = ds.edid_size_mm(edid), {'edid': args.edid, 'resolution_px': ds.edid_native_resolution(edid)}
    elif args.connector:
        monitor = next((m for m in ds.drm_monitors() if m.connector == args.connector), None)
        if monitor is None:
            raise ValueError(f'No EDID for connector {args.connector}')
        size, source = monitor.size_mm, {'connector': monitor.connector, 'resolution_px': monitor.resolution_px}
    elif args.size:
        size, source = tuple(args.size), {'size': 'given'}
    elif args.diagonal:
        aspect = tuple(float(v) for v in args.aspect.split(':'))
        size, source = ds.size_from_diagonal(args.diagonal, aspect), {'diagonal_in': args.diagonal, 'aspect': aspect}
    else:
        raise ValueError('Give the monitor size: --edid, --connector, --size or --diagonal')
    if 'resolution_px' in source:
        source['size_hint_plausible'] = ds.size_hint_is_plausible(size, source['resolution_px'])
    return (size[1], size[0]) if args.portrait else size, source


def tracker_offset(args, size, mounting):
    if args.lines:
        size, offset = ds.size_from_tracker_lines(*args.lines, mounting.width_mm, size[0] / size[1])
        return size, offset
    return size, args.offset


def compose(args, mounting):
    size, source = monitor_size(args)
    size, offset = tracker_offset(args, size, mounting)
    area = ds.compose_display_area(size, mounting, offset)
    emit('display_setup', monitor=source, size_mm=list(size), tracker_offset_mm=offset,
         mounting=mounting_record(mounting), area=area_record(area))
    return area


def show(session, args):
    area, mounting = ds.read_display_area(session), ds.read_mounting(session)
    emit('device', area=area_record(area), mounting=mounting_record(mounting),
         setup=setup_record(ds.decompose_display_area(area, mounting)))
    for monitor in ds.drm_monitors():
        emit('monitor', connector=monitor.connector, status=monitor.status, size_mm=monitor.size_mm,
             resolution_px=monitor.resolution_px)
    return 0


def compute(args):
    mounting = ds.MountingGeometry(int(args.mounting[0]), *args.mounting[1:3], tuple(args.mounting[3:6]),
                                   tuple(args.mounting[6:9])) if args.mounting else ds.ET5_MOUNTING
    area = compose(args, mounting)
    if args.save:
        emit('saved', path=str(ds.save_display_area(args.save, area, overwrite=args.overwrite)))
    return 0


def apply(session, args):
    before = ds.read_display_area(session)
    if args.from_device:
        wanted = before
    elif args.file:
        wanted = ds.load_display_area(args.file).as_float32()
    else:
        wanted = compose(args, ds.read_mounting(session))
    request = gd.set_display_area(0, *wanted.corners).encode()
    identical = wanted == before
    emit('apply', before=area_record(before), wanted=area_record(wanted), identical_to_device=identical,
         almost_equal=wanted.almost_equals(before), request_bytes=len(request),
         request_sha256=hashlib.sha256(request).hexdigest(), dry_run=args.dry_run)
    if args.dry_run:
        device = cal.DryRunSession(session, report=lambda r: emit('would_send', **r), allowed=DISPLAY_READS)
        ds.apply_display_area(device, wanted)
        return 0
    after = ds.apply_display_area(session, wanted)
    emit('applied', after=area_record(after), device_holds_wanted=after == wanted)
    return 0 if after == wanted else 3


def size_hint(args):
    if args.edid:
        return ds.edid_size_mm(Path(args.edid).read_bytes())
    if args.connector:
        monitor = next((m for m in ds.drm_monitors() if m.connector == args.connector), None)
        if monitor is None:
            raise ValueError(f'No EDID for connector {args.connector}')
        return monitor.size_mm
    return None


def save_setup(output, area, overwrite):
    """The area in screenplane.setpm layout and as JSON holding the `--display` arguments."""
    setpm = ds.save_display_area(Path(output).with_suffix('.setpm'), area, overwrite=overwrite)
    record = {**area_record(area), 'display_option': '--display ' + ' '.join(repr(v) for v in area.floats)}
    json_path = setpm.with_suffix('.json')
    if json_path.exists() and not overwrite:
        raise FileExistsError(f'{json_path} exists')
    json_path.write_text(json.dumps(record, indent=2) + '\n')
    return setpm, json_path


def evaluate_lines(device_area, mounting):
    """The page's confirm handler: geometry of the confirmed lines, as the wizard computes it on Done."""
    def evaluate(left, right, viewport, aspect):
        size, offset, area = ds.area_from_lines(left, right, aspect, mounting)
        return {'left': left, 'right': right, 'size_mm': list(size), 'tracker_offset_mm': offset,
                'area': area_record(area), 'identical_to_device': area == device_area, 'viewport': viewport}
    return evaluate


def setup(session, args):
    """Read the device mapping, run the page, then save and (with --apply) send what the user confirmed."""
    from tobii_linux.display_setup_page import DisplaySetupPage
    device_area, mounting = ds.read_display_area(session), ds.read_mounting(session)
    hint = size_hint(args)
    emit('device', area=area_record(device_area), mounting=mounting_record(mounting),
         setup=setup_record(ds.decompose_display_area(device_area, mounting)), size_hint_mm=hint)
    chosen = {}

    def configure(screen_px):
        lines = ds.initial_lines(mounting, device_area, hint, screen_px)
        chosen.update(lines=lines, screen_px=screen_px)
        emit('initial_lines', left=lines.left, right=lines.right, size_mm=list(lines.size_mm),
             tracker_offset_mm=lines.tracker_offset_mm, aspect=lines.aspect, source=lines.source,
             screen_px=screen_px)
        return {'left': lines.left, 'right': lines.right, 'aspect': lines.aspect,
                'tracker_width_mm': mounting.width_mm, 'picture_proportion': ds.picture_proportion(mounting),
                'instructions': 'Move the two lines to the marks on top of your eye tracker.'}

    evaluate = evaluate_lines(device_area, mounting)

    def on_confirm(left, right, viewport):
        if 'lines' not in chosen:
            configure(None)
        outcome = evaluate(left, right, viewport, chosen['lines'].aspect)
        size, offset = outcome['size_mm'], outcome['tracker_offset_mm']
        outcome['message'] = 'Display setup saved' + ('' if args.apply else ' (not sent to the tracker)')
        outcome['summary'] = f'{size[0]:.1f} x {size[1]:.1f} mm, tracker offset {offset:+.1f} mm'
        if args.then_calibrate:
            outcome['next_url'] = f'http://localhost:{args.calibration_port}/'
        return outcome

    page = DisplaySetupPage(configure, on_confirm, args.host, args.port).start()
    try:
        emit('page', url=page.url, auto_url=page.url + 'auto')
        result = page.wait_result(args.page_timeout_s)
        outcome = result['reply']
        emit('confirmed', **{k: v for k, v in outcome.items() if k not in ('message', 'summary', 'next_url')})
        area = ds.DisplayArea(*(tuple(outcome['area'][k]) for k in ('top_left', 'top_right', 'bottom_left')))
        setpm, json_path = save_setup(args.output, area, args.overwrite)
        emit('saved', setpm=str(setpm), json=str(json_path), display_option=area.floats)
        status = apply_area(session, device_area, area, args.apply)
        page.finish('Display setup sent to the tracker' if args.apply else
                    'Display setup saved; not sent to the tracker (run with --apply to send it)')
        return status
    except Exception:
        page.finish('Display setup stopped')
        raise
    finally:
        page.close()


def apply_area(session, before, area, send):
    request = gd.set_display_area(0, *area.corners).encode()
    emit('apply', before=area_record(before), wanted=area_record(area), identical_to_device=area == before,
         request_bytes=len(request), request_sha256=hashlib.sha256(request).hexdigest(), dry_run=not send)
    if not send:
        device = cal.DryRunSession(session, report=lambda r: emit('would_send', **r), allowed=DISPLAY_READS)
        ds.apply_display_area(device, area)
        return 0
    after = ds.apply_display_area(session, area)
    emit('applied', after=area_record(after), device_holds_wanted=after == area)
    return 0 if after == area else 3


def calibrate_after_setup(args):
    import subprocess
    command = [sys.executable, str(Path(__file__).with_name('gaze_calibrate.py')), 'calibrate',
               '--port', str(args.calibration_port),
               '--before', str(Path(args.output).with_name('calibration-before.setpm')),
               '--after', str(Path(args.output).with_name('calibration-after.setpm'))]
    if not args.apply:
        command.append('--dry-run')
    emit('calibrate', command=command)
    return subprocess.call(command)


def print_args(args):
    print(' '.join(repr(v) for v in ds.load_display_area(args.file).floats))
    return 0


def add_size_arguments(parser):
    size = parser.add_mutually_exclusive_group()
    size.add_argument('--edid', help='raw EDID file, e.g. /sys/class/drm/card0-DP-1/edid')
    size.add_argument('--connector', help='DRM connector name, e.g. card0-DP-1')
    size.add_argument('--size', type=float, nargs=2, metavar=('WIDTH', 'HEIGHT'), help='visible area in mm')
    size.add_argument('--diagonal', type=float, metavar='INCHES')
    parser.add_argument('--aspect', default='16:9', help='aspect ratio for --diagonal')
    parser.add_argument('--portrait', action='store_true', help='monitor rotated by 90 degrees')
    offset = parser.add_mutually_exclusive_group()
    offset.add_argument('--offset', type=float, default=0.0, metavar='MM',
                        help='tracker centre relative to the display centre, positive to the right')
    offset.add_argument('--lines', type=float, nargs=2, metavar=('LEFT', 'RIGHT'),
                        help="tracker ends on screen, 0 to 1 from the left; sets width and offset like the app")


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--channel', type=int, default=0)
    parser.add_argument('--index', type=int, default=0)
    modes = parser.add_subparsers(dest='mode', required=True)
    modes.add_parser('show')
    p = modes.add_parser('compute')
    add_size_arguments(p)
    p.add_argument('--mounting', type=float, nargs=9,
                   metavar=('GUIDES', 'WIDTH', 'ANGLE', 'EX', 'EY', 'EZ', 'IX', 'IY', 'IZ'),
                   help='2110 geometry instead of the Eye Tracker 5 values')
    p.add_argument('--save', help='write the area in screenplane.setpm layout')
    p.add_argument('--overwrite', action='store_true')
    p = modes.add_parser('apply')
    add_size_arguments(p)
    p.add_argument('--file', help='saved display area to send')
    p.add_argument('--from-device', action='store_true', help='send back exactly what 1430 returns')
    p.add_argument('--dry-run', action='store_true')
    p = modes.add_parser('args')
    p.add_argument('file')
    p = modes.add_parser('setup')
    size = p.add_mutually_exclusive_group()
    size.add_argument('--edid', help='EDID whose size hint replaces the stored size, as the app does')
    size.add_argument('--connector', help='DRM connector whose EDID gives the size hint')
    p.add_argument('--output', default='display-area', help='file stem for the .setpm and .json results')
    p.add_argument('--overwrite', action='store_true')
    p.add_argument('--apply', action='store_true', help='send the confirmed area to the tracker (1440)')
    p.add_argument('--host', default='127.0.0.1')
    p.add_argument('--port', type=int, default=8776)
    p.add_argument('--page-timeout-s', type=float, default=600.0)
    p.add_argument('--then-calibrate', action='store_true',
                   help='continue with the calibration page, as Tobii runs calibration after display setup')
    p.add_argument('--calibration-port', type=int, default=8775)
    args = parser.parse_args()
    try:
        if args.mode == 'compute':
            return compute(args)
        if args.mode == 'args':
            return print_args(args)
        with ExitStack() as stack:
            session = open_session(args, stack)
            if args.mode == 'setup':
                status = setup(session, args)
            else:
                return show(session, args) if args.mode == 'show' else apply(session, args)
        return calibrate_after_setup(args) if status == 0 and args.then_calibrate else status
    except Exception as error:
        emit('error', type=type(error).__name__, message=str(error))
        return 1


if __name__ == '__main__':
    raise SystemExit(main())
