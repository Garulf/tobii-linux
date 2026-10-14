#!/usr/bin/env python3
"""Serve the game DLL's tet-tcp:// route on 127.0.0.1:4455 (plus 4457 and 4459).

  --backend none   record what the DLL sends; nothing is answered
  --backend usb    relay channel 1 TTP to the first Eye Tracker 5 libusb sees
  --backend replay answer from recorded tracker replies (traffic_log raw USB logs,
                   --replay, repeatable): the first recorded reply per command,
                   an error for commands never recorded, and the recorded stream
                   messages, looped, while a stream is subscribed. Offline only.

Only read and subscribe requests reach the tracker (tet_tcp.READ_COMMANDS plus
--allow); everything else is answered with an error and logged as blocked.
--unlock negotiates the protocol and opens the unlock realm with the service
key before the DLL connects. Runs for --seconds, then prints a JSON summary.
"""
import argparse
import json
import signal
import sys
import threading
import time
from contextlib import ExitStack
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'src'))

from tobii_linux.tet_tcp import (CHANNEL_PORTS, UNSUPPORTED_OPERATION, NullBackend, RawLog,  # noqa: E402
                                 Router, TetTcpServer, UsbBackend, subscribed_stream)
from tobii_linux.traffic_log import replay_messages  # noqa: E402
from tobii_linux.ttp import Decoder, Message  # noqa: E402
from tobii_linux.ttp_commands import Command, RESPONSE_KIND, STREAM_DATA_KIND  # noqa: E402


class ReplayBackend:
    """Recorded tracker replies instead of a tracker; for exploring the DLL's requests offline."""
    def __init__(self, paths, log, extra_replies=(), period_s=1 / 33):
        self.log = log
        self.period_s = period_s
        self.replies, self.streams = {}, {}
        for command, payload in extra_replies:
            self.replies[command] = Message(RESPONSE_KIND, 0, 1, command, 0, payload)
        for path in paths:
            for _, direction, message in replay_messages(path):
                if isinstance(message, str) or direction != 'in':
                    continue
                if message.kind == RESPONSE_KIND:
                    self.replies.setdefault(message.command, message)
                elif message.kind == STREAM_DATA_KIND:
                    self.streams.setdefault(message.command, []).append(message)
        self.decoder = Decoder()
        self.on_message = None
        self.active = set()
        self.lock = threading.Lock()
        self.stopping = threading.Event()
        threading.Thread(target=self._stream, daemon=True, name='replay-streams').start()

    def send(self, data):
        for message in self.decoder.feed(data):
            stream = subscribed_stream(message)
            recorded = self.replies.get(message.command)
            if message.command == Command.SUBSCRIBE_STREAM and stream not in self.streams:
                recorded = None
            if recorded is None:
                self.log.record('replay_unanswered', command=message.command, stream=stream)
                self.on_message(Message(RESPONSE_KIND, message.request_id, 0, message.command,
                                        UNSUPPORTED_OPERATION))
                continue
            with self.lock:
                if message.command == Command.SUBSCRIBE_STREAM:
                    self.active.add(stream)
                elif message.command == Command.UNSUBSCRIBE_STREAM:
                    self.active.discard(stream)
            self.on_message(Message(recorded.kind, message.request_id, recorded.word2, recorded.command,
                                    recorded.word4, recorded.payload))

    def _stream(self):
        index = 0
        while not self.stopping.wait(self.period_s):
            with self.lock:
                active = sorted(self.active)
            for stream in active:
                recorded = self.streams[stream]
                self.on_message(recorded[index % len(recorded)])
            index += 1

    def stop(self):
        self.stopping.set()


def open_usb(stack, index=0):
    from tobii_linux.usb import USBContext
    from tobii_linux.usb_channel import USBChannel
    context = stack.enter_context(USBContext())
    devices = context.probe()
    if index >= len(devices):
        raise SystemExit('No Eye Tracker 5 is visible to libusb')
    device = devices[index]
    transport = stack.enter_context(context.open_transport(bus=device['bus'], address=device['address']))
    return stack.enter_context(USBChannel(transport, 0)), device


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--backend', choices=('none', 'usb', 'replay'), default='none')
    parser.add_argument('--replay', action='append', default=[], metavar='RAW_USB_LOG')
    parser.add_argument('--reply', action='append', default=[], metavar='COMMAND:HEX',
                        help='replay backend: answer COMMAND with this payload (synthetic, not recorded)')
    parser.add_argument('--unlock', action='store_true')
    parser.add_argument('--allow', type=int, action='append', default=[], metavar='COMMAND')
    parser.add_argument('--raw-log', required=True)
    parser.add_argument('--seconds', type=float, default=60.0)
    parser.add_argument('--host', default='127.0.0.1')
    args = parser.parse_args(argv)

    log = RawLog(args.raw_log)
    with ExitStack() as stack:
        backend = NullBackend(log)
        if args.backend == 'usb':
            channel, device = open_usb(stack)
            log.record('usb_device', device={k: v for k, v in device.items() if isinstance(v, (int, str))})
            backend = UsbBackend(channel, log)
        if args.backend == 'replay':
            extra = [(int(c), bytes.fromhex(h)) for c, h in (item.split(':', 1) for item in args.reply)]
            backend = ReplayBackend(args.replay, log, extra)
            stack.callback(backend.stop)
            log.record('replay_loaded', replies=sorted(backend.replies),
                       streams={f'0x{k:x}': len(v) for k, v in backend.streams.items()})
        router = Router(backend, log, unlock=args.unlock, allow=args.allow)
        if args.backend == 'replay':
            backend.on_message = router.from_device
        if isinstance(backend, UsbBackend):
            backend.on_message = router.from_device
            backend.start()
            stack.callback(backend.stop)
        router.prepare()
        server = TetTcpServer(router, log, host=args.host).start()
        stack.callback(server.stop)
        stack.callback(router.close)
        print(f'listening on {args.host} ports {sorted(CHANNEL_PORTS)}', flush=True)
        deadline = time.monotonic() + args.seconds
        stop = threading.Event()
        for number in (signal.SIGINT, signal.SIGTERM):
            signal.signal(number, lambda *_: stop.set())
        while time.monotonic() < deadline and not stop.is_set():
            time.sleep(0.2)
            if isinstance(backend, UsbBackend) and backend.failure is not None:
                break
        summary = dict(router.counts, stream={f'0x{k:x}': v for k, v in router.counts['stream'].items()},
                       tracker_failure=repr(getattr(backend, 'failure', None)))
    log.record('summary', **summary)
    log.close()
    print(json.dumps(summary), flush=True)


if __name__ == '__main__':
    main()
