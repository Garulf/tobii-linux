#!/usr/bin/env python3
"""Run recorded gaze stream messages through the original runtime and game DLL code.

Chain under test, every step original machine code in Unicorn:

1. TTP parser 0x1402cfa50/0x1402cf8c0/0x1402cfde0 on the raw 0x500 message;
2. gaze column dispatcher 0x1402c1c30 and receive_gaze_data 0x140265d70, whose
   subscriber callbacks are replaced by recorders (the platmod records);
3. the service's stream callbacks (0x1400fc170 gaze point, 0x1400fcdd0
   responsive gaze point, 0x1400fc040 gaze origin, 0x1400faf30 gaze data,
   0x1400fae20 eye openness, 0x1400fd0f0 user position guide) with the queue
   allocation, enqueue and wake-up calls replaced by recorders (the PRP records);
4. the game DLL's PRP packet serializer 0x18004f730 on those records.

The native counterpart is tobii_linux.gaze; every step is compared byte for byte.
"""
import json
from pathlib import Path
import struct
import sys

from unicorn import UC_HOOK_CODE, UcError
from unicorn.x86_const import UC_X86_REG_RAX, UC_X86_REG_RCX, UC_X86_REG_RDX, UC_X86_REG_RSP, UC_X86_REG_RIP

from ttp_reply_oracle import Machine, PARSER_BLOCK, PARSER_FEED, PARSER_INIT, PARSER_NEXT
import prp_encoder_oracle as dll
from tobii_linux import gaze
from tobii_linux.ttp import Message

ROOT = Path(__file__).resolve().parents[1]
RUNTIME = ROOT/'evidence/private/binaries/platform_runtime_IS5LEYETRACKER5_service.exe'
GAME_DLL = ROOT/'evidence/private/binaries/tobii_gameintegration_x64.dll'
DISPATCH, RECEIVE = 0x1402C1C30, 0x140265D70
DEVICE_SIZE, SERVICE_SIZE = 0x11600, 0x360300
CALLBACK_PAGE = 0x63000000
QUEUE_ALLOCATE, QUEUE_PUSH, QUEUE_WAKE = 0x14010DB20, 0x14010E1A0, 0x1403B3CB0
GAZE_DATA_CONVERT = 0x1400F9F00

PLATMOD_SLOTS = {  # device offset of the subscriber callback, record size
    'gaze_point': (0x11388, 0x14),
    'responsive_gaze_point': (0x11398, 0x14),
    'eye_openness': (0x114E0, 0x18),
    'gaze_origin': (0x113A8, 0x28),
    'gaze_data': (0x113B8, 0xA8),
    'user_position_guide': (0x11440, 0x28),
}
SERVICE_CALLBACKS = {
    'gaze_point': 0x1400FC170,
    'responsive_gaze_point': 0x1400FCDD0,
    'gaze_origin': 0x1400FC040,
    'gaze_data': 0x1400FAF30,
    'eye_openness': 0x1400FAE20,
    'user_position_guide': 0x1400FD0F0,
}
PRP_RECORD_SIZE = 0xC0


class RuntimeMachine(Machine):
    def __init__(self, binary):
        super().__init__(binary)
        self.vm.mem_map(CALLBACK_PAGE, 0x1000)
        self.records = {}
        self.callbacks = {}
        self.sizes = {name: size for name, (_, size) in PLATMOD_SLOTS.items()}
        for index, name in enumerate(PLATMOD_SLOTS):
            self.add_recorder(CALLBACK_PAGE + 0x10 * index, name)
        for address in (QUEUE_ALLOCATE, QUEUE_PUSH, QUEUE_WAKE):
            self.vm.hook_add(UC_HOOK_CODE, self.queue_stub, begin=address, end=address)
        self.queued = []

    def add_recorder(self, address, name, size=None):
        self.callbacks[address] = name
        if size is not None:
            self.sizes[name] = size
        self.vm.hook_add(UC_HOOK_CODE, self.recorder, begin=address, end=address)

    def ret(self, value=0):
        stack = self.vm.reg_read(UC_X86_REG_RSP)
        self.vm.reg_write(UC_X86_REG_RAX, value)
        self.vm.reg_write(UC_X86_REG_RSP, stack + 8)
        self.vm.reg_write(UC_X86_REG_RIP, struct.unpack('<Q', self.vm.mem_read(stack, 8))[0])

    def recorder(self, vm, address, size, _):
        name = self.callbacks[address]
        self.records[name] = bytes(vm.mem_read(vm.reg_read(UC_X86_REG_RCX), self.sizes[name]))
        self.ret()

    def queue_stub(self, vm, address, size, _):
        if address == QUEUE_ALLOCATE:
            record = self.allocate(PRP_RECORD_SIZE)
            vm.mem_write(record, bytes(PRP_RECORD_SIZE))
            vm.mem_write(vm.reg_read(UC_X86_REG_RDX), struct.pack('<QQ', 0, record))
            self.ret(1)
        elif address == QUEUE_PUSH:
            record = struct.unpack('<Q', vm.mem_read(vm.reg_read(UC_X86_REG_RDX) + 8, 8))[0]
            self.queued.append(bytes(vm.mem_read(record, PRP_RECORD_SIZE)))
            self.ret()
        else:
            self.ret()

    def parse(self, encoded):
        block, holder = self.allocate(PARSER_BLOCK), self.allocate(8)
        self.call(PARSER_INIT, holder, block, PARSER_BLOCK, 0, 0, 0, 0, 0)
        data = self.put(encoded)
        self.call(PARSER_FEED, block, data, len(encoded))
        message = self.allocate(0x20)
        self.call(PARSER_NEXT, block, message)
        self.parser_status = self.result() & 0xFFFFFFFF
        return None if self.parser_status else message

    def device(self, host_gaze):
        device = self.allocate(DEVICE_SIZE)
        self.vm.mem_write(device, bytes(DEVICE_SIZE))
        settings = self.allocate(0x200)
        self.vm.mem_write(settings, bytes(0x200))
        self.vm.mem_write(settings + 0x188, struct.pack('<I', int(host_gaze)))
        self.vm.mem_write(device + 8, struct.pack('<Q', settings))
        for address, name in self.callbacks.items():
            if name in PLATMOD_SLOTS:
                self.vm.mem_write(device + PLATMOD_SLOTS[name][0], struct.pack('<Q', address))
        return device

    def service(self):
        service = self.allocate(SERVICE_SIZE)
        wake = self.allocate(0x700)
        self.vm.mem_write(service + 0x35FA40, struct.pack('<Q', wake))
        return service


def original_platmod(binary, encoded, host_gaze=False):
    """Platmod records produced for one raw TTP message."""
    machine = RuntimeMachine(binary)
    message = machine.parse(encoded)
    if message is None:
        return {'parser_status': machine.parser_status}
    config = machine.allocate(0x1B0)
    machine.vm.mem_write(config, bytes(0x1B0))
    machine.vm.mem_write(config + 0x108, struct.pack('<QQ', machine.device(host_gaze), RECEIVE))
    machine.call(DISPATCH, message, config)
    return machine.records


def original_prp(binary, name, platmod_record):
    """PRP record the service queues for one platmod record."""
    machine = RuntimeMachine(binary)
    record = machine.put(platmod_record + bytes(0x10))
    machine.call(SERVICE_CALLBACKS[name], record, machine.service())
    if len(machine.queued) != 1:
        raise RuntimeError(f'{name}: expected one queued record, got {len(machine.queued)}')
    return machine.queued[0]


PRESENCE_RECEIVE, PRESENCE_CONVERT, PRESENCE_SLOT = 0x140266B60, 0x1400FA2B0, 0x113C8


def original_presence(binary, timestamp, device_value):
    """Platmod presence record and the property record the service builds from it."""
    machine = RuntimeMachine(binary)
    device = machine.device(False)
    machine.add_recorder(CALLBACK_PAGE + 0x100, 'presence', 0x10)
    machine.vm.mem_write(device + PRESENCE_SLOT, struct.pack('<Q', CALLBACK_PAGE + 0x100))
    record = machine.put(struct.pack('<qI4x', timestamp, device_value))
    machine.call(PRESENCE_RECEIVE, device, record)
    platmod = machine.records['presence']
    slot = machine.allocate(0x10)
    target = machine.allocate(PRP_RECORD_SIZE)
    machine.vm.mem_write(target, bytes(PRP_RECORD_SIZE))
    machine.vm.mem_write(slot, struct.pack('<QQ', 0, target))
    machine.call(PRESENCE_CONVERT, 0, machine.service(), slot, machine.put(platmod))
    return platmod, bytes(machine.vm.mem_read(target, 0x20))


def compare_presence(runtime):
    cases = []
    for device_value in (0, 1, 2, 3, 7):
        platmod, prp = original_presence(runtime, 0x1122334455, device_value)
        native_platmod = gaze.platmod_presence(device_value)
        kind, prop = struct.unpack_from('<II', prp, 4)
        timestamp, value = struct.unpack_from('<qI', prp, 0x10)
        same = (struct.unpack_from('<qI', platmod) == (0x1122334455, native_platmod)
                and (kind, prop, timestamp) == (2, gaze.PROPERTY_USER_PRESENCE, 0x1122334455)
                and value == gaze.prp_presence(native_platmod))
        cases.append({'device_value': device_value, 'platmod_value': struct.unpack_from('<I', platmod, 8)[0],
                      'operation': kind, 'property': prop, 'prp_value': value, 'match': same})
    return cases


def original_packet(game_dll, kind, stream, native, version):
    vm = dll.machine(game_dll)
    source, output, length, logger = dll.DATA, dll.DATA + 0x1000, dll.DATA + 0x2000, dll.DATA + 0x3000
    vm.mem_write(source, struct.pack('<IIII', 0, kind, stream, 0) + native)
    vm.mem_write(logger, struct.pack('<Q', 1))
    rsp = dll.STACK + 0xFEF8
    vm.mem_write(rsp, struct.pack('<Q', dll.STOP))
    vm.mem_write(rsp + 0x28, struct.pack('<QQ', output, length))
    for register, value in ((UC_X86_REG_RCX, logger), (UC_X86_REG_RDX, version),
                            (dll.UC_X86_REG_R8, source), (dll.UC_X86_REG_R9, 4096),
                            (UC_X86_REG_RSP, rsp)):
        vm.reg_write(register, value)
    try:
        vm.emu_start(0x18004F730, dll.STOP, timeout=2_000_000, count=200_000)
    except UcError:
        # Streams without a serializer case reach the logger, which is not mapped.
        return None
    if vm.reg_read(UC_X86_REG_RIP) != dll.STOP:
        raise RuntimeError('Original packet serializer did not return')
    if vm.reg_read(UC_X86_REG_RAX) & 0xFFFFFFFF:
        return None
    size = struct.unpack('<Q', vm.mem_read(length, 8))[0]
    return bytes(vm.mem_read(output, size))


def recorded_messages(path, limit=None):
    messages = [message for _, message in gaze.recorded_gaze_messages(path)]
    return messages if limit is None else messages[:limit]


def selection(messages, count):
    """A spread of messages: every validity pattern seen, then evenly spaced extras."""
    chosen, patterns = [], set()
    for message in messages:
        record = gaze.decode_gaze_message(message.payload)
        pattern = tuple(getattr(record, field) for field in gaze.VALIDITY_FIELDS)
        if pattern not in patterns:
            patterns.add(pattern)
            chosen.append(message)
    step = max(1, len(messages) // max(1, count - len(chosen)))
    for message in messages[::step]:
        if len(chosen) >= count:
            break
        if message not in chosen:
            chosen.append(message)
    return chosen


def compare_message(runtime, game_dll, message, host_gaze=False):
    expected = original_platmod(runtime, message.encode(), host_gaze)
    parser_status = expected.pop('parser_status', 0)
    record = gaze.decode_gaze_message(message.payload)
    native = {} if record is None else gaze.platmod_records(record, host_gaze=host_gaze)
    case = {'timestamp': record.timestamp if record else None, 'host_gaze': host_gaze,
            'original_parser_status': parser_status, 'dropped': not expected,
            'platmod': {}, 'prp': {}, 'packets': {}}
    ok = set(expected) == set(native)
    for name in sorted(set(expected) | set(native)):
        same = expected.get(name) == native.get(name)
        case['platmod'][name] = same
        ok &= same
    for name, platmod_record in expected.items():
        queued = original_prp(runtime, name, platmod_record)
        kind, stream = struct.unpack_from('<II', queued, 4)
        native_prp = gaze.prp_record(name, gaze.platmod_from_bytes(name, platmod_record))
        size = len(native_prp.native)
        same = (kind, stream) == (native_prp.kind, native_prp.stream) and queued[0x10:0x10 + size] == native_prp.native
        case['prp'][name] = {'kind': kind, 'stream': stream, 'match': same}
        ok &= same
        for version in (0, 0x7FFFFFFF):
            packet = original_packet(game_dll, kind, stream, queued[0x10:0x10 + size], version)
            native_packet = gaze.stream_packet(native_prp, extended=bool(version))
            same = packet == native_packet
            case['packets'][f'{name}/{version:#x}'] = (
                'unsupported by the game DLL serializer' if packet is None and native_packet is None
                else same)
            ok &= same
    case['match'] = ok
    return case


def synthetic_messages(template):
    """Edge cases the recording does not cover, built from one recorded message."""
    columns = gaze.message_columns(template.payload)

    def message(update):
        changed = dict(columns)
        changed.update(update)
        return gaze.gaze_stream_message(changed)
    return {
        'validity_2_and_1': message({7: gaze.uint32_column(1), 13: gaze.uint32_column(2),
                                     27: gaze.uint32_column(2), 31: gaze.uint32_column(7),
                                     33: gaze.uint32_column(2), 35: gaze.uint32_column(1),
                                     38: gaze.uint32_column(2), 40: gaze.uint32_column(1),
                                     42: gaze.uint32_column(3)}),
        'position_guide_clamp': message({37: gaze.vec3_column((-0.5, 0.25, 1.75)),
                                         38: gaze.uint32_column(1),
                                         39: gaze.vec3_column((1.0, 0.0, 1.0000001)),
                                         40: gaze.uint32_column(1)}),
        'eyeball_absent': message({23: None, 24: None}),
        'rounding': message({28: gaze.vec2_column((0.1234567891, -0.98765432123)),
                             2: gaze.vec3_column((-123.456789012, 0.000000123, 1e5 / 3)),
                             6: gaze.fixed16_column(3.14159)}),
        'type_mismatch_dropped': message({28: gaze.vec3_column((0.5, 0.5, 0.5))}),
        'column_missing': message({27: None, 28: None}),
        'unknown_column_skipped': message({17: None, 99: gaze.vec3_column((1.0, 2.0, 3.0))}),
        'int32_validity_dropped': message({7: gaze.int32_column(0)}),
    }


def main():
    runtime, game_dll = RUNTIME.read_bytes(), GAME_DLL.read_bytes()
    sources = [ROOT/'evidence/private/gaze-capture/linux-gaze.jsonl',
               ROOT/'evidence/private/gaze-baseline/linux-gaze.jsonl',
               ROOT/'evidence/private/gaze-device/linux-gaze.jsonl',
               ROOT/'evidence/private/gaze-device/linux-gaze-image.jsonl']
    count = int(sys.argv[1]) if len(sys.argv) > 1 else 24
    report = {'kind': 'supplemental-emulated-original-functions',
              'functions': {'parser': [hex(PARSER_INIT), hex(PARSER_FEED), hex(PARSER_NEXT)],
                            'dispatcher': hex(DISPATCH), 'receive_gaze_data': hex(RECEIVE),
                            'service_callbacks': {k: hex(v) for k, v in SERVICE_CALLBACKS.items()},
                            'gaze_data_convert': hex(GAZE_DATA_CONVERT),
                            'packet_serializer': '0x18004f730 (game DLL)'},
              'recorded': [], 'synthetic': {}}
    passed = True
    for source in sources:
        if not source.exists():
            continue
        messages = recorded_messages(source)
        for message in selection(messages, count):
            case = compare_message(runtime, game_dll, message)
            case['source'] = str(source.relative_to(ROOT))
            report['recorded'].append(case)
            passed &= case['match']
        cases = [c for c in report['recorded'] if c['source'] == str(source.relative_to(ROOT))]
        print(source.relative_to(ROOT), sum(c['match'] for c in cases), '/', len(cases))
    template = recorded_messages(sources[0], 1)[0]
    for name, message in synthetic_messages(template).items():
        case = compare_message(runtime, game_dll, message)
        report['synthetic'][name] = case
        passed &= case['match']
        print(name, 'ok' if case['match'] else 'FAIL', 'dropped' if case['dropped'] else sorted(case['platmod']),
              f"parser status {case['original_parser_status']}")
    report['presence'] = compare_presence(runtime)
    passed &= all(case['match'] for case in report['presence'])
    print('presence', [(c['device_value'], c['prp_value'], c['match']) for c in report['presence']])
    host = compare_message(runtime, game_dll, template, host_gaze=True)
    report['synthetic']['host_gaze_mode'] = host
    passed &= host['match']
    print('host_gaze_mode', 'ok' if host['match'] else 'FAIL', sorted(host['platmod']))
    report['passed'] = bool(passed)
    (ROOT/'evidence/private/gaze-oracle.json').write_text(json.dumps(report, indent=2))
    print('PASS' if passed else 'FAIL')
    raise SystemExit(0 if passed else 1)


if __name__ == '__main__':
    main()
