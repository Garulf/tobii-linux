#!/usr/bin/env python3
"""Run user presence through the original runtime and game DLL code and compare with the Linux service.

Chain under test, original machine code in Unicorn:

1. TTP parser and the stream dispatcher 0x1402c2700 (index 5, parse_presence
   0x1402cba80 inside the parser) on raw 0x504 messages, delivering to
   receive_presence_data 0x140266b60 with a recording subscriber;
2. platmod_stream_user_presence_subscribe 0x140276dd0, with the device stream
   subscribe 0x140273660 and tracker_timestamp_get 0x1402b3860 stubbed: the
   unknown record it emits for a new subscription;
3. the service callback 0x1400fd220 with its real property cache 0x14010e930 on a
   sequence of records, queue allocation, push and free replaced by recorders:
   which records become operation 2 notifications;
4. the game DLL's PRP serializer 0x18004f730 on each queued notification record.

The native counterparts are gaze.presence_message, prp_service.presence_subscribe_time,
prp_service.UserPresence and prp_session.encode_property_notification.
"""
import json
from pathlib import Path
import struct

from unicorn import UC_HOOK_CODE
from unicorn.x86_const import UC_X86_REG_RCX, UC_X86_REG_RDX

import gaze_oracle as runtime
from tobii_linux import gaze, prp_service, streams
from tobii_linux.ttp import Message
from tobii_linux.ttp_commands import STREAM_DATA_KIND, Stream, wire_stream_id

ROOT = Path(__file__).resolve().parents[1]
DISPATCH_STREAMS, RECEIVE_PRESENCE = 0x1402C2700, 0x140266B60
SUBSCRIBE_PRESENCE = 0x140276DD0
GENERAL_STREAM_SUBSCRIBE, TRACKER_TIMESTAMP_GET = 0x140273660, 0x1402B3860
LOCK, UNLOCK = 0x1403B3C90, 0x1403B3CA0
SERVICE_PRESENCE, QUEUE_FREE = 0x1400FD220, 0x14010DD10
PRESENCE_SLOT = 0x113C8
HANDLE_BASE = 0x65432100
CAPTURED_PAYLOAD = bytes.fromhex(
    '0000050000000400020bb8050000000400020bb90200000004000000010600000008000000d18a5be71e'
    '050000000400020bb9020000000400000002010000000400000002')


class RepeatedColumns(list):
    """Column pairs in order, repeats allowed, for gaze.columns_payload."""

    def items(self):
        return list(self)


def presence_message(columns):
    return Message(STREAM_DATA_KIND, 0, 1, wire_stream_id(Stream.PRESENCE), 0,
                   gaze.columns_payload(columns))


def messages():
    timestamp = gaze.int64_column(900_000_123_456)
    cases = {'captured_linux_person_present': Message(STREAM_DATA_KIND, 0, 1,
                                                      wire_stream_id(Stream.PRESENCE), 0,
                                                      CAPTURED_PAYLOAD)}
    for value in (0, 1, 2, 3, -1):
        cases[f'int32_{value}'] = presence_message({1: timestamp, 2: gaze.int32_column(value)})
    cases.update({
        'uint32_value_dropped': presence_message({1: timestamp, 2: gaze.uint32_column(2)}),
        'uint32_timestamp_dropped': presence_message({1: gaze.uint32_column(5),
                                                      2: gaze.int32_column(1)}),
        'value_missing': presence_message({1: timestamp}),
        'timestamp_missing': presence_message({2: gaze.int32_column(1)}),
        'unknown_columns_skipped': presence_message({3: gaze.uint32_column(9), 1: timestamp,
                                                     7: gaze.vec3_column((1.0, 2.0, 3.0)),
                                                     2: gaze.int32_column(1)}),
        'repeated_value_overwrites': presence_message(RepeatedColumns(
            [(1, timestamp), (2, gaze.int32_column(1)), (2, gaze.int32_column(2))])),
    })
    return cases


def original_dispatch(binary, message):
    """(timestamp, platmod value) recorded from receive_presence_data, or None when dropped."""
    machine = runtime.RuntimeMachine(binary)
    machine.add_recorder(runtime.CALLBACK_PAGE + 0x100, 'presence', 0x10)
    parsed = machine.parse(message.encode())
    if parsed is None:
        return {'parser_status': machine.parser_status}
    device = machine.device(False)
    machine.vm.mem_write(device + PRESENCE_SLOT, struct.pack('<Q', runtime.CALLBACK_PAGE + 0x100))
    config = machine.allocate(0x1B0)
    machine.vm.mem_write(config, bytes(0x1B0))
    machine.vm.mem_write(config + 0x108, struct.pack('<Q', device))
    machine.vm.mem_write(config + 0x120, struct.pack('<Q', RECEIVE_PRESENCE))
    machine.call(DISPATCH_STREAMS, parsed, config)
    record = machine.records.get('presence')
    return None if record is None else struct.unpack_from('<qi', record)


def native_dispatch(message):
    item = streams.presence(message)
    return None if item is None else (item.timestamp_us, gaze.platmod_presence(item.value))


class SubscribeMachine(runtime.RuntimeMachine):
    """platmod_stream_user_presence_subscribe with the device-side calls stubbed."""

    def __init__(self, binary, tracker_time, timestamp_status=0):
        super().__init__(binary)
        self.tracker_time, self.timestamp_status = tracker_time, timestamp_status
        self.add_recorder(runtime.CALLBACK_PAGE + 0x200, 'initial', 0x10)
        self.vm.hook_add(UC_HOOK_CODE, self.stub, begin=GENERAL_STREAM_SUBSCRIBE,
                         end=GENERAL_STREAM_SUBSCRIBE)
        self.vm.hook_add(UC_HOOK_CODE, self.stub, begin=TRACKER_TIMESTAMP_GET,
                         end=TRACKER_TIMESTAMP_GET)
        for address in (LOCK, UNLOCK):
            self.vm.hook_add(UC_HOOK_CODE, self.stub, begin=address, end=address)

    def stub(self, vm, address, size, _):
        if address == TRACKER_TIMESTAMP_GET:
            vm.mem_write(vm.reg_read(UC_X86_REG_RDX), struct.pack('<q', self.tracker_time))
            self.ret(self.timestamp_status)
        else:
            self.ret(0)


def original_subscribe(binary, tracker_time, connected=True, received=False, timestamp_status=0):
    machine = SubscribeMachine(binary, tracker_time, timestamp_status)
    device = machine.allocate(runtime.DEVICE_SIZE)
    machine.vm.mem_write(device, bytes(runtime.DEVICE_SIZE))
    machine.vm.mem_write(device + 8, struct.pack('<Q', machine.allocate(0x200)))
    machine.vm.mem_write(device + 0x10F10, struct.pack('<Q', machine.allocate(0x100)))
    machine.vm.mem_write(device + 0x10F3A, bytes([int(connected)]))
    machine.vm.mem_write(device + 0x113D8, bytes([int(received)]))
    table = machine.allocate(0x800)
    machine.vm.mem_write(table, bytes(0x800))
    index, generation = 1, 7
    machine.vm.mem_write(table + (index * 3 + 0x71) * 8, struct.pack('<iiQQ', generation, 0, device, 0))
    handle = generation << 32 | HANDLE_BASE | index
    machine.call(SUBSCRIBE_PRESENCE, table, handle, runtime.CALLBACK_PAGE + 0x200, 0)
    record = machine.records.get('initial')
    return None if record is None else struct.unpack_from('<qi', record)


def native_subscribe(tracker_time, connected=True, received=False, timestamp_status=0):
    if received:
        return None
    if not connected or timestamp_status:
        return (0, gaze.PLATMOD_UNKNOWN)
    return (prp_service.presence_subscribe_time(tracker_time), gaze.PLATMOD_UNKNOWN)


class ServiceMachine(runtime.RuntimeMachine):
    def __init__(self, binary):
        super().__init__(binary)
        self.freed = 0
        self.vm.hook_add(UC_HOOK_CODE, self.free, begin=QUEUE_FREE, end=QUEUE_FREE)

    def free(self, vm, address, size, _):
        self.freed += 1
        self.ret()


def original_service(binary, records):
    """For each platmod record: the queued property record, or None when it was only cached."""
    machine = ServiceMachine(binary)
    service = machine.service()
    queued = []
    for timestamp, value in records:
        before = len(machine.queued)
        machine.call(SERVICE_PRESENCE, machine.put(struct.pack('<qi4x', timestamp, value)), service)
        queued.append(machine.queued[-1] if len(machine.queued) > before else None)
    return queued


def native_service(records):
    presence = prp_service.UserPresence()
    results = []
    for timestamp, value in records:
        pushed = presence.record(timestamp, value)
        results.append((presence.value, presence.since_us, presence.notification()) if pushed else None)
    return results


def compare_service(runtime_binary, game_dll, name, records):
    expected = original_service(runtime_binary, records)
    native = native_service(records)
    steps, ok = [], True
    for (timestamp, value), want, got in zip(records, expected, native):
        step = {'timestamp': timestamp, 'platmod_value': value, 'notified': want is not None}
        same = (want is None) == (got is None)
        if want is not None and got is not None:
            operation, property_id = struct.unpack_from('<II', want, 4)
            when, prp_value = struct.unpack_from('<qI', want, 0x10)
            packet = runtime.original_packet(game_dll, operation, property_id,
                                             struct.pack('<qI4x', when, prp_value), 0)
            step.update(operation=operation, property=property_id, prp_value=prp_value,
                        packet=packet.hex() if packet else None)
            same &= (operation, property_id, when, prp_value) == (
                2, prp_service.PROPERTY_USER_PRESENCE, got[1], got[0]) and packet == got[2]
        step['match'] = same
        ok &= same
        steps.append(step)
    print(name, [(s['platmod_value'], s['notified']) for s in steps], 'ok' if ok else 'FAIL')
    return {'steps': steps, 'match': ok}


def main():
    binary = runtime.RUNTIME.read_bytes()
    game_dll = runtime.GAME_DLL.read_bytes()
    report = {'kind': 'supplemental-emulated-original-functions',
              'functions': {'dispatcher': hex(DISPATCH_STREAMS), 'parse_presence': '0x1402cba80',
                            'receive_presence_data': hex(RECEIVE_PRESENCE),
                            'user_presence_subscribe': hex(SUBSCRIBE_PRESENCE),
                            'service_presence_callback': hex(SERVICE_PRESENCE),
                            'property_cache': '0x14010e930',
                            'packet_serializer': '0x18004f730 (game DLL)'},
              'messages': {}, 'subscribe': {}, 'service': {}}
    passed = True
    for name, message in messages().items():
        want, got = original_dispatch(binary, message), native_dispatch(message)
        if isinstance(want, dict):
            same = got is None
        else:
            same = want == got
        report['messages'][name] = {'original': want, 'native': got, 'match': same}
        passed &= same
        print(name, want, got, 'ok' if same else 'FAIL')
    subscribe_cases = {
        'new_subscription': dict(tracker_time=900_000_000_000),
        'tracker_time_below_lead': dict(tracker_time=12_345),
        'data_already_received': dict(tracker_time=900_000_000_000, received=True),
        'device_not_connected': dict(tracker_time=900_000_000_000, connected=False),
        'timestamp_get_failed': dict(tracker_time=900_000_000_000, timestamp_status=7),
    }
    for name, case in subscribe_cases.items():
        want, got = original_subscribe(binary, **case), native_subscribe(**case)
        same = want == got
        report['subscribe'][name] = {'original': want, 'native': got, 'match': same}
        passed &= same
        print('subscribe', name, want, got, 'ok' if same else 'FAIL')
    sequences = {
        'subscribe_then_changes': [(1000, 0), (1500, 2), (9000, 1), (12000, 2)],
        'repeated_value_still_pushed': [(1000, 1), (2000, 1), (3000, 1)],
        'unknown_after_present': [(5, 2), (6, 0)],
    }
    for name, records in sequences.items():
        case = compare_service(binary, game_dll, name, records)
        report['service'][name] = case
        passed &= case['match']
    report['passed'] = bool(passed)
    (ROOT/'evidence/private/presence-oracle.json').write_text(json.dumps(report, indent=2))
    print('PASS' if passed else 'FAIL')
    raise SystemExit(0 if passed else 1)


if __name__ == '__main__':
    main()
