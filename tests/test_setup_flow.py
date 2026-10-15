from contextlib import contextmanager
import json
from pathlib import Path
import tempfile
import threading
import time
import unittest
import urllib.request

from tobii_linux import browser
from tobii_linux import calibration as cal
from tobii_linux import display_setup as ds
from tobii_linux.driver import TrackerSession
from tobii_linux.gaze_device import GazeCommand
from tobii_linux.setup import flow as setup_flow
from tobii_linux.setup.server import Cancelled, PageChannel, SetupServer, fills_screen
from tobii_linux.simulated_tracker import ALBEDO_AREA, SimulatedTracker, SimulatedUser
from tobii_linux.ttp_commands import Command

# The flow runs against SimulatedTracker, a scripted device following the
# command semantics recovered from the runtime and the Tobii client, NOT device traffic.
FAST = {'collect_s': 0.05, 'compute_s': 0.05}
FULLSCREEN_VIEW = {'width': 1280, 'height': 720, 'screen_width': 1280, 'screen_height': 720,
                   'screen_px': [1280, 720], 'fullscreen': False}


class ScriptedPage(threading.Thread):
    """Plays the browser and the user in front of it.

It acknowledges every screen, answers with `answers[name]` (an action, or a
function of the screen returning one), and looks at the first calibration
target on screen that has not exploded yet.
"""

    def __init__(self, channel, answers=None, view=FULLSCREEN_VIEW, monitor=None):
        super().__init__(daemon=True)
        self.monitor = monitor
        self.channel = channel
        self.answers = {'positioning': {'action': 'continue'}, **(answers or {})}
        self.view = view
        self.seen = []
        self.targets = {}
        self.stopped = threading.Event()

    def looking_at(self):
        return next(iter(self.targets.values()), (0.5, 0.5))

    def follow_targets(self, screen):
        phase = screen.get('phase')
        if phase == 'targets':
            self.targets = {t['index']: (t['x'], t['y']) for t in screen['targets']}
        elif phase == 'pop':
            self.targets.pop(screen['index'], None)
        elif phase in ('intro', 'restart'):
            self.targets = {}

    def run(self):
        last = None
        self.channel.page_event({'seq': -1, 'view': self.view, 'monitor': self.monitor})
        while not self.stopped.is_set():
            screen = self.channel.snapshot()
            if screen.get('monitor') and screen['monitor'] != self.monitor:
                return
            if screen['seq'] != last:
                last = screen['seq']
                self.seen.append((screen['name'], screen.get('phase')))
                if screen['name'] == 'calibration':
                    self.follow_targets(screen)
                self.channel.page_event({'seq': last, 'ack': True, 'view': self.view, 'monitor': self.monitor})
                answer = self.answers.get(screen['name'])
                event = answer(screen) if callable(answer) else answer
                if event:
                    self.channel.page_event({'seq': last, 'view': self.view, 'monitor': self.monitor, **event})
            time.sleep(0.005)

    def names(self):
        return [name for name, _ in self.seen]


def done_with(left_delta=0.0):
    return lambda screen: {'action': 'done', 'left': screen['left'] + left_delta, 'right': screen['right']}


class FlowHarness:
    def __init__(self, test, tracker=None, answers=None, desktop=None, **options):
        self.tracker = tracker or SimulatedTracker(user=SimulatedUser(seated_after_s=0.2), **FAST)
        self.channel = PageChannel()
        directory = Path(tempfile.mkdtemp())
        test.addCleanup(lambda: [p.unlink() for p in directory.rglob('*') if p.is_file()])
        self.profiles = directory/'profiles'
        self.reports = []
        self.record = directory/'display-setup.json'
        self.options = setup_flow.SetupOptions(profiles_directory=self.profiles, backups_directory=directory/'backups',
                                               display_record=self.record, **options)
        self.desktop = desktop
        self.page = ScriptedPage(self.channel, answers, monitor=desktop.current.key if desktop else None)
        self.tracker.looking_at = self.page.looking_at

    @contextmanager
    def session(self):
        yield TrackerSession(self.tracker)

    def run(self):
        self.page.start()
        flow = setup_flow.SetupFlow(self.channel, self.session, self.options,
                                    report=lambda kind, **values: self.reports.append((kind, values)),
                                    desktop=self.desktop)
        try:
            outcome = flow.run()
            self.channel.wait_ack(self.channel.screen['seq'], 2)
            return outcome
        finally:
            self.page.stopped.set()

    def commands(self):
        return self.tracker.commands()


SAVE_PROFILE = {'profile': {'action': 'save', 'name': 'Garulf'}}


class FlowTests(unittest.TestCase):
    def test_first_setup_runs_display_positioning_calibration_and_names_the_profile(self):
        tracker = SimulatedTracker(mapped=False, user=SimulatedUser(seated_after_s=0.2), **FAST)
        harness = FlowHarness(self, tracker, {'display': done_with(), 'display-save': {'action': 'save'},
                                              **SAVE_PROFILE})
        outcome = harness.run()
        self.assertEqual(outcome.status, 'finished', outcome.message)
        names = harness.page.names()
        for screen in ('display', 'display-save', 'positioning', 'calibration', 'profile', 'finished'):
            self.assertIn(screen, names)
        self.assertLess(names.index('display'), names.index('positioning'))
        self.assertLess(names.index('positioning'), names.index('calibration'))
        self.assertTrue(outcome.display_written)
        self.assertTrue(ds.is_mapped(tracker.area))
        self.assertEqual(outcome.profile, 'Garulf')
        store = setup_flow.profiles.ProfileStore(harness.profiles)
        self.assertEqual((store.names(), store.current()), (['Garulf'], 'Garulf'))
        self.assertEqual(store.load('Garulf').calibration.calibration_id, tracker.calibration_id)
        self.assertNotIn(GazeCommand.CALIBRATION_APPLY, harness.commands())

    def test_mapped_tracker_skips_the_display_and_the_intro(self):
        harness = FlowHarness(self, answers=SAVE_PROFILE)
        outcome = harness.run()
        self.assertEqual(outcome.status, 'finished')
        self.assertNotIn('display', harness.page.names())
        self.assertNotIn(GazeCommand.SET_DISPLAY_AREA, harness.commands())
        phases = [phase for name, phase in harness.page.seen if name == 'calibration']
        self.assertEqual(phases[:2], ['intro', 'targets'])
        self.assertEqual(phases.count('pop'), 7)

    def test_unchanged_lines_write_nothing_and_ask_nothing(self):
        harness = FlowHarness(self, answers={'display': done_with(), **SAVE_PROFILE}, display='always',
                              calibrate=False)
        outcome = harness.run()
        self.assertEqual(outcome.status, 'finished')
        self.assertNotIn('display-save', harness.page.names())
        self.assertNotIn(GazeCommand.SET_DISPLAY_AREA, harness.commands())

    def test_done_outside_fullscreen_shows_the_lines_again_where_they_were(self):
        windowed = {**FULLSCREEN_VIEW, 'height': 600}
        answers = iter([lambda screen: {'action': 'done', 'left': 0.3, 'right': 0.6, 'view': windowed},
                        lambda screen: {'action': 'done', 'left': screen['left'], 'right': screen['right']}])
        shown = []

        def display(screen):
            shown.append((screen['left'], screen['right'], screen['intro']))
            return next(answers)(screen)
        harness = FlowHarness(self, SimulatedTracker(mapped=False, **FAST),
                              {'display': display, 'display-save': {'action': 'skip'}}, calibrate=False)
        harness.run()
        self.assertEqual(len(shown), 2)
        self.assertEqual(shown[1], (0.3, 0.6, False))
        self.assertTrue(shown[0][2])

    def test_a_changed_area_is_kept_when_the_user_declines(self):
        harness = FlowHarness(self, answers={'display': done_with(-0.02), 'display-save': {'action': 'skip'}},
                              display='always', calibrate=False)
        harness.run()
        self.assertIn('display-save', harness.page.names())
        self.assertNotIn(GazeCommand.SET_DISPLAY_AREA, harness.commands())

    def test_save_display_without_asking_writes_and_keeps_a_backup(self):
        harness = FlowHarness(self, answers={'display': done_with(-0.02)}, display='always', calibrate=False,
                              save_display_without_asking=True)
        outcome = harness.run()
        self.assertNotIn('display-save', harness.page.names())
        self.assertIn(GazeCommand.SET_DISPLAY_AREA, harness.commands())
        self.assertEqual(len(outcome.backups), 1)
        self.assertEqual(ds.load_display_area(outcome.backups[0]), ALBEDO_AREA.as_float32())

    def test_recalibration_improves_the_current_profile_without_asking_for_a_name(self):
        harness = FlowHarness(self)
        store = setup_flow.profiles.ProfileStore(harness.profiles)
        store.save('Garulf', cal.backup_calibration(TrackerSession(harness.tracker)))
        store.set_current('Garulf')
        outcome = harness.run()
        self.assertEqual((outcome.status, outcome.profile), ('finished', 'Garulf'))
        self.assertNotIn('profile', harness.page.names())
        commands = harness.commands()
        self.assertIn(GazeCommand.CALIBRATION_APPLY, commands)
        self.assertLess(commands.index(GazeCommand.CALIBRATION_CLEAR), commands.index(GazeCommand.CALIBRATION_APPLY))
        self.assertEqual(store.load('Garulf').calibration.calibration_id, harness.tracker.calibration_id)

    def test_all_points_failed_offers_try_again(self):
        tracker = SimulatedTracker(fail_compute_at={2}, user=SimulatedUser(seated_after_s=0.2), **FAST)
        harness = FlowHarness(self, tracker, {'calibration-failed': {'action': 'retry'}, **SAVE_PROFILE})
        outcome = harness.run()
        self.assertEqual(outcome.status, 'finished')
        names = harness.page.names()
        self.assertIn('calibration-failed', names)
        self.assertEqual(names.count('positioning'), 2)
        self.assertGreater(len(names) - names.index('calibration-failed'), 3)

    def test_cancel_after_the_calibration_started_puts_the_previous_calibration_back(self):
        tracker = SimulatedTracker(fail_compute_at={2}, user=SimulatedUser(seated_after_s=0.2), **FAST)
        before = tracker.blob
        harness = FlowHarness(self, tracker, {'calibration-failed': {'action': 'cancel'}})
        outcome = harness.run()
        self.assertEqual(outcome.status, 'cancelled')
        self.assertEqual(tracker.blob, before)
        self.assertEqual(harness.commands()[-2], GazeCommand.CALIBRATION_APPLY)
        self.assertEqual(harness.page.names()[-1], 'cancelled')
        self.assertEqual(len(outcome.backups), 1)

    def test_cancel_while_positioning_changes_nothing(self):
        harness = FlowHarness(self, answers={'positioning': {'action': 'cancel'}})
        harness.tracker.user.present = False
        outcome = harness.run()
        self.assertEqual(outcome.status, 'cancelled')
        writes = {GazeCommand.CALIBRATION_START, GazeCommand.CALIBRATION_CLEAR, GazeCommand.CALIBRATION_APPLY,
                  GazeCommand.SET_DISPLAY_AREA}
        self.assertFalse(writes & set(harness.commands()))

    def test_profile_names_are_checked_like_the_app(self):
        attempts = iter(['', 'Guest', 'a/b', 'Garulf'])
        harness = FlowHarness(self, answers={'profile': lambda screen: {'action': 'save', 'name': next(attempts)}})
        outcome = harness.run()
        self.assertEqual(outcome.profile, 'Garulf')
        self.assertEqual(harness.page.names().count('profile'), 4)

    def test_dry_run_sends_no_write(self):
        tracker = SimulatedTracker(mapped=False, user=SimulatedUser(seated_after_s=0.2), **FAST)
        harness = FlowHarness(self, tracker, {'display': done_with(), 'display-save': {'action': 'save'},
                                              **SAVE_PROFILE}, dry_run=True)
        outcome = harness.run()
        self.assertEqual(outcome.status, 'finished')
        writes = {GazeCommand.CALIBRATION_START, GazeCommand.CALIBRATION_CLEAR, GazeCommand.CALIBRATION_COLLECT_2D,
                  GazeCommand.CALIBRATION_COMPUTE_AND_APPLY, GazeCommand.SET_DISPLAY_AREA,
                  GazeCommand.CALIBRATION_APPLY}
        self.assertFalse(writes & set(harness.commands()))
        self.assertEqual(setup_flow.profiles.ProfileStore(harness.profiles).names(), [])


class MonitorTests(unittest.TestCase):
    def albedo(self):
        from test_monitors import WINDOWS_OUTPUT
        from tobii_linux import monitors
        primary, portrait = monitors.parse_windows_monitors(WINDOWS_OUTPUT)
        return primary, portrait

    def test_previous_moves_the_page_to_the_portrait_monitor_and_records_it(self):
        primary, portrait = self.albedo()
        opened = []
        pages = []

        def open_on(monitor):
            opened.append(monitor.key)
            page = ScriptedPage(harness.channel, {'display': done_with(), 'display-save': {'action': 'save'}},
                                view={**FULLSCREEN_VIEW, 'width': 1440, 'height': 2560, 'screen_width': 1440,
                                      'screen_height': 2560, 'screen_px': [1440, 2560]}, monitor=monitor.key)
            pages.append(page)
            page.start()
            return True
        desktop = setup_flow.Desktop([portrait, primary], primary, open_on)
        harness = FlowHarness(self, answers={'display': {'action': 'previous'}}, desktop=desktop,
                              display='always', calibrate=False)
        self.addCleanup(lambda: [page.stopped.set() for page in pages])
        displays = []
        harness.page.answers['display'] = lambda screen: displays.append(screen) or {'action': 'previous'}
        outcome = harness.run()
        self.assertEqual(opened, [portrait.key])
        self.assertTrue(displays[0]['previous'])
        self.assertFalse(displays[0]['next'])
        self.assertEqual(len(displays), 1)
        self.assertEqual(outcome.status, 'finished')
        self.assertIn(GazeCommand.SET_DISPLAY_AREA, harness.commands())
        record = json.loads(harness.record.read_text())
        self.assertEqual(record['monitor']['key'], portrait.key)
        self.assertEqual(record['display_id'], portrait.display_id)
        size = [r for kind, r in harness.reports if kind == 'display_confirmed'][0]['size_mm']
        self.assertAlmostEqual(size[0], 336.0, places=3)

    def test_unchanged_area_on_the_mapped_monitor_records_its_display_id(self):
        primary, portrait = self.albedo()
        desktop = setup_flow.Desktop([portrait, primary], primary, lambda monitor: True)
        harness = FlowHarness(self, answers={'display': done_with()}, desktop=desktop, display='always',
                              calibrate=False)
        outcome = harness.run()
        self.assertNotIn(GazeCommand.SET_DISPLAY_AREA, harness.commands())
        self.assertEqual(outcome.monitor['display_id'], 'DISPLAY\\GSM5B7F\\5&5028A6B&0&UID41221##eId:'
                                                       '1E6D7F5B0B220100061E#or:1#v:1.0')

    def test_a_page_that_never_opens_keeps_the_setup_where_it_was(self):
        primary, portrait = self.albedo()
        desktop = setup_flow.Desktop([portrait, primary], primary, lambda monitor: False)
        answers = iter([{'action': 'previous'}, None])
        displays = []

        def display(screen):
            displays.append(screen)
            return next(answers) or done_with()(screen)
        harness = FlowHarness(self, answers={'display': display}, desktop=desktop, display='always', calibrate=False)
        outcome = harness.run()
        self.assertEqual(outcome.status, 'finished')
        self.assertEqual(len(displays), 2)
        self.assertEqual(outcome.monitor['key'], primary.key)

    def test_a_single_monitor_shows_no_switcher(self):
        primary, _ = self.albedo()
        displays = []
        harness = FlowHarness(self, answers={'display': lambda s: displays.append(s) or done_with()(s)},
                              desktop=setup_flow.Desktop([primary], primary, lambda monitor: True),
                              display='always', calibrate=False)
        harness.run()
        self.assertEqual((displays[0]['previous'], displays[0]['next']), (False, False))

    def test_aspect_warning_yes_stops_the_setup(self):
        primary, _ = self.albedo()
        harness = FlowHarness(self, answers={'aspect-warning': {'action': 'yes'}},
                              desktop=setup_flow.Desktop([primary], primary), display='always', calibrate=False)
        harness.page.view = {**FULLSCREEN_VIEW, 'screen_px': [1280, 1024]}
        outcome = harness.run()
        self.assertEqual(outcome.status, 'cancelled')
        self.assertIn('aspect-warning', harness.page.names())
        self.assertNotIn('display', harness.page.names())
        self.assertEqual(harness.channel.screen['title'], 'Change the screen resolution first')


class ProfileNameTests(unittest.TestCase):
    def test_rules(self):
        problem = setup_flow.profile_name_problem
        self.assertIsNone(problem('Anna (glasses)', ['Bob']))
        self.assertIsNotNone(problem('   ', []))
        self.assertIsNotNone(problem('x' * 16, []))
        self.assertIsNotNone(problem('guest', []))
        self.assertIsNotNone(problem('bob', ['Bob']))
        self.assertIsNotNone(problem('a.b', []))


class ChannelTests(unittest.TestCase):
    def test_actions_from_an_older_screen_are_ignored_but_cancel_stands(self):
        channel = PageChannel()
        first = channel.show('display')
        channel.show('display-save')
        self.assertFalse(channel.page_event({'seq': first, 'action': 'done'}))
        self.assertFalse(channel.has_action('done'))
        channel.page_event({'seq': first, 'action': 'cancel'})
        with self.assertRaises(Cancelled):
            channel.wait_action(('save',), timeout_s=1)

    def test_kiosk_window_counts_as_fullscreen(self):
        self.assertTrue(fills_screen({'width': 2560, 'height': 1440, 'screen_width': 2560, 'screen_height': 1440}))
        self.assertTrue(fills_screen({'width': 1707, 'height': 960, 'screen_width': 1708, 'screen_height': 961}))
        self.assertFalse(fills_screen({'width': 2560, 'height': 1300, 'screen_width': 2560, 'screen_height': 1440}))
        self.assertTrue(fills_screen({'fullscreen': True}))
        self.assertFalse(fills_screen(None))

    def test_server_serves_the_page_and_relays_events(self):
        channel = PageChannel()
        server = SetupServer(channel).start()
        self.addCleanup(server.close)
        page = urllib.request.urlopen(server.url, timeout=5).read()
        self.assertIn(b'setup.js', page)
        self.assertIn(b'poll', urllib.request.urlopen(server.url + 'setup.js', timeout=5).read())
        seq = channel.show('busy', title='Saving')
        state = json.loads(urllib.request.urlopen(server.url + 'state', timeout=5).read())
        self.assertEqual((state['seq'], state['name'], state['title']), (seq, 'busy', 'Saving'))
        request = urllib.request.Request(server.url + 'event', method='POST',
                                         data=json.dumps({'seq': seq, 'ack': True, 'view': FULLSCREEN_VIEW}).encode())
        self.assertEqual(json.loads(urllib.request.urlopen(request, timeout=5).read()), {'current': True})
        self.assertEqual(channel.wait_ack(seq, 1), True)
        self.assertTrue(channel.fullscreen)


class BrowserTests(unittest.TestCase):
    def test_wsl_opens_edge_in_kiosk_mode(self):
        command = browser.edge_kiosk_command('http://localhost:8777/')
        self.assertEqual(command[:4], ['cmd.exe', '/c', 'start', 'msedge'])
        self.assertIn('--kiosk', command)
        self.assertIn('http://localhost:8777/', command)

    def test_wsl_detection(self):
        self.assertTrue(browser.running_under_wsl({'WSL_DISTRO_NAME': 'Debian'}, Path('/nonexistent')))
        self.assertFalse(browser.running_under_wsl({}, Path('/nonexistent')))


class GazeChoiceTests(unittest.TestCase):
    def test_targets_are_taken_in_the_order_the_user_looks(self):
        tracker = SimulatedTracker(user=SimulatedUser(seated_after_s=0.0), **FAST)
        events = []

        class UI(cal.CalibrationUI):
            def show_targets(self, stage, targets):
                events.append(('show', stage, tuple(t[0] for t in targets)))

            def point_done(self, index, accepted):
                events.append(('done', index, accepted))

        outcome = cal.Calibrator(TrackerSession(tracker), UI(), focus=cal.FocusCheck(), choose_by_gaze=True).run()
        self.assertEqual([e for e in events if e[0] == 'show'], [('show', 0, (0,)), ('show', 1, (1, 2, 3)),
                                                                 ('show', 2, (4, 5, 6))])
        self.assertEqual([e[1] for e in events if e[0] == 'done'], [0, 1, 2, 3, 4, 5, 6])
        self.assertEqual(len(outcome.points), 7)

    def test_attention_watch(self):
        watch = cal.AttentionWatch()

        def sample(ms):
            return cal.GazeSample(int(ms * 1000), (0.5, 0.5))
        for ms in range(0, 4100, 100):
            watch.observe(sample(ms), False)
        self.assertFalse(watch.stalled)
        watch.observe(sample(4100), False)
        self.assertTrue(watch.stalled)
        looking = cal.AttentionWatch()
        for ms in range(0, 10100, 100):
            looking.observe(sample(ms), True)
        self.assertFalse(looking.stalled)
        looking.observe(sample(10100), True)
        self.assertTrue(looking.stalled)
        away = cal.AttentionWatch()
        for ms in list(range(0, 3000, 100)) + list(range(4000, 7500, 100)):
            away.observe(sample(ms), False)
        self.assertFalse(away.stalled)

    def test_ignored_targets_restart_the_calibration(self):
        tracker = SimulatedTracker(user=SimulatedUser(seated_after_s=0.0), **FAST)
        tracker.looking_at = lambda: (0.3, 0.3)
        restarts = []

        class UI(cal.CalibrationUI):
            def restarting(self, attempt):
                restarts.append(attempt)

        original = cal.ATTENTION_NO_POP_MS, cal.ATTENTION_STALL_MS
        cal.ATTENTION_NO_POP_MS, cal.ATTENTION_STALL_MS = 600.0, 300.0
        try:
            with self.assertRaises(cal.CalibrationFailed) as failure:
                cal.Calibrator(TrackerSession(tracker), UI(), focus=cal.FocusCheck(), choose_by_gaze=True,
                               max_stalled_runs=1).run()
        finally:
            cal.ATTENTION_NO_POP_MS, cal.ATTENTION_STALL_MS = original
        self.assertIn('not looked at', str(failure.exception))
        self.assertEqual(restarts, [1])
        self.assertEqual(tracker.commands().count(GazeCommand.CALIBRATION_START), 2)
        self.assertEqual(tracker.commands()[-1], Command.UNSUBSCRIBE_STREAM)


if __name__ == '__main__':
    unittest.main()
