#!/usr/bin/env python3
"""Back up, verify, restore and run the user gaze calibration of an Eye Tracker 5 from Linux.

Modes (JSON lines on stdout):

  backup OUT [--compare calibration.setpm]   read the device calibration (1490 key 7, 1100)
                                              into OUT in calibration.setpm layout; reads only
  verify FILE                                 compare FILE with the device id and blob; reads only
  restore FILE [--dry-run]                    apply FILE (1110) and check the device id
  calibrate --before B --after A [--dry-run]  back up to B, run the Tobii Experience 7-target
                                              sequence, save the result to A
  profiles list                               list saved calibration profiles; no device access
  profiles save NAME [--overwrite]            save the device calibration as profile NAME; reads only
  profiles switch NAME [--dry-run]            apply profile NAME (3160, 1110) as the Tobii engine does
                                              on a profile change; --directory picks the store

calibrate shows its targets on a page served at http://localhost:PORT/ (open it
fullscreen on the display the tracker is mounted on; under WSL a Windows
browser reaches it through mirrored networking). --ui none skips the display,
which only makes sense with --dry-run. --dry-run forwards read commands to the
device and prints every other command instead of sending it.
See docs/gaze-calibration.md.
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
from tobii_linux import calibration_profiles as profiles_store  # noqa: E402
from tobii_linux import driver  # noqa: E402
from tobii_linux import gaze_device as gd  # noqa: E402


def emit(kind, **values):
    print(json.dumps({'kind': kind, 'time': time.time(), **values}, default=repr), flush=True)


def digest(data):
    return {'bytes': len(data), 'sha256': hashlib.sha256(data).hexdigest()}


def open_session(args, stack):
    """The startup the read-only query tool uses: upgrade, unlock_ppu, realm 0x2712 with hardware info.

With --raw-log every USB transfer is recorded (tobii_linux.traffic_log).
"""
    from tobii_linux.usb import USBContext
    from tobii_linux.usb_channel import USBChannel
    context = stack.enter_context(USBContext())
    device = context.probe()[args.index]
    transport = stack.enter_context(context.open_transport(bus=device['bus'], address=device['address']))
    if args.raw_log:
        from tobii_linux.traffic_log import LoggingTransport, TrafficLog
        log = TrafficLog(args.raw_log)
        stack.callback(log.close)
        transport = LoggingTransport(transport, log)
    channel = stack.enter_context(USBChannel(transport, args.channel))
    session = driver.TrackerSession(channel)
    session.initialize()
    session.camera_calibration()
    return session


def dry_run(session, enabled):
    if not enabled:
        return session
    return cal.DryRunSession(session, report=lambda request: emit('would_send', **request))


def report_check(check):
    emit('verify', file_id=hex(check.file_id), device_id=hex(check.device_id), ids_match=check.ids_match,
         blob_matches=check.blob_matches, file_blob_sha256=check.file_blob_sha256,
         device_blob_sha256=check.device_blob_sha256)
    return 0 if check.ok else 3


def backup(session, args):
    calibration = cal.backup_calibration(session)
    path = cal.save_backup(args.output, calibration, overwrite=args.overwrite)
    written = path.read_bytes()
    emit('backup', path=str(path), calibration_id=hex(calibration.calibration_id),
         blob_id=hex(gd.blob_calibration_id(calibration.blob)), blob=digest(calibration.blob), file=digest(written))
    status = report_check(cal.verify_backup(session, cal.load_backup(path)))
    if args.compare:
        reference = Path(args.compare).read_bytes()
        equal = written[16:] == reference[16:]
        emit('compare', reference=str(args.compare), reference_file=digest(reference),
             equal_from_byte_16=equal, equal_whole_file=written == reference,
             reference_id=hex(gd.parse_calibration_setting(reference).calibration_id))
        status = status or (0 if equal else 4)
    return status


def verify(session, args):
    return report_check(cal.verify_backup(session, cal.load_backup(args.file)))


def restore(session, args):
    calibration = cal.load_backup(args.file)
    before = cal.read_device_calibration_id(session)
    emit('restore', file=str(args.file), file_id=hex(calibration.calibration_id), device_id_before=hex(before))
    if args.dry_run:
        dry_run(session, True).call(lambda rid: gd.calibration_apply(rid, calibration.blob))
        return 0
    after = cal.restore_calibration(session, calibration)
    emit('restored', device_id=hex(after))
    return report_check(cal.verify_backup(session, calibration))


class LoggedUI(cal.CalibrationUI):
    def __init__(self, inner):
        self.inner = inner

    def show_point(self, index, x, y):
        emit('show_point', index=index, x=x, y=y)
        self.inner.show_point(index, x, y)

    def point_done(self, index, accepted):
        emit('point_done', index=index, accepted=accepted)
        self.inner.point_done(index, accepted)

    def stage_done(self, stage):
        emit('stage_done', stage=stage)
        self.inner.stage_done(stage)

    def restarting(self, attempt):
        emit('restarting', attempt=attempt)
        self.inner.restarting(attempt)


def calibrate(session, args):
    display_mm = cal.read_display_size(session)
    targets = cal.calibration_targets(display_mm)
    emit('display', size_mm=[round(v, 1) for v in display_mm], area=cal.calibration_area(display_mm), targets=targets)
    before = cal.backup_calibration(session)
    if not args.dry_run:
        cal.save_backup(args.before, before, overwrite=args.overwrite)
    emit('backup_before', path=None if args.dry_run else str(args.before),
         calibration_id=hex(before.calibration_id), blob=digest(before.blob))
    page = None
    ui = cal.CalibrationUI()
    focus = None
    if args.ui == 'page':
        from tobii_linux.calibration_page import PageCalibrationUI
        page = PageCalibrationUI(args.host, args.port, settle_s=args.settle_s).start()
        emit('page', url=page.url)
        display = page.wait_for_display(args.display_timeout_s)
        emit('page_ready', **display)
        ui = page
        if not args.dry_run:
            focus = cal.FocusCheck(page.screen_px)
    device = dry_run(session, args.dry_run)
    calibrator = cal.Calibrator(device, LoggedUI(ui), targets=targets, focus=focus,
                                previous=before if args.improve else None,
                                wait_for_fixation=args.ui == 'page' and not args.dry_run,
                                log=lambda kind, **v: emit(kind, **v))
    try:
        return run_and_save(session, args, calibrator, before, page)
    finally:
        if page:
            page.close()


def run_and_save(session, args, calibrator, before, page):
    try:
        outcome = calibrator.run()
    except cal.CalibrationFailed as failure:
        emit('calibration_failed', step=failure.step, error=str(failure.cause),
             device_id=hex(cal.read_device_calibration_id(session)), previous_id=hex(before.calibration_id),
             restore=f'tools/gaze_calibrate.py restore {args.before}')
        if page:
            page.message('Calibration failed')
        return 5
    emit('calibration', calibration_id=hex(outcome.calibration.calibration_id), blob=digest(outcome.calibration.blob),
         discarded=outcome.discarded, notifications=len(calibrator.notifications))
    emit('quality', stimulus_points=cal.quality_report(outcome.stimulus_points))
    if page and page.screen_px[0]:
        emit('points', records=cal.points_report(outcome.points, page.screen_px))
    if args.dry_run:
        emit('dry_run', blocked=len(calibrator.session.blocked), device_unchanged=outcome.calibration == before)
        return 0
    cal.save_backup(args.after, outcome.calibration, overwrite=args.overwrite)
    emit('saved', path=str(args.after))
    if page:
        page.message('Calibration complete')
    return 0


def list_profiles(store):
    current = store.current()
    for name in store.names():
        profile = store.load(name)
        emit('profile', name=name, current=name == current, path=str(profile.path),
             calibration_id=hex(profile.calibration.calibration_id), blob=digest(profile.calibration.blob))
    return 0


def profiles(session, args):
    store = profiles_store.ProfileStore(args.directory)
    if args.action == 'save':
        profile = profiles_store.save_from_device(session, store, args.name, overwrite=args.overwrite)
        emit('profile_saved', name=profile.name, path=str(profile.path),
             calibration_id=hex(profile.calibration.calibration_id), blob=digest(profile.calibration.blob))
        return 0
    result = profiles_store.switch(dry_run(session, args.dry_run), store, args.name)
    emit('profile_switch', name=result.profile, device_id_before=hex(result.device_id_before),
         device_id_after=hex(result.device_id_after), applied=result.applied, dry_run=args.dry_run)
    return 0


MODES = {'backup': backup, 'verify': verify, 'restore': restore, 'calibrate': calibrate, 'profiles': profiles}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--channel', type=int, default=0)
    parser.add_argument('--index', type=int, default=0)
    parser.add_argument('--raw-log', help='record every USB transfer in hex to this JSON lines file')
    modes = parser.add_subparsers(dest='mode', required=True)
    p = modes.add_parser('backup')
    p.add_argument('output')
    p.add_argument('--compare', help='calibration.setpm to compare with, from byte 16 on')
    p.add_argument('--overwrite', action='store_true')
    p = modes.add_parser('verify')
    p.add_argument('file')
    p = modes.add_parser('restore')
    p.add_argument('file')
    p.add_argument('--dry-run', action='store_true')
    p = modes.add_parser('calibrate')
    p.add_argument('--before', required=True, help='backup of the current calibration, written first')
    p.add_argument('--after', required=True, help='the new calibration, written on success')
    p.add_argument('--dry-run', action='store_true')
    p.add_argument('--improve', action='store_true',
                   help='re-apply the current calibration after clearing, as the Tobii recalibration does')
    p.add_argument('--ui', choices=('page', 'none'), default='page')
    p.add_argument('--host', default='127.0.0.1')
    p.add_argument('--port', type=int, default=8775)
    p.add_argument('--settle-s', type=float, default=0.2)
    p.add_argument('--display-timeout-s', type=float, default=120.0)
    p.add_argument('--overwrite', action='store_true')
    p = modes.add_parser('profiles')
    p.add_argument('action', choices=('list', 'save', 'switch'))
    p.add_argument('name', nargs='?')
    p.add_argument('--directory', help='profile store, default $XDG_DATA_HOME/tobii-linux/profiles')
    p.add_argument('--dry-run', action='store_true')
    p.add_argument('--overwrite', action='store_true')
    args = parser.parse_args()
    if args.mode == 'calibrate' and args.ui == 'none' and not args.dry_run:
        parser.error('calibrate without a display needs --dry-run')
    if args.mode == 'profiles' and args.action != 'list' and not args.name:
        parser.error(f'profiles {args.action} needs a profile name')
    try:
        if args.mode == 'profiles' and args.action == 'list':
            return list_profiles(profiles_store.ProfileStore(args.directory))
        with ExitStack() as stack:
            session = open_session(args, stack)
            return MODES[args.mode](session, args)
    except Exception as error:
        emit('error', type=type(error).__name__, message=str(error))
        return 1


if __name__ == '__main__':
    raise SystemExit(main())
