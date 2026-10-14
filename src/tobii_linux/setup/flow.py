"""The guided setup of Tobii's configuration app, step by step on one page.

The order and the rules follow Tobii.Configuration (docs/setup-flow.md):

1. Display setup, when the device holds no display area (Tobii: no monitor
   mapped yet) or when asked for: "Where is your eye tracker?" only for a
   first setup, then the two lines to line up with the tracker's marks.
2. Eye positioning: the user's eyes in the track box from the user position
   guide, with Tobii's hints, until they sit well (or press a key).
3. Calibration: 1 target, then 3, then 3, each stage's targets shown at
   once and taken in the order the user looks at them.
4. The result: a new profile is named and saved; a recalibration is saved
   to the current profile.

Writes stay behind the user: a changed display area is sent (1440) only
after the user pressed Done and, unless the command allows it, confirmed
saving it; the calibration is changed only once positioning ended and the
calibration started. Leaving after the calibration started (Esc, Cancel, a
failure the user does not retry, a closed page) puts the previous calibration
back (1110). The previous calibration and display area are also saved to
files first. With dry_run every write is reported instead of sent.

All tracker work runs on the thread that calls run(); while gaze is
subscribed it never pauses reading (see docs/gaze-calibration.md).
"""
from contextlib import contextmanager
from dataclasses import dataclass, field
import getpass
import logging
from pathlib import Path
import time

from .. import calibration as cal
from .. import calibration_profiles as profiles
from .. import display_setup as ds
from ..eye_positioning import PositioningGuide, guide_sample
from ..gaze import gaze_message
from ..display_setup_page import valid_lines
from ..ttp_commands import (STREAM_DATA_KIND, Command, Stream, subscribe_stream, unsubscribe_stream,
                           wire_stream_id)
from .server import Cancelled, PageGone

ACK_TIMEOUT_S = 15.0
PAGE_FIRST_SEEN_TIMEOUT_S = 180.0
READ_SLICE_MS = 100
PROFILE_NAME_LENGTH = 15
RESERVED_PROFILE = 'Guest'
DRY_RUN_ALLOWED = cal.READ_COMMANDS | {Command.SUBSCRIBE_STREAM, Command.UNSUBSCRIBE_STREAM}

log = logging.getLogger(__name__)


@dataclass
class SetupOptions:
    display: str = 'auto'
    calibrate: bool = True
    save_display_without_asking: bool = False
    dry_run: bool = False
    profile: str | None = None
    profiles_directory: Path | None = None
    backups_directory: Path | None = None
    size_hint_mm: tuple | None = None


@dataclass
class SetupOutcome:
    status: str = 'running'
    display_written: bool = False
    calibration_id: int | None = None
    profile: str | None = None
    backups: list = field(default_factory=list)
    message: str = ''


def default_backups_directory():
    return profiles.default_directory().parent/'backups'


def profile_name_problem(name, existing):
    """NewProfileViewModel's checks, or None when the name can be saved."""
    name = name.strip()
    if not name:
        return 'Please type a name for the profile.'
    if len(name) > PROFILE_NAME_LENGTH:
        return f'Use at most {PROFILE_NAME_LENGTH} characters.'
    try:
        profiles.check_name(name)
    except profiles.ProfileError:
        return 'Use only letters, digits, spaces, brackets and dashes.'
    if name.lower() == RESERVED_PROFILE.lower():
        return 'Guest is a reserved name.'
    if name.lower() in (n.lower() for n in existing):
        return 'A profile with this name exists already.'
    return None


def suggested_profile_name(existing):
    try:
        name = getpass.getuser()[:PROFILE_NAME_LENGTH]
    except (KeyError, OSError):
        return ''
    return '' if existing or profile_name_problem(name, existing) else name


def read_device_messages(session, timeout_ms=READ_SLICE_MS):
    data = session.read(timeout_ms)
    return session.decoder.feed(data) if data else []


class PageCalibrationUI(cal.CalibrationUI):
    """Calibrator callbacks as page events; every event waits until the page drew it, so none is skipped."""

    def __init__(self, channel, welcome):
        self.channel = channel
        self.welcome = welcome

    def _show(self, **data):
        self.channel.wait_ack(self.channel.show('calibration', **data), ACK_TIMEOUT_S)

    def calibration_started(self, attempt):
        self._show(phase='intro', welcome=self.welcome and attempt == 0, again=attempt > 0)

    def show_targets(self, stage, targets):
        self._show(phase='targets', stage=stage, targets=[{'index': i, 'x': x, 'y': y} for i, x, y in targets])

    def focus(self, index, focused):
        self._show(phase='focus', index=index, focused=focused)

    def point_done(self, index, accepted):
        self._show(phase='pop' if accepted else 'missed', index=index)

    def computing(self, stage):
        self._show(phase='computing', stage=stage)

    def stage_done(self, stage):
        self._show(phase='computed', stage=stage)

    def restarting(self, attempt):
        self._show(phase='restart', attempt=attempt)

    def check(self):
        self.channel.check_cancel()


class SetupFlow:
    """Runs the steps against one tracker session; `open_session()` is a context manager yielding it."""

    def __init__(self, channel, open_session, options=None, report=None, clock=time.monotonic):
        self.channel = channel
        self.open_session = open_session
        self.options = options or SetupOptions()
        self.report = report or (lambda kind, **values: None)
        self.clock = clock
        self.outcome = SetupOutcome()
        self.store = profiles.ProfileStore(self.options.profiles_directory)
        self.backups = Path(self.options.backups_directory or default_backups_directory())
        self.session = None
        self.device = None
        self.calibration_before = None

    def run(self) -> SetupOutcome:
        try:
            with self.open_session() as session:
                self.session = session
                self.device = (cal.DryRunSession(session, self._would_send, DRY_RUN_ALLOWED) if self.options.dry_run
                               else session)
                self._steps()
        except Cancelled:
            self._finish_cancelled()
        except PageGone as gone:
            self._finish_cancelled(str(gone))
        except Exception as error:
            log.exception('setup failed')
            self._fail(error)
        return self.outcome

    def _would_send(self, request):
        self.report('would_send', **request)

    def _steps(self):
        self.channel.show('connecting', ready=True)
        self._wait_for_page()
        area, mounting = ds.read_display_area(self.session), ds.read_mounting(self.session)
        self.report('device', display_mm=[round(area.width_mm, 1), round(area.height_mm, 1)], mapped=ds.is_mapped(area))
        wanted = self.options.display
        if wanted == 'always' or (wanted == 'auto' and not ds.is_mapped(area)):
            area = self._display_step(area, mounting)
        if not self.options.calibrate:
            self._finish('Display setup done', self._display_summary(area))
            return
        if not ds.is_mapped(area):
            raise RuntimeError('The eye tracker has no display setup; run the display setup first')
        self._positioning_step(ran_display=self.outcome.display_written)
        outcome, profile = self._calibration_step(area)
        self._result_step(outcome, profile)

    def _wait_for_page(self):
        self.channel.wait_action((), PAGE_FIRST_SEEN_TIMEOUT_S, ready=lambda: self.channel.view is not None)

    def _screen_px(self):
        view = self.channel.view or {}
        return tuple(view.get('screen_px') or (view.get('width', 1920), view.get('height', 1080)))

    # Display ------------------------------------------------------------------------------------

    def _display_summary(self, area):
        size = (area.width_mm, area.height_mm)
        return f'Screen {size[0]:.0f} x {size[1]:.0f} mm ({(size[0] ** 2 + size[1] ** 2) ** 0.5 / 25.4:.1f} in)'

    def _display_step(self, area, mounting):
        mapped = ds.is_mapped(area)
        lines = ds.initial_lines(mounting, area if mapped else None, self.options.size_hint_mm, self._screen_px())
        self.report('initial_lines', left=lines.left, right=lines.right, source=lines.source)
        left, right, intro = lines.left, lines.right, not mapped
        while True:
            self.channel.show('display', intro=intro, left=left, right=right, aspect=lines.aspect,
                              tracker_width_mm=mounting.width_mm, picture_proportion=ds.picture_proportion(mounting),
                              instructions=ds.line_instructions(mounting))
            event = self.channel.wait_action(('done',))
            left, right = ds.clamp_lines(float(event.get('left', left)), float(event.get('right', right)))
            if valid_lines(left, right) and self.channel.fullscreen:
                break
            intro = False
        size, offset, wanted = ds.area_from_lines(left, right, lines.aspect, mounting)
        self.report('display_confirmed', left=left, right=right, size_mm=list(size), tracker_offset_mm=offset)
        if mapped and wanted.almost_equals(area):
            return area
        if not self.options.save_display_without_asking:
            self.channel.show('display-save', summary=self._display_summary(wanted),
                              offset_mm=round(offset, 1), first=not mapped)
            if self.channel.wait_action(('save', 'skip'))['action'] == 'skip':
                return area
        self.channel.show('busy', title='Saving the display setup...')
        self._backup_display_area(area)
        stored = ds.apply_display_area(self.device, wanted)
        self.outcome.display_written = not self.options.dry_run
        self.report('display_written', dry_run=self.options.dry_run, stored_matches=stored.almost_equals(wanted))
        return wanted if self.options.dry_run else stored

    def _backup_display_area(self, area):
        if not ds.is_mapped(area):
            return
        path = ds.save_display_area(self._backup_path('display-area'), area)
        self.outcome.backups.append(str(path))

    def _backup_path(self, kind):
        self.backups.mkdir(parents=True, exist_ok=True)
        return self.backups/f'{kind}-{time.strftime("%Y%m%d-%H%M%S")}.setpm'

    # Positioning --------------------------------------------------------------------------------

    def _positioning_step(self, ran_display):
        guide = PositioningGuide()
        latest = {'us': None}
        gaze_id = wire_stream_id(Stream.GAZE)

        def read():
            for message in read_device_messages(self.session):
                if message.kind == STREAM_DATA_KIND and message.command == gaze_id:
                    g = gaze_message(message)
                    guide.update(guide_sample(g))
                    latest['us'] = g.timestamp
            self.channel.update_live(eyes=guide.view())

        def ready():
            if not self.channel.fullscreen or not guide.good:
                return None
            pressed = self.channel.has_action('continue')
            return pressed or (latest['us'] is not None and guide.ready_to_advance(latest['us']))

        self.session.call(lambda rid: subscribe_stream(rid, Stream.GAZE))
        try:
            self.channel.show('positioning', sit_upright=ran_display)
            self.channel.wait_action((), idle=read, ready=ready)
            self.channel.wait_ack(self.channel.show('positioning', sit_upright=ran_display, leaving=True),
                                  ACK_TIMEOUT_S, idle=read)
        finally:
            self.channel.update_live(eyes=None)
            self.session.call(lambda rid: unsubscribe_stream(rid, Stream.GAZE))

    # Calibration --------------------------------------------------------------------------------

    def _calibration_step(self, area):
        profile = self.options.profile or self.store.current()
        recalibration = profile is not None and profile in self.store.names()
        before = cal.backup_calibration(self.session)
        path = cal.save_backup(self._backup_path('calibration'), before)
        self.outcome.backups.append(str(path))
        self.calibration_before = before
        targets = cal.calibration_targets((area.width_mm, area.height_mm))
        welcome = not recalibration
        while True:
            calibrator = cal.Calibrator(self.device, PageCalibrationUI(self.channel, welcome), targets=targets,
                                        previous=before if recalibration else None,
                                        focus=cal.FocusCheck(self._screen_px()), choose_by_gaze=True, restarts=0,
                                        log=self.report)
            try:
                return calibrator.run(), (profile if recalibration else None)
            except cal.CalibrationFailed as failure:
                if isinstance(failure.cause, (Cancelled, PageGone)):
                    raise failure.cause from None
                if getattr(self.session, 'dead', False):
                    raise
                self.report('calibration_failed', step=failure.step, error=str(failure.cause))
                self.channel.show('calibration-failed')
                self.channel.wait_action(('retry',))
                welcome = False

    def _result_step(self, outcome, profile):
        calibration = outcome.calibration
        if profile is None:
            existing = self.store.names()
            self.channel.show('profile', suggested=suggested_profile_name(existing), problem='')
            while True:
                name = str(self.channel.wait_action(('save',)).get('name', '')).strip()
                problem = profile_name_problem(name, existing)
                if problem is None:
                    break
                self.channel.show('profile', suggested=name, problem=problem)
            profile = name
        if not self.options.dry_run:
            self.store.save(profile, calibration, overwrite=True)
            self.store.set_current(profile)
        self.outcome.calibration_id = calibration.calibration_id
        self.outcome.profile = profile
        self.calibration_before = None
        self.report('saved', profile=profile, calibration_id=hex(calibration.calibration_id), dry_run=self.options.dry_run)
        self._finish('Calibration successful!', f'Saved as profile "{profile}".')

    # Endings ------------------------------------------------------------------------------------

    def _restore_calibration(self):
        before, self.calibration_before = self.calibration_before, None
        if before is None or self.session is None or getattr(self.session, 'dead', False):
            return False
        self.channel.show('busy', title='Putting your previous calibration back...')
        if self.options.dry_run:
            cal.DryRunSession(self.session, self._would_send).call(lambda rid: cal.gd.calibration_apply(rid, before.blob))
            return True
        cal.restore_calibration(self.session, before)
        self.report('restored', calibration_id=hex(before.calibration_id))
        return True

    def _finish(self, title, text):
        self.outcome.status = 'finished'
        self.outcome.message = title
        self.channel.show('finished', title=title, text=text)

    def _finish_cancelled(self, reason=''):
        self.channel.clear_cancel()
        restored = self._safely(self._restore_calibration)
        self.outcome.status = 'cancelled'
        self.outcome.message = reason or 'Cancelled'
        if restored:
            text = 'Your previous calibration is back.'
        elif self.outcome.display_written:
            text = 'The new display setup was kept.'
        else:
            text = 'Nothing was changed.'
        self.channel.show('cancelled', title='Setup cancelled', text=text)

    def _fail(self, error):
        restored = self._safely(self._restore_calibration)
        self.outcome.status = 'failed'
        self.outcome.message = str(error)
        text = str(error) + (' Your previous calibration is back.' if restored else '')
        self.channel.show('error', title='Something went wrong', text=text)

    def _safely(self, action):
        try:
            return action()
        except Exception as error:
            log.warning('could not restore the previous calibration: %s', error)
            self.report('restore_failed', error=str(error))
            return False


@contextmanager
def tracker_session(index=0, channel=0):
    """A started session with the realm the calibration and display tools open, as tools/gaze_calibrate.py."""
    from ..driver import usb_session
    with usb_session(index, channel) as session:
        session.camera_calibration()
        yield session
