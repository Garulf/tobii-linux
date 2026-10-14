"""Minimal asyncio HTTP server: the static page, a JSON status snapshot and a Server-Sent Events stream."""
import asyncio
from importlib import resources
from urllib.parse import urlsplit

from .events import compact_json, sse_block

LATEST_ONLY_TOPICS = frozenset({'status', 'frame', 'gaze_columns', 'mesh_topology'})
FLUSH_INTERVAL_S = 1 / 30
HEARTBEAT_S = 10.0
QUEUE_LIMIT = 256
REQUEST_TIMEOUT_S = 10.0
MAX_HEADER_LINES = 100
CONTENT_TYPES = {'.html': 'text/html; charset=utf-8', '.js': 'text/javascript; charset=utf-8',
                 '.css': 'text/css; charset=utf-8', '.svg': 'image/svg+xml'}


class Subscriber:
    """Events waiting for one browser: every stream sample, but only the newest of each latest-only topic."""
    def __init__(self, limit=QUEUE_LIMIT):
        self.limit = limit
        self.blocks = []
        self.latest = {}
        self.ready = asyncio.Event()
        self.dropped = 0
        self.closed = False

    def push(self, topic, block):
        if topic in LATEST_ONLY_TOPICS:
            self.latest[topic] = block
        else:
            self.blocks.append(block)
            if len(self.blocks) > self.limit:
                del self.blocks[0]
                self.dropped += 1
        self.ready.set()

    def take(self):
        data = b''.join(self.blocks) + b''.join(self.latest.values())
        self.blocks, self.latest = [], {}
        self.ready.clear()
        return data

    def close(self):
        self.closed = True
        self.ready.set()


class EventHub:
    """Fans published events out to every connected browser; publish_threadsafe may be called from any thread."""
    def __init__(self):
        self.loop = None
        self.subscribers = set()
        self.retained = {}
        self.published = 0

    def attach(self, loop):
        self.loop = loop

    def publish(self, topic, payload, retain=False):
        block = sse_block(topic, compact_json(payload))
        self.published += 1
        if retain:
            self.retained[topic] = (block, payload)
        for subscriber in self.subscribers:
            subscriber.push(topic, block)

    def publish_threadsafe(self, topic, payload, retain=False):
        if self.loop is not None and not self.loop.is_closed():
            self.loop.call_soon_threadsafe(self.publish, topic, payload, retain)

    def snapshot(self, topic):
        entry = self.retained.get(topic)
        return entry[1] if entry is not None else None

    def subscribe(self):
        subscriber = Subscriber()
        for block, _ in self.retained.values():
            subscriber.blocks.append(block)
        subscriber.ready.set()
        self.subscribers.add(subscriber)
        return subscriber

    def unsubscribe(self, subscriber):
        self.subscribers.discard(subscriber)

    def close(self):
        for subscriber in list(self.subscribers):
            subscriber.close()


def static_files(package='tobii_linux.demo', directory='static'):
    root = resources.files(package)/directory
    files = {}
    for entry in root.iterdir():
        suffix = '.' + entry.name.rsplit('.', 1)[-1]
        if entry.is_file() and suffix in CONTENT_TYPES:
            files[entry.name] = (entry.read_bytes(), CONTENT_TYPES[suffix])
    return files


def http_response(status, body=b'', content_type='text/plain; charset=utf-8', extra=()):
    headers = [f'HTTP/1.1 {status}', f'Content-Type: {content_type}', f'Content-Length: {len(body)}',
               'Cache-Control: no-store', 'Connection: close', *extra]
    return ('\r\n'.join(headers) + '\r\n\r\n').encode() + body


SSE_HEADERS = ('HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-store\r\n'
               'Connection: keep-alive\r\nX-Accel-Buffering: no\r\n\r\nretry: 1000\n\n').encode()


class DemoServer:
    def __init__(self, hub, host='127.0.0.1', port=0, files=None):
        self.hub = hub
        self.host, self.requested_port = host, port
        self.files = static_files() if files is None else files
        self.server = None
        self.writers = set()

    @property
    def port(self):
        return self.server.sockets[0].getsockname()[1]

    async def start(self):
        self.server = await asyncio.start_server(self.handle, self.host, self.requested_port)
        return self

    async def close(self):
        self.hub.close()
        self.server.close()
        for writer in list(self.writers):
            writer.close()
        await self.server.wait_closed()

    async def handle(self, reader, writer):
        self.writers.add(writer)
        try:
            method, target = await asyncio.wait_for(self.read_request(reader), REQUEST_TIMEOUT_S)
            path = urlsplit(target).path
            if method not in ('GET', 'HEAD'):
                writer.write(http_response('405 Method Not Allowed', b'Only GET is supported\n', extra=('Allow: GET',)))
            elif path == '/events' and method == 'GET':
                await self.stream_events(writer)
            else:
                response = self.route(path)
                writer.write(response if method == 'GET' else response.split(b'\r\n\r\n', 1)[0] + b'\r\n\r\n')
            await writer.drain()
        except (ConnectionError, asyncio.IncompleteReadError, asyncio.TimeoutError, ValueError):
            pass
        finally:
            self.writers.discard(writer)
            writer.close()

    async def read_request(self, reader):
        request_line = (await reader.readline()).decode('latin-1').split()
        if len(request_line) != 3:
            raise ValueError('Malformed request line')
        for _ in range(MAX_HEADER_LINES):
            if (await reader.readline()) in (b'\r\n', b'\n', b''):
                return request_line[0], request_line[1]
        raise ValueError('Too many request headers')

    def route(self, path):
        if path in ('/', '/index.html'):
            path = '/static/index.html'
        if path == '/api/status':
            status = self.hub.snapshot('status')
            body = compact_json(status if status is not None else {'connection': 'starting'}).encode()
            return http_response('200 OK', body, 'application/json')
        if path.startswith('/static/') and path[len('/static/'):] in self.files:
            body, content_type = self.files[path[len('/static/'):]]
            return http_response('200 OK', body, content_type)
        return http_response('404 Not Found', b'Not found\n')

    async def stream_events(self, writer):
        subscriber = self.hub.subscribe()
        try:
            writer.write(SSE_HEADERS)
            await writer.drain()
            while not subscriber.closed:
                try:
                    await asyncio.wait_for(subscriber.ready.wait(), HEARTBEAT_S)
                except asyncio.TimeoutError:
                    writer.write(b': keepalive\n\n')
                    await writer.drain()
                    continue
                data = subscriber.take()
                if data:
                    writer.write(data)
                    await writer.drain()
                await asyncio.sleep(FLUSH_INTERVAL_S)
        finally:
            self.hub.unsubscribe(subscriber)
