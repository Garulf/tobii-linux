#!/usr/bin/env python3
"""Live diagnostics for the Linux gaze calibration, with a raw USB traffic log.

  host-pattern   Read only. Runs Calibrator's exact host-side pattern (gaze
                 subscription, 2.1 s pauses without reading while a target is
                 shown, the short-read drain) with every calibration write
                 replaced by reading the gaze stream for --hold-s seconds, and
                 checks after each target that the device still answers 1490.
  pause-threshold
                 Read only. Subscribes gaze and stops reading for growing
                 pauses (--pauses) until the stream does not come back, then
                 checks whether the device still answers 1490.
  collect        WRITES. 1010 start, 1060 clear, one 1030 collect at the centre
                 (eye 3), 1020 stop, with the gaze stream subscribed and read
                 continuously. Every message received while waiting is logged,
                 including responses whose request id does not match. Shows a
                 centre dot on the calibration page when --page is given.

Both write --raw-log (every bulk transfer in hex) and JSON lines on stdout.
See docs/gaze-calibration.md.
"""
import argparse
from contextlib import ExitStack
import json
from pathlib import Path
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1]/'src'))
from tobii_linux import calibration as cal  # noqa: E402
from tobii_linux import driver  # noqa: E402
from tobii_linux import gaze_device as gd  # noqa: E402
from tobii_linux import ttp_commands as tc  # noqa: E402
from tobii_linux.traffic_log import LoggingTransport, TrafficLog  # noqa: E402

READ_SLICE_MS = 100
GAZE = tc.wire_stream_id(tc.Stream.GAZE)


def emit(kind, **values):
    print(json.dumps({'kind': kind, 'time': time.time(), **values}, default=repr), flush=True)


def describe(message):
    return {'message_kind': hex(message.kind), 'request_id': message.request_id, 'word2': message.word2,
            'command': message.command, 'word4': hex(message.word4), 'payload_bytes': len(message.payload),
            'payload_head': message.payload[:64].hex()}


class Monitor:
    """Reads the session's channel and accounts for every message, without dropping any silently."""

    def __init__(self, session):
        self.session = session
        self.streams = 0
        self.last_stream = None

    def take(self, message, waiting_for=None):
        if message.kind == tc.STREAM_DATA_KIND:
            self.streams += 1
            self.last_stream = time.monotonic()
            return None
        if message.kind == tc.RESPONSE_KIND and message.request_id == waiting_for:
            return message
        emit('unexpected_message', waiting_for=waiting_for, **describe(message))
        return None

    def pump(self, seconds, waiting_for=None):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            data = self.session.channel.read(timeout_ms=READ_SLICE_MS)
            for message in self.session.decoder.feed(data) if data else ():
                reply = self.take(message, waiting_for)
                if reply is not None:
                    return reply
        return None

    def call(self, name, build, timeout_s, late_s=0.0):
        """Send one request and wait for its reply, logging stream activity each second."""
        message = build(self.session._next_id())
        self.session.channel.write(message.encode())
        sent = time.monotonic()
        emit('sent', name=name, command=message.command, request_id=message.request_id)
        for second in range(int(timeout_s + late_s)):
            before = self.streams
            reply = self.pump(1.0, message.request_id)
            if reply is not None:
                emit('reply', name=name, seconds=round(time.monotonic() - sent, 3), late=second >= timeout_s,
                     status=driver.status_name(driver.response_status(reply)), **describe(reply))
                return reply
            emit('waiting', name=name, second=second + 1, gaze_messages=self.streams - before,
                 last_stream_age_s=None if self.last_stream is None else round(time.monotonic() - self.last_stream, 3))
        emit('no_reply', name=name, seconds=round(time.monotonic() - sent, 3))
        return None


def status_id(monitor):
    reply = monitor.call('device status (1490)',
                         lambda rid: tc.header_only_request(rid, tc.Command.GET_DEVICE_STATUS), 5)
    return None if reply is None else gd.device_calibration_id(monitor.session.reply_values(reply))


class HoldingSession(cal.DryRunSession):
    """Lets reads and the gaze subscription through; every other write becomes a --hold-s read of the stream."""

    def __init__(self, session, monitor, hold_s):
        super().__init__(session, report=lambda request: emit('held', **request),
                         allowed=cal.READ_COMMANDS | {tc.Command.SUBSCRIBE_STREAM, tc.Command.UNSUBSCRIBE_STREAM})
        self.monitor = monitor
        self.hold_s = hold_s

    def call(self, build, timeout_s=cal.COMMAND_TIMEOUT_S):
        held = len(self.blocked)
        reply = super().call(build, timeout_s=timeout_s)
        if len(self.blocked) > held:
            before = self.monitor.streams
            self.monitor.pump(self.hold_s)
            emit('hold', command=reply.command, gaze_messages=self.monitor.streams - before)
            if reply.command == gd.GazeCommand.CALIBRATION_COLLECT_2D:
                emit('still_answering', device_calibration_id=status_id(self.monitor))
        return reply


class PausingUI(cal.CalibrationUI):
    """Blocks for the page's animation time, as PageCalibrationUI does; it never touches the device."""

    def __init__(self, pause_s):
        self.pause_s = pause_s

    def show_point(self, index, x, y):
        time.sleep(self.pause_s)


def host_pattern(session, args):
    monitor = Monitor(session)
    emit('start', calibration_id=status_id(monitor))
    holding = HoldingSession(session, monitor, args.hold_s)
    calibrator = cal.Calibrator(holding, PausingUI(args.pause_s), log=lambda kind, **v: emit(kind, **v))
    outcome = calibrator.run()
    emit('host_pattern_done', held=[m.command for m in holding.blocked], streams_seen=monitor.streams,
         device_unchanged=outcome.calibration.calibration_id == status_id(monitor))
    return 0


def pause_threshold(session, args):
    monitor = Monitor(session)
    if monitor.call('subscribe gaze (1220)', lambda rid: tc.subscribe_stream(rid, tc.Stream.GAZE), 5) is None:
        return 2
    for pause in args.pauses:
        monitor.pump(1.0)
        time.sleep(pause)
        before = monitor.streams
        monitor.pump(1.0)
        resumed = monitor.streams - before
        emit('pause', seconds=pause, gaze_messages_after=resumed)
        if not resumed:
            emit('silent_after_pause', seconds=pause, calibration_id=status_id(monitor))
            return 0
    monitor.call('unsubscribe gaze (1230)', lambda rid: tc.unsubscribe_stream(rid, tc.Stream.GAZE), 5)
    emit('no_silence', pauses=args.pauses, calibration_id=status_id(monitor))
    return 0


def collect(session, args):
    monitor = Monitor(session)
    page = None
    if args.page:
        from tobii_linux.calibration_page import PageCalibrationUI
        page = PageCalibrationUI(args.host, args.port, settle_s=0).start()
        emit('page', url=page.url)
        emit('page_ready', **page.wait_for_display(args.display_timeout_s))
    try:
        emit('before', calibration_id=status_id(monitor))
        if monitor.call('subscribe gaze (1220)', lambda rid: tc.subscribe_stream(rid, tc.Stream.GAZE), 5) is None:
            return 2
        if page:
            page.page.publish(phase='point', index=0, x=0.5, y=0.5)
        before = monitor.streams
        monitor.pump(args.look_s)
        emit('looking', seconds=args.look_s, gaze_messages=monitor.streams - before)
        for name, command in (('start (1010)', gd.GazeCommand.CALIBRATION_START),
                              ('clear (1060)', gd.GazeCommand.CALIBRATION_CLEAR)):
            if monitor.call(name, lambda rid, c=command: gd.header_only(rid, c), 5) is None:
                break
        else:
            monitor.pump(0.5)
            monitor.call('collect centre (1030)', lambda rid: gd.calibration_collect_2d(rid, 0.5, 0.5, gd.Eye.BOTH),
                         args.collect_timeout_s, late_s=args.late_s)
        monitor.call('stop (1020)', lambda rid: gd.header_only(rid, gd.GazeCommand.CALIBRATION_STOP), 10)
        monitor.call('unsubscribe gaze (1230)', lambda rid: tc.unsubscribe_stream(rid, tc.Stream.GAZE), 5)
        emit('after', calibration_id=status_id(monitor))
    finally:
        if page:
            page.message('Done')
            page.close()
    return 0


def open_session(args, stack, log):
    from tobii_linux.usb import USBContext
    from tobii_linux.usb_channel import USBChannel
    context = stack.enter_context(USBContext())
    device = context.probe()[args.index]
    transport = stack.enter_context(context.open_transport(bus=device['bus'], address=device['address']))
    channel = stack.enter_context(USBChannel(LoggingTransport(transport, log), args.channel))
    session = driver.TrackerSession(channel)
    session.initialize()
    session.camera_calibration()
    return session


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('mode', choices=('host-pattern', 'pause-threshold', 'collect'))
    parser.add_argument('--raw-log', required=True)
    parser.add_argument('--channel', type=int, default=0)
    parser.add_argument('--index', type=int, default=0)
    parser.add_argument('--hold-s', type=float, default=1.0)
    parser.add_argument('--pause-s', type=float, default=2.1)
    parser.add_argument('--look-s', type=float, default=4.0)
    parser.add_argument('--collect-timeout-s', type=float, default=15.0)
    parser.add_argument('--late-s', type=float, default=20.0)
    parser.add_argument('--pauses', type=float, nargs='+', default=[0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 2.0, 3.0])
    parser.add_argument('--page', action='store_true')
    parser.add_argument('--host', default='127.0.0.1')
    parser.add_argument('--port', type=int, default=8775)
    parser.add_argument('--display-timeout-s', type=float, default=180.0)
    args = parser.parse_args()
    log = TrafficLog(args.raw_log)
    try:
        with ExitStack() as stack:
            session = open_session(args, stack, log)
            modes = {'host-pattern': host_pattern, 'pause-threshold': pause_threshold, 'collect': collect}
            return modes[args.mode](session, args)
    except Exception as error:
        emit('error', type=type(error).__name__, message=str(error))
        return 1
    finally:
        log.close()


if __name__ == '__main__':
    raise SystemExit(main())
