"""Native endpoint for the game DLL's tet-tcp:// tracker route.

The game DLL's discovery (0x180058010) reports `tet-tcp://127.0.0.1` when a TCP
connect to 127.0.0.1:4455 succeeds. device_create then builds its in-process
legacy TTP platform module, whose tracker client opens channel 1 on port 4455
and speaks plain TTP framing, the same framing `ttp.py` decodes, with no USB
envelope. Timesync uses a UDP socket to port 4457 (channel 3), and channel 4 is
TCP 4459 (docs/tet-tcp.md).

TetTcpServer accepts those connections. Channel 1 traffic goes through a Router
that relays TTP requests to a tracker backend, remapping request ids, and sends
responses and subscribed stream data back. Every byte in either direction is
written to a RawLog. Requests outside READ_COMMANDS are never forwarded: the
router answers them with an error and logs them as blocked, so the DLL cannot
write calibration, display area, settings or files to the device. The policy is
implemented in crates/tobii-service/src/tet_tcp.rs; the sockets, threads and router stay
here, since the router calls Python backends and connections.
"""
from dataclasses import dataclass, field
import itertools
import json
import socket
import threading
import time

from ._native import tet_tcp as _tet_tcp
from .ttp import Decoder, Message, ProtocolError, fields, protocol_upgrade
from .ttp_commands import (Command, NOTIFICATION_KIND, REQUEST_KIND, RESPONSE_KIND,
                           STREAM_DATA_KIND, header_only_request, open_realm_response,
                           open_realm_salt, realm_digest, request, uint32_field)
from .ttp_fields import value

COMMAND_PORT = _tet_tcp.COMMAND_PORT
CHANNEL_PORTS = dict(_tet_tcp.CHANNEL_PORTS)
TIMESYNC_UDP_PORT = _tet_tcp.TIMESYNC_UDP_PORT
SERVICE_UNLOCK_KEY = _tet_tcp.SERVICE_UNLOCK_KEY
UNSUPPORTED_OPERATION = _tet_tcp.UNSUPPORTED_OPERATION
INTERNAL_TIMEOUT_S = 3.0
DISPLAY_AREA_WRITABLE = _tet_tcp.DISPLAY_AREA_WRITABLE
PERSISTENT_FILE_ENUMERATE = _tet_tcp.PERSISTENT_FILE_ENUMERATE
PERSISTENT_FILE_READ = _tet_tcp.PERSISTENT_FILE_READ

READ_COMMANDS = frozenset(_tet_tcp.READ_COMMANDS)


def describe(message):
    return {'kind': message.kind, 'rid': message.request_id, 'word2': message.word2,
            'command': message.command, 'word4': message.word4, 'length': len(message.payload)}


class RawLog:
    """JSON lines: host time, connection, port, direction, raw hex and the decoded header."""
    def __init__(self, path=None, stream=None):
        self.file = open(path, 'w', buffering=1) if path else stream
        self.start = time.monotonic()
        self.lock = threading.Lock()

    def record(self, event, **values):
        if self.file is None:
            return
        line = json.dumps({'t': round(time.monotonic() - self.start, 6), 'event': event, **values})
        with self.lock:
            self.file.write(line + '\n')
            self.file.flush()

    def bytes(self, direction, connection, port, data):
        self.record('bytes', dir=direction, conn=connection, port=port, hex=data.hex())

    def message(self, direction, connection, message, note=None):
        values = describe(message)
        if note:
            values['note'] = note
        self.record('message', dir=direction, conn=connection, payload=message.payload.hex(), **values)

    def close(self):
        if self.file is not None and not self.file.closed:
            self.file.close()


def subscribed_stream(message):
    """Wire stream id of a SUBSCRIBE_STREAM or UNSUBSCRIBE_STREAM request, if readable."""
    return _tet_tcp.subscribed_stream(message)


@dataclass
class Pending:
    connection: object
    request_id: int
    command: int
    done: threading.Event = field(default_factory=threading.Event)
    reply: Message = None


class Router:
    """Relays DLL requests to a tracker backend and routes what the tracker sends back.

The backend needs send(bytes); the backend calls `from_device(message)` for every
TTP message it reads. In `unlock` mode `prepare()` negotiates the protocol and
opens the unlock realm with the service key before any client connects; the
DLL's own protocol upgrade is then answered from the cached reply.
"""
    def __init__(self, backend, log, *, unlock=False, allow=()):
        self.backend = backend
        self.log = log
        self.unlock = unlock
        self.allowed = READ_COMMANDS | frozenset(allow)
        self.ids = itertools.count(0x1000)
        self.pending = {}
        self.subscriptions = {}
        self.upgrade_reply = None
        self.lock = threading.Lock()
        self.counts = {'forwarded': 0, 'blocked': 0, 'responses': 0, 'stream': {}, 'notifications': 0}

    def _send_to_device(self, message, connection, original_id):
        proxy_id = next(self.ids)
        entry = Pending(connection, original_id, message.command)
        with self.lock:
            self.pending[proxy_id] = entry
        outgoing = Message(message.kind, proxy_id, message.word2, message.command, message.word4,
                           message.payload)
        self.log.message('proxy->device', None, outgoing, note=f'for rid {original_id}')
        self.backend.send(outgoing.encode())
        return entry

    def call(self, build, timeout_s=INTERNAL_TIMEOUT_S):
        message = build(0)
        entry = self._send_to_device(message, None, 0)
        if not entry.done.wait(timeout_s):
            raise TimeoutError(f'Tracker did not answer command {message.command}')
        if entry.reply.word4:
            raise RuntimeError(f'Command {message.command} failed with 0x{entry.reply.word4:08x}')
        return entry.reply

    def prepare(self):
        if not self.unlock:
            return
        self.upgrade_reply = self.call(protocol_upgrade)
        unlock = [value(f) for f in fields(self.call(
            lambda rid: header_only_request(rid, Command.UNLOCK_PPU)).payload)]
        integers = [v for v in unlock if isinstance(v, int)]
        self.log.record('unlock_ppu', values=[v if isinstance(v, int) else bytes(v).hex() for v in unlock])
        if len(integers) >= 3 and integers[0] and not integers[2]:
            salt_reply = [value(f) for f in fields(self.call(
                lambda rid: open_realm_salt(rid, integers[1])).payload)]
            numbers = [v for v in salt_reply if isinstance(v, int)]
            salt = next(bytes(v) for v in salt_reply if isinstance(v, (bytes, bytearray)))
            digest = realm_digest(SERVICE_UNLOCK_KEY, salt)
            self.call(lambda rid: open_realm_response(rid, (numbers[0], numbers[1]), digest))
            self.log.record('realm_opened', realm=integers[1])

    def from_client(self, connection, message):
        if message.kind != REQUEST_KIND:
            self.log.message('dll->proxy', connection.id, message, note='not a request, dropped')
            return
        if message.command not in self.allowed:
            self.counts['blocked'] += 1
            self.log.message('dll->proxy', connection.id, message, note='blocked')
            connection.send_message(Message(RESPONSE_KIND, message.request_id, 0, message.command,
                                            UNSUPPORTED_OPERATION))
            return
        if message.command == Command.PROTOCOL_UPGRADE and self.upgrade_reply is not None:
            self.log.message('dll->proxy', connection.id, message, note='answered from cache')
            reply = self.upgrade_reply
            connection.send_message(Message(reply.kind, message.request_id, reply.word2,
                                            reply.command, reply.word4, reply.payload))
            return
        if message.command in (Command.SUBSCRIBE_STREAM, Command.UNSUBSCRIBE_STREAM):
            if self._shared_subscription(connection, message):
                self.log.message('dll->proxy', connection.id, message, note='shared stream, answered locally')
                connection.send_message(Message(RESPONSE_KIND, message.request_id, 0, message.command, 0))
                return
        self.counts['forwarded'] += 1
        self.log.message('dll->proxy', connection.id, message, note='forwarded')
        self._send_to_device(message, connection, message.request_id)

    def _shared_subscription(self, connection, message):
        """Record the change; true when another connection keeps the device's subscription as it is."""
        stream = subscribed_stream(message)
        if stream is None:
            return False
        with self.lock:
            others = any(stream in streams for c, streams in self.subscriptions.items() if c is not connection)
            streams = self.subscriptions.setdefault(connection, set())
            if message.command == Command.SUBSCRIBE_STREAM:
                streams.add(stream)
            else:
                streams.discard(stream)
        return others

    def from_device(self, message):
        if message.kind == RESPONSE_KIND:
            with self.lock:
                entry = self.pending.pop(message.request_id, None)
            if entry is None:
                self.log.message('device->proxy', None, message, note='no pending request')
                return
            self.counts['responses'] += 1
            reply = Message(message.kind, entry.request_id, message.word2, message.command,
                            message.word4, message.payload)
            entry.reply = reply
            entry.done.set()
            if entry.connection is not None:
                self.log.message('proxy->dll', entry.connection.id, reply)
                entry.connection.send_message(reply)
            return
        if message.kind == STREAM_DATA_KIND:
            stream = self.counts['stream']
            stream[message.command] = stream.get(message.command, 0) + 1
            with self.lock:
                targets = [c for c, s in self.subscriptions.items() if message.command in s]
            if stream[message.command] <= 3 or not targets:
                self.log.message('device->proxy', None, message,
                                 note=f'stream to {[c.id for c in targets]}')
            for connection in targets:
                connection.send_message(message, log=False)
            return
        if message.kind == NOTIFICATION_KIND:
            self.counts['notifications'] += 1
        with self.lock:
            targets = list(self.subscriptions)
        self.log.message('device->proxy', None, message, note=f'broadcast to {[c.id for c in targets]}')
        for connection in targets:
            connection.send_message(message)

    def attach(self, connection):
        with self.lock:
            self.subscriptions.setdefault(connection, set())

    def detach(self, connection):
        with self.lock:
            streams = self.subscriptions.pop(connection, set())
            still_used = set().union(*self.subscriptions.values()) if self.subscriptions else set()
            for entry in self.pending.values():
                if entry.connection is connection:
                    entry.connection = None
        for stream in sorted(streams - still_used):
            self.log.record('unsubscribe_on_close', conn=connection.id, stream=stream)
            try:
                self._send_to_device(request(0, Command.UNSUBSCRIBE_STREAM, (uint32_field(stream),)),
                                     None, 0)
            except OSError as error:
                self.log.record('unsubscribe_failed', stream=stream, error=str(error))

    def close(self, settle_s=0.3):
        """Unsubscribe every stream a client left subscribed, then give the replies time to arrive."""
        with self.lock:
            connections = list(self.subscriptions)
        for connection in connections:
            self.detach(connection)
        if connections:
            time.sleep(settle_s)


class Connection:
    """One accepted TCP connection; channel 1 connections feed the router."""
    def __init__(self, sock, number, port, log, router):
        self.sock = sock
        self.id = number
        self.port = port
        self.log = log
        self.router = router if CHANNEL_PORTS.get(port) == 1 else None
        self.send_lock = threading.Lock()
        self.closed = False

    def send_message(self, message, log=True):
        data = message.encode()
        if log:
            self.log.bytes('proxy->dll', self.id, self.port, data)
        with self.send_lock:
            if self.closed:
                return
            try:
                self.sock.sendall(data)
            except OSError as error:
                self.log.record('send_failed', conn=self.id, error=str(error))
                self.closed = True

    def serve(self):
        decoder = Decoder()
        if self.router is not None:
            self.router.attach(self)
        try:
            while True:
                data = self.sock.recv(65536)
                if not data:
                    break
                self.log.bytes('dll->proxy', self.id, self.port, data)
                try:
                    messages = decoder.feed(data)
                except ProtocolError as error:
                    self.log.record('framing_error', conn=self.id, error=str(error))
                    break
                for message in messages:
                    if self.router is None:
                        self.log.message('dll->proxy', self.id, message, note='side channel, not answered')
                    else:
                        self.router.from_client(self, message)
        except OSError as error:
            self.log.record('recv_failed', conn=self.id, error=str(error))
        finally:
            self.log.record('close', conn=self.id, port=self.port)
            with self.send_lock:
                self.closed = True
            if self.router is not None:
                self.router.detach(self)
            self.sock.close()


class TetTcpServer:
    """Listens on the tet-tcp ports.

`ports` maps a listening port (0 picks a free one) to the DLL port it stands for.
"""
    def __init__(self, router, log, host='127.0.0.1', ports=None, udp_ports=(TIMESYNC_UDP_PORT,)):
        self.router = router
        self.log = log
        self.listeners = []
        self.numbers = itertools.count(1)
        self.stopping = threading.Event()
        for listen_port, port in (ports or {p: p for p in CHANNEL_PORTS}).items():
            listener = socket.create_server((host, listen_port))
            listener.settimeout(0.2)
            self.listeners.append((listener, port))
        self.threads = [threading.Thread(target=self._accept, args=item, daemon=True,
                                         name=f'tet-tcp-{item[1]}') for item in self.listeners]
        self.datagram_sockets = []
        for port in udp_ports:
            datagrams = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            datagrams.bind((host, port))
            datagrams.settimeout(0.2)
            self.datagram_sockets.append(datagrams)
            self.threads.append(threading.Thread(target=self._log_datagrams, args=(datagrams, port),
                                                 daemon=True, name=f'tet-udp-{port}'))

    @property
    def addresses(self):
        return [listener.getsockname() for listener, _ in self.listeners]

    def start(self):
        for thread in self.threads:
            thread.start()
        self.log.record('listening', addresses=[list(a) for a in self.addresses])
        return self

    def _accept(self, listener, port):
        while not self.stopping.is_set():
            try:
                sock, peer = listener.accept()
            except socket.timeout:
                continue
            except OSError:
                return
            sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
            number = next(self.numbers)
            self.log.record('accept', conn=number, port=port, peer=list(peer))
            connection = Connection(sock, number, port, self.log, self.router)
            threading.Thread(target=connection.serve, daemon=True, name=f'tet-tcp-conn-{number}').start()

    def _log_datagrams(self, datagrams, port):
        """Timesync datagrams are recorded, not answered: their format is not reconstructed."""
        while not self.stopping.is_set():
            try:
                data, peer = datagrams.recvfrom(65536)
            except socket.timeout:
                continue
            except OSError:
                return
            self.log.record('datagram', port=port, peer=list(peer), hex=data.hex())

    def stop(self):
        self.stopping.set()
        for listener, _ in self.listeners:
            listener.close()
        for datagrams in self.datagram_sockets:
            datagrams.close()


class NullBackend:
    """No tracker: requests are logged and never answered (discovery and request recording)."""
    def __init__(self, log):
        self.log = log

    def send(self, data):
        self.log.record('no_device', hex=data.hex())


class UsbBackend:
    """A tracker USB channel; one thread reads it continuously and hands messages to `on_message`.

The tracker stops sending for good when its endpoint is left unread for about a
second while a stream is subscribed, so the reader never waits on the router.
"""
    def __init__(self, channel, log, on_message=None):
        self.channel = channel
        self.log = log
        self.on_message = on_message
        self.write_lock = threading.Lock()
        self.stopping = threading.Event()
        self.failure = None
        self.thread = threading.Thread(target=self._read, daemon=True, name='tracker-reader')

    def start(self):
        self.thread.start()
        return self

    def send(self, data):
        with self.write_lock:
            self.channel.write(data)

    def _read(self):
        decoder = Decoder()
        try:
            while not self.stopping.is_set():
                data = self.channel.read(timeout_ms=100)
                if not data:
                    continue
                for message in decoder.feed(data):
                    self.on_message(message)
        except Exception as error:
            self.failure = error
            self.log.record('tracker_failed', error=repr(error))

    def stop(self):
        self.stopping.set()
        self.thread.join(timeout=2)
