#!/usr/bin/env python3
"""Run the service's tet-tcp route (`cli serve --tet-tcp`) without a tracker.

The PrpService of `cli serve --tet-tcp` listens for the Wine bridge (relay port,
default 7760) and on TCP 127.0.0.1:4455. Instead of a tracker session it gets:

- device replies recorded from the real ET5 (`--device-query`, the getter replies
  of gaze-capture/device-query.jsonl; `--replay`, raw USB logs for protocol
  upgrade and unlock_ppu), so every reply a game sees is the device's own bytes;
- gaze: the recorded 0x500 messages of the raw USB logs, looped at 30 Hz, and
  presence (0x504) records saying "present" once a second, both stamped with
  the service clock as a live session stamps them;
- head pose: the service's synthetic head sweep.

--interrupt-at drops the session once, so a client's recovery can be watched.

Prints a JSON summary after --seconds. Offline only; nothing is sent to a device.
"""
import argparse
import asyncio
import itertools
import json
import logging
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]/'src'))
from tobii_linux import gaze  # noqa: E402
from tobii_linux.prp_service import (HeadPoseHub, PrpService, publish_synthetic,  # noqa: E402
                                     wine_clock)
from tobii_linux.tet_tcp_service import DeviceReplies, GAZE_STREAM, PRESENCE_STREAM  # noqa: E402
from tobii_linux.traffic_log import replay_messages  # noqa: E402
from tobii_linux.ttp import Message  # noqa: E402
from tobii_linux.ttp_commands import Command, RESPONSE_KIND, STREAM_DATA_KIND  # noqa: E402

PRIVATE = Path(__file__).resolve().parents[1]/'evidence/private'
DEFAULT_QUERY = PRIVATE/'gaze-capture/device-query.jsonl'
DEFAULT_REPLAY = PRIVATE/'calibration/probe/host-pattern-fixed-raw.jsonl'
GAZE_PERIOD_S = 1 / 30
PRESENCE_PERIOD_S = 1.0
INTERRUPTION_S = 3.0
PRESENT_DEVICE_VALUE = 2


def recorded(device_query, replay):
    replies, gaze_messages = {}, []
    for line in Path(device_query).read_text().splitlines():
        row = json.loads(line)
        if row.get('kind') == 'reply':
            command = int(Command[row['command']])
            replies[command] = Message(RESPONSE_KIND, 0, 1, command, 0, bytes.fromhex(row['payload']))
    for _, direction, message in replay_messages(replay):
        if isinstance(message, str) or direction != 'in':
            continue
        if message.kind == RESPONSE_KIND and message.command in (Command.PROTOCOL_UPGRADE,
                                                                  Command.UNLOCK_PPU):
            replies.setdefault(message.command, message)
        elif message.kind == STREAM_DATA_KIND and message.command == GAZE_STREAM:
            gaze_messages.append(message)
    return DeviceReplies(replies, frozenset({GAZE_STREAM, PRESENCE_STREAM})), gaze_messages


def presence_message(timestamp_us):
    payload = gaze.columns_payload({1: gaze.int64_column(timestamp_us),
                                    2: gaze.int32_column(PRESENT_DEVICE_VALUE)})
    return Message(STREAM_DATA_KIND, 0, 1, PRESENCE_STREAM, 0, payload)


async def feed_streams(service, clock, gaze_messages):
    loop = asyncio.get_running_loop()
    next_presence = 0.0
    for index in itertools.count():
        now = clock()
        service.publish_device_stream(gaze_messages[index % len(gaze_messages)], now)
        if loop.time() >= next_presence:
            service.publish_device_stream(presence_message(now), now)
            next_presence = loop.time() + PRESENCE_PERIOD_S
        await asyncio.sleep(GAZE_PERIOD_S)


async def run(args):
    clock = wine_clock(args.wine_clock)
    hub = HeadPoseHub()
    service = PrpService(hub, clock=clock, display_area=None, tet_tcp=True,
                         compensate_sesp_position=args.sesp_positions == 'prp')
    device, gaze_messages = recorded(args.device_query, args.replay)
    server = await service.serve(args.host, args.port)
    service.attach_tracker(0)
    service.attach_device(device)
    tasks = [asyncio.create_task(publish_synthetic(hub, clock=clock)),
             asyncio.create_task(feed_streams(service, clock, gaze_messages))]
    logging.info('tet-tcp emulation: relay %s:%d, pipes %s, %d device replies, %d gaze messages',
                 args.host, args.port, service.pipe_names, len(device.replies), len(gaze_messages))
    if args.interrupt_at is not None:
        await asyncio.sleep(args.interrupt_at)
        logging.info('interrupting the tracker session for %.1f s', INTERRUPTION_S)
        service.detach_tracker()
        await asyncio.sleep(INTERRUPTION_S)
        service.attach_tracker(0)
        service.attach_device(device)
        await asyncio.sleep(args.seconds - args.interrupt_at - INTERRUPTION_S)
    else:
        await asyncio.sleep(args.seconds)
    for task in tasks:
        task.cancel()
    summary = {'sesp_head_poses_sent': service.sesp.sent, 'sesp_dropped': service.sesp.dropped,
               'tet_tcp_stream_messages_sent': service.tet_tcp.sent,
               'tet_tcp_dropped': service.tet_tcp.dropped,
               'clock': service.clock.source, 'game_time_offset_us': service.game_time.offset_us()}
    await service.close(server)
    print(json.dumps(summary), flush=True)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--host', default='127.0.0.1')
    parser.add_argument('--port', type=int, default=7760)
    parser.add_argument('--device-query', type=Path, default=DEFAULT_QUERY)
    parser.add_argument('--replay', type=Path, default=DEFAULT_REPLAY)
    parser.add_argument('--seconds', type=float, default=60.0)
    parser.add_argument('--wine-clock', default='auto')
    parser.add_argument('--sesp-positions', choices=('prp', 'windows'), default='prp')
    parser.add_argument('--interrupt-at', type=float, metavar='SECONDS',
                        help='end the tracker session after this long and start it again '
                             f'{INTERRUPTION_S:g} s later, as an unplugged tracker does')
    parser.add_argument('--debug', action='store_true')
    args = parser.parse_args(argv)
    logging.basicConfig(level=logging.DEBUG if args.debug else logging.INFO,
                        format='%(asctime)s %(name)s %(message)s')
    asyncio.run(run(args))


if __name__ == '__main__':
    main()
