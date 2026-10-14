#!/usr/bin/env python3
"""Differential oracle for src/tobii_linux/sesp.py against the original SESP code.

Runs, in Unicorn (tools/win64_emulator.py), the hash-bound service builders of
ses_windows.dll and the client builders and parser of the game DLL:

1. every service message sesp.py encodes is compared byte for byte with the
   original builder's output for the same inputs;
2. the game DLL's sesp_parse_message (verifier and dispatch) decodes each of
   sesp.py's messages, and the decoded record is checked field by field;
3. the game DLL's request builders produce requests that sesp.py decodes;
4. the game DLL's legacy-module position transform agrees with
   sesp_service.LegacyPositionTransform, and undoes its inverse.

No Windows loader, device, pipe or game runs. Requires Unicorn
(PYTHONPATH=.local/python). Writes evidence/private/sesp-oracle.json.
"""
import json
from pathlib import Path
import struct
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
from win64_emulator import Win64Emulator  # noqa: E402
from tobii_linux import sesp  # noqa: E402

ROOT = Path(__file__).resolve().parents[1]
GAME = (ROOT/'evidence/private/binaries/tobii_gameintegration_x64.dll',
        '35a071073c7498fd917b7e29292299f8b3214fe5a7a8cf5d62610d0331146020')
SERVICE = (ROOT/'evidence/private/binaries/ses_windows.dll',
           '2f7ee110e784aaad75df09cb6a71f35fd5875c445a72ec3004e83e5712f6ff6e')

GAME_CONTEXT_CREATE = 0x180087530
GAME_PARSE = 0x18008E8A0
GAME_REQUEST_INITIALIZE = 0x1800882D0
GAME_REQUEST_HEADPOSE_START = 0x180087FD0
GAME_REQUEST_HEADPOSE_STOP = 0x180088150
GAME_REQUEST_NOTIFY_STREAM_STATUS = 0x180088840
SERVICE_CONTEXT_CREATE = 0x1800B71D0
SERVICE_RESPONSE_INITIALIZE = 0x1800B91B0
SERVICE_RESPONSE_HEADPOSE_START = 0x1800B8E50
SERVICE_RESPONSE_HEADPOSE_STOP = 0x1800B9000
SERVICE_SIGNAL_INVALID_OPERATION = 0x1800BAA20
SERVICE_RESPONSE_NOTIFY_STREAM_STATUS = 0x1800B9D50
SERVICE_SUBSCRIPTION_HEADPOSE = 0x1800BB490
SERVICE_RESPONSE_SEND_STATISTICS = 0x1800BA1B0
GAME_DISPLAY_TRANSFORM = 0x18004AC80
GAME_TRANSFORM_POSITION = 0x180048D30
# Initialize response: the service's capability struct (15 u32) feeds fields in this order.
SERVICE_CAPABILITY_FIELDS = (1, 2, 15, 4, 5, 3, 6, 7, 8, 9, 10, 11, 12, 13, 14)
DECODED_RECORD_SIZE = 0x40


class Codec:
    """One original SESP context with a capturing send callback."""

    def __init__(self, binary, create):
        self.emulator = Win64Emulator(*binary)
        self.sent = []
        logger = self.emulator.put(struct.pack('<4Q', self.emulator.callback(self._log), 0, 0, 0))
        out = self.emulator.allocate(8)
        status = self.emulator.call(create, out, self.emulator.allocate(0x200), 0x200, 0, logger)
        if status:
            raise RuntimeError(f'context create failed: {status}')
        self.context = struct.unpack('<Q', self.emulator.read(out, 8))[0]
        self.send = self.emulator.callback(
            lambda args: self.sent.append(self.emulator.read(args[0], args[1])))

    def _log(self, args):
        raise RuntimeError('original code logged an error')

    def build(self, entry, *args):
        before = len(self.sent)
        status = self.emulator.call(entry, self.context, *args)
        if status or len(self.sent) != before + 1:
            raise RuntimeError(f'{entry:#x} failed: {status}')
        return self.sent[-1]


def service_cases(service):
    e = service.emulator
    cases = []

    def compare(name, original, ours):
        cases.append({'case': name, 'match': original == ours, 'bytes': ours.hex(),
                      **({} if original == ours else {'original': original.hex()})})

    for sequence, error, flags in ((1, 0, {1}), (2, 0, {1, 3}), (7, 0, set(range(1, 16))),
                                   (0x01020304, 5, {1, 15}), (3, 9, set()), (0, 0, {2, 9})):
        struct_words = [1 if field in flags else 0 for field in SERVICE_CAPABILITY_FIELDS]
        original = service.build(SERVICE_RESPONSE_INITIALIZE, sequence, error,
                                 e.put(struct.pack('<15I', *struct_words)), service.send, 0)
        compare(f'initialize response {sequence} error {error} flags {sorted(flags)}', original,
                sesp.encode_initialize_response(sequence, error, flags))
    for entry, kind in ((SERVICE_RESPONSE_HEADPOSE_START, sesp.RESPONSE_HEADPOSE_START),
                        (SERVICE_RESPONSE_HEADPOSE_STOP, sesp.RESPONSE_HEADPOSE_STOP),
                        (SERVICE_RESPONSE_SEND_STATISTICS, sesp.RESPONSE_SEND_STATISTICS),
                        (SERVICE_SIGNAL_INVALID_OPERATION, sesp.SIGNAL_INVALID_OPERATION)):
        for sequence, error in ((3, 0), (4, 6), (0, 7), (0xFFFFFFFF, 12)):
            compare(f'type {kind} sequence {sequence} error {error}',
                    service.build(entry, sequence, error, service.send, 0),
                    sesp.encode_error_response(sequence, kind, error))
    for sequence in (1, 0, 99):
        compare(f'stream status response {sequence}',
                service.build(SERVICE_RESPONSE_NOTIFY_STREAM_STATUS, sequence, service.send, 0),
                sesp.encode_stream_status_response(sequence))
    poses = (
        sesp.HeadPose(0x0000000112345678, (1.5, -2.0, 600.0), (0.1, -0.2, 0.3)),
        sesp.HeadPose(123456789012, (0.0, 0.0, 0.0), (0.0, 0.0, 0.0)),
        sesp.HeadPose(-5, (12.25, 160.5, 540.0), (-0.5, 0.0, 3.0), 0, (0, 1, 0)),
        sesp.HeadPose(987654321, (-0.0, 1e-7, -800.0), (0.25, 0.75, -0.125), 0, (0, 0, 0)),
        sesp.HeadPose(2**62, (3.0, 2.0, 1.0), (0.0, 0.0, 1.0), 1, (1, 0, 1)),
    )
    for sequence, pose in enumerate(poses, start=1):
        original = service.build(
            SERVICE_SUBSCRIPTION_HEADPOSE, sequence, pose.timestamp_us,
            e.put(struct.pack('<3f', *pose.position)), e.put(struct.pack('<3f', *pose.rotation)),
            pose.position_validity, e.put(struct.pack('<3I', *pose.rotation_validity)),
            service.send, 0)
        compare(f'head pose {sequence}', original, sesp.encode_head_pose(sequence, pose))
    return cases


def parse_with_game(game, message):
    """The decoded record the game DLL's parser hands to its dispatch callback."""
    e = game.emulator
    records = []
    callback = e.callback(lambda args: records.append(e.read(args[0], DECODED_RECORD_SIZE)))
    data = e.put(message)
    status = e.call(GAME_PARSE, game.context, data, len(message), callback, 0)
    if status or len(records) != 1:
        raise RuntimeError(f'game parser returned {status} with {len(records)} records')
    return records[0]


def client_cases(game):
    cases = []

    def check(name, record, expected):
        words = struct.unpack('<16I', record)
        floats = struct.unpack('<16f', record)
        got = {key: (floats if kind == 'f' else words)[index]
               for key, (index, kind, _) in expected.items()}
        want = {key: value for key, (_, _, value) in expected.items()}
        cases.append({'case': name, 'match': all(
            got[k] == want[k] or (isinstance(want[k], float) and abs(got[k] - want[k]) < 1e-6)
            for k in want), 'decoded': got, 'expected': want})

    for flags in ({1}, {1, 3}, set(range(1, 16))):
        record = parse_with_game(game, sesp.encode_initialize_response(5, 0, flags))
        check(f'game parses initialize flags {sorted(flags)}', record, {
            'sequence': (0, 'u', 5), 'type': (1, 'u', 3), 'error': (2, 'u', 0),
            'head_pose': (3, 'u', int(1 in flags)), 'display_info': (4, 'u', int(2 in flags)),
            'stream_status': (8, 'u', int(3 in flags)), 'device_list': (12, 'u', int(9 in flags))})
    pose = sesp.HeadPose(0x123456789A, (10.5, -20.25, 640.0), (0.125, -0.25, 0.5), 0, (1, 0, 1))
    record = parse_with_game(game, sesp.encode_head_pose(9, pose))
    check('game parses head pose', record, {
        'sequence': (0, 'u', 9), 'type': (1, 'u', 8), 'time_low': (2, 'u', 0x3456789A),
        'time_high': (3, 'u', 0x12), 'x': (4, 'f', 10.5), 'y': (5, 'f', -20.25),
        'z': (6, 'f', 640.0), 'rx': (7, 'f', 0.125), 'ry': (8, 'f', -0.25), 'rz': (9, 'f', 0.5),
        'position_validity': (10, 'u', 0), 'rx_validity': (11, 'u', 1),
        'ry_validity': (12, 'u', 0), 'rz_validity': (13, 'u', 1)})
    record = parse_with_game(game, sesp.encode_head_pose(1, sesp.HeadPose(7, (0, 0, 0), (0, 0, 0))))
    check('game parses default validities', record, {
        'position_validity': (10, 'u', 1), 'rx_validity': (11, 'u', 1), 'rz_validity': (13, 'u', 1)})
    for kind in (sesp.RESPONSE_HEADPOSE_START, sesp.RESPONSE_HEADPOSE_STOP):
        record = parse_with_game(game, sesp.encode_error_response(11, kind, 6))
        check(f'game parses type {kind}', record,
              {'sequence': (0, 'u', 11), 'type': (1, 'u', kind), 'error': (2, 'u', 6)})
    record = parse_with_game(game, sesp.encode_stream_status_response(12))
    check('game parses stream status response', record, {'sequence': (0, 'u', 12)})
    return cases


def request_cases(game):
    e = game.emulator
    cases = []
    url = 'tet-tcp://127.0.0.1'
    for sequence, subscription in ((1, 0), (2, 3), (0x7FFFFFFF, 0x10000)):
        sent = game.build(GAME_REQUEST_INITIALIZE, sequence, e.put(url.encode() + b'\0'),
                          subscription, game.send, 0)
        message = sesp.decode(sesp.FrameDecoder().feed(sent)[0])
        request = sesp.initialize_request(message)
        cases.append({'case': f'initialize request {sequence}/{subscription}', 'bytes': sent.hex(),
                      'match': (message.sequence, request.device_url, request.subscription_id)
                      == (sequence, url, subscription)})
    for entry, kind in ((GAME_REQUEST_HEADPOSE_START, sesp.REQUEST_HEADPOSE_START),
                        (GAME_REQUEST_HEADPOSE_STOP, sesp.REQUEST_HEADPOSE_STOP)):
        sent = game.build(entry, 4, game.send, 0)
        message = sesp.decode(sesp.FrameDecoder().feed(sent)[0])
        cases.append({'case': f'request type {kind}', 'bytes': sent.hex(),
                      'match': (message.sequence, message.kind) == (4, kind)})
    sent = game.build(GAME_REQUEST_NOTIFY_STREAM_STATUS, 5, e.put(struct.pack('<I', 13)), 1, 0, 0,
                      game.send, 0)
    message = sesp.decode(sesp.FrameDecoder().feed(sent)[0])
    cases.append({'case': 'notify stream status request', 'bytes': sent.hex(),
                  'match': (message.sequence, message.kind) == (5, sesp.REQUEST_NOTIFY_STREAM_STATUS)})
    return cases


DISPLAY_AREAS = (
    ((-296.6200866699219, 326.0040588378906, 111.8187484741211),
     (300.3799133300781, 326.0040588378906, 111.8187484741211),
     (-296.6200866699219, 10.267330169677734, -3.100020170211792)),
    ((-265.0, 320.0, 30.0), (265.0, 320.0, 30.0), (-265.0, 20.0, 0.0)),
    ((-350.0, 400.0, 0.0), (350.0, 400.0, 0.0), (-350.0, 10.0, 0.0)),
    ((-200.0, 250.0, 150.0), (240.0, 250.0, 150.0), (-200.0, 30.0, 20.0)),
)
POSITIONS = ((0.0, 0.0, 0.0), (0.0, 0.0, 600.0), (25.0, -40.0, 550.0), (-120.5, 80.25, 720.0))


def legacy_position_cases(game):
    """The game DLL's own transform (0x18004ac80, 0x180048d30) against sesp_service's model."""
    from tobii_linux.head_pose_pipeline import DisplayArea
    from tobii_linux.sesp_service import LegacyPositionTransform
    e = game.emulator
    cases = []
    for corners in DISPLAY_AREAS:
        area_address = e.put(struct.pack('<9f', *(v for corner in corners for v in corner)))
        matrix, angle = e.allocate(64), e.allocate(4)
        e.call(GAME_DISPLAY_TRANSFORM, area_address, matrix, angle)
        transform = LegacyPositionTransform.from_display_area(DisplayArea(*corners))

        def module(position):
            out = e.allocate(12)
            e.call(GAME_TRANSFORM_POSITION, matrix, e.put(struct.pack('<3f', *position)), out)
            return struct.unpack('<3f', e.read(out, 12))

        for position in POSITIONS:
            original = module(position)
            modelled = transform.apply(position)
            restored = module(transform.invert(position))
            cases.append({'case': f'area {corners[0]} position {position}',
                          'module': original, 'model': modelled, 'module_of_inverse': restored,
                          'match': max(abs(a - b) for a, b in zip(original, modelled)) < 1e-3
                          and max(abs(a - b) for a, b in zip(restored, position)) < 1e-3})
    return cases


def main():
    service = Codec(SERVICE, SERVICE_CONTEXT_CREATE)
    game = Codec(GAME, GAME_CONTEXT_CREATE)
    results = {'service_builders': service_cases(service), 'game_parser': client_cases(game),
               'game_requests': request_cases(game), 'legacy_position': legacy_position_cases(game)}
    out = ROOT/'evidence/private/sesp-oracle.json'
    out.write_text(json.dumps(results, indent=1) + '\n')
    total = failed = 0
    for group, cases in results.items():
        for case in cases:
            total += 1
            if not case['match']:
                failed += 1
                print('MISMATCH', group, case['case'], json.dumps(case)[:400])
    print(f'{total - failed}/{total} cases match; {out}')
    return 1 if failed else 0


if __name__ == '__main__':
    sys.exit(main())
