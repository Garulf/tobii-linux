//! Minimal HTTP server: the static page, a JSON status snapshot and a Server-Sent Events stream.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;

use super::events::sse_block;
use crate::pyjson::{self, Json};

pub const LATEST_ONLY_TOPICS: [&str; 4] = ["status", "frame", "gaze_columns", "mesh_topology"];
pub const FLUSH_INTERVAL: Duration = Duration::from_micros(33_333);
pub const HEARTBEAT: Duration = Duration::from_secs(10);
pub const QUEUE_LIMIT: usize = 256;
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
pub const MAX_HEADER_LINES: usize = 100;

/// The page's files, compiled in.
pub const STATIC_FILES: [(&str, &[u8], &str); 3] = [
    ("index.html", include_bytes!("../../../../src/tobii_linux/demo/static/index.html"), "text/html; charset=utf-8"),
    ("demo.js", include_bytes!("../../../../src/tobii_linux/demo/static/demo.js"), "text/javascript; charset=utf-8"),
    ("demo.css", include_bytes!("../../../../src/tobii_linux/demo/static/demo.css"), "text/css; charset=utf-8"),
];

/// Events waiting for one browser: every stream sample, but only the newest of each
/// latest-only topic.
#[derive(Debug)]
pub struct Subscriber {
    limit: usize,
    blocks: RefCell<VecDeque<Rc<Vec<u8>>>>,
    latest: RefCell<Vec<(String, Rc<Vec<u8>>)>>,
    ready: Notify,
    pending: Cell<bool>,
    pub dropped: Cell<u64>,
    closed: Cell<bool>,
}

impl Subscriber {
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            blocks: RefCell::new(VecDeque::new()),
            latest: RefCell::new(Vec::new()),
            ready: Notify::new(),
            pending: Cell::new(false),
            dropped: Cell::new(0),
            closed: Cell::new(false),
        }
    }

    pub fn push(&self, topic: &str, block: Rc<Vec<u8>>) {
        if LATEST_ONLY_TOPICS.contains(&topic) {
            let mut latest = self.latest.borrow_mut();
            match latest.iter_mut().find(|(name, _)| name == topic) {
                Some(entry) => entry.1 = block,
                None => latest.push((topic.into(), block)),
            }
        } else {
            let mut blocks = self.blocks.borrow_mut();
            blocks.push_back(block);
            if blocks.len() > self.limit {
                blocks.pop_front();
                self.dropped.set(self.dropped.get() + 1);
            }
        }
        self.set_ready();
    }

    fn set_ready(&self) {
        self.pending.set(true);
        self.ready.notify_one();
    }

    pub fn take(&self) -> Vec<u8> {
        let mut data: Vec<u8> = self.blocks.borrow_mut().drain(..).flat_map(|block| block.to_vec()).collect();
        data.extend(self.latest.borrow_mut().drain(..).flat_map(|(_, block)| block.to_vec()));
        self.pending.set(false);
        data
    }

    pub fn close(&self) {
        self.closed.set(true);
        self.set_ready();
    }

    pub fn is_closed(&self) -> bool {
        self.closed.get()
    }

    /// Waits until something is pushed or the subscriber closes; false after `timeout`.
    pub async fn wait(&self, timeout: Duration) -> bool {
        if self.pending.get() {
            return true;
        }
        tokio::time::timeout(timeout, self.ready.notified()).await.is_ok()
    }
}

/// Fans published events out to every connected browser.
/// A retained event: topic, its SSE block and its payload.
type Retained = (String, Rc<Vec<u8>>, Json);

#[derive(Debug, Default)]
pub struct EventHub {
    subscribers: RefCell<Vec<Rc<Subscriber>>>,
    retained: RefCell<Vec<Retained>>,
    pub published: Cell<u64>,
}

impl EventHub {
    /// One event already encoded as `block`; `retain` keeps it for new browsers and snapshots.
    pub fn publish_block(&self, topic: &str, block: Vec<u8>, payload: Option<Json>) {
        let block = Rc::new(block);
        self.published.set(self.published.get() + 1);
        if let Some(payload) = payload {
            let mut retained = self.retained.borrow_mut();
            match retained.iter_mut().find(|(name, ..)| name == topic) {
                Some(entry) => {
                    entry.1 = Rc::clone(&block);
                    entry.2 = payload;
                }
                None => retained.push((topic.into(), Rc::clone(&block), payload)),
            }
        }
        for subscriber in self.subscribers.borrow().iter() {
            subscriber.push(topic, Rc::clone(&block));
        }
    }

    pub fn publish(&self, topic: &str, payload: Json, retain: bool) {
        if let Some(block) = encode(topic, &payload) {
            self.publish_block(topic, block, retain.then_some(payload));
        }
    }

    pub fn snapshot(&self, topic: &str) -> Option<Json> {
        self.retained.borrow().iter().find(|(name, ..)| name == topic).map(|(_, _, payload)| payload.clone())
    }

    pub fn subscribe(&self) -> Rc<Subscriber> {
        let subscriber = Rc::new(Subscriber::new(QUEUE_LIMIT));
        for (_, block, _) in self.retained.borrow().iter() {
            subscriber.blocks.borrow_mut().push_back(Rc::clone(block));
        }
        subscriber.set_ready();
        self.subscribers.borrow_mut().push(Rc::clone(&subscriber));
        subscriber
    }

    pub fn unsubscribe(&self, subscriber: &Rc<Subscriber>) {
        self.subscribers.borrow_mut().retain(|other| !Rc::ptr_eq(other, subscriber));
    }

    pub fn close(&self) {
        for subscriber in self.subscribers.borrow().iter() {
            subscriber.close();
        }
    }
}

/// The SSE block of one event; `None` for a payload JSON cannot carry, which Python refused.
pub fn encode(topic: &str, payload: &Json) -> Option<Vec<u8>> {
    if super::events::has_non_finite(payload) {
        log::error!("dropping the {topic} event: Out of range float values are not JSON compliant");
        return None;
    }
    Some(sse_block(topic, &pyjson::compact(payload)))
}

pub fn http_response(status: &str, body: &[u8], content_type: &str, extra: &[&str]) -> Vec<u8> {
    let mut headers = vec![
        format!("HTTP/1.1 {status}"),
        format!("Content-Type: {content_type}"),
        format!("Content-Length: {}", body.len()),
        "Cache-Control: no-store".into(),
        "Connection: close".into(),
    ];
    headers.extend(extra.iter().map(|header| header.to_string()));
    let mut response = format!("{}\r\n\r\n", headers.join("\r\n")).into_bytes();
    response.extend_from_slice(body);
    response
}

const TEXT: &str = "text/plain; charset=utf-8";
const SSE_HEADERS: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-store\r\nConnection: keep-alive\r\nX-Accel-Buffering: no\r\n\r\nretry: 1000\n\n";

/// The path of a request target, as `urlsplit(target).path`.
pub fn target_path(target: &str) -> &str {
    let rest = match target.split_once("://") {
        Some((scheme, rest))
            if !scheme.is_empty() && scheme.bytes().all(|b| b.is_ascii_alphanumeric() || b"+-.".contains(&b)) =>
        {
            rest.find('/').map_or("", |slash| &rest[slash..])
        }
        _ => target.strip_prefix("//").map_or(target, |rest| rest.find('/').map_or("", |slash| &rest[slash..])),
    };
    let end = rest.find(['?', '#']).unwrap_or(rest.len());
    &rest[..end]
}

#[derive(Debug)]
pub struct DemoServer {
    pub hub: Rc<EventHub>,
    pub port: u16,
    accept: tokio::task::JoinHandle<()>,
    connections: Rc<RefCell<Vec<Rc<Notify>>>>,
}

impl DemoServer {
    pub async fn start(hub: Rc<EventHub>, host: &str, port: u16) -> std::io::Result<Self> {
        let listener = crate::service::listen(host, port).await?;
        let port = listener.local_addr()?.port();
        let connections: Rc<RefCell<Vec<Rc<Notify>>>> = Rc::default();
        let accept = {
            let (hub, connections) = (Rc::clone(&hub), Rc::clone(&connections));
            tokio::task::spawn_local(accept_loop(listener, hub, connections))
        };
        Ok(Self { hub, port, accept, connections })
    }

    pub async fn close(&self) {
        self.hub.close();
        self.accept.abort();
        for closed in self.connections.borrow().iter() {
            closed.notify_one();
        }
        tokio::task::yield_now().await;
    }
}

async fn accept_loop(listener: TcpListener, hub: Rc<EventHub>, connections: Rc<RefCell<Vec<Rc<Notify>>>>) {
    while let Ok((stream, _)) = listener.accept().await {
        let closed = Rc::new(Notify::new());
        connections.borrow_mut().push(Rc::clone(&closed));
        let (hub, connections) = (Rc::clone(&hub), Rc::clone(&connections));
        tokio::task::spawn_local(async move {
            tokio::select! {
                () = handle(stream, &hub) => {}
                () = closed.notified() => {}
            }
            connections.borrow_mut().retain(|other| !Rc::ptr_eq(other, &closed));
        });
    }
}

async fn read_request(reader: &mut BufReader<tokio::net::tcp::ReadHalf<'_>>) -> Option<(String, String)> {
    let mut line = Vec::new();
    reader.read_until(b'\n', &mut line).await.ok()?;
    let text: String = line.iter().map(|&b| b as char).collect();
    let parts: Vec<&str> = text.split_whitespace().collect();
    let [method, target, _] = parts.as_slice() else { return None };
    let request = (method.to_string(), target.to_string());
    for _ in 0..MAX_HEADER_LINES {
        let mut header = Vec::new();
        reader.read_until(b'\n', &mut header).await.ok()?;
        if header == b"\r\n" || header == b"\n" || header.is_empty() {
            return Some(request);
        }
    }
    None
}

fn route(hub: &EventHub, path: &str) -> Vec<u8> {
    let path = if path == "/" || path == "/index.html" { "/static/index.html" } else { path };
    if path == "/api/status" {
        let status =
            hub.snapshot("status").unwrap_or_else(|| crate::pyjson::object([("connection", "starting".into())]));
        return http_response("200 OK", pyjson::compact(&status).as_bytes(), "application/json", &[]);
    }
    if let Some(name) = path.strip_prefix("/static/")
        && let Some((_, body, content_type)) = STATIC_FILES.iter().find(|(file, ..)| *file == name)
    {
        return http_response("200 OK", body, content_type, &[]);
    }
    http_response("404 Not Found", b"Not found\n", TEXT, &[])
}

async fn handle(mut stream: TcpStream, hub: &EventHub) {
    let (reader, mut writer) = stream.split();
    let mut reader = BufReader::new(reader);
    let Ok(Some((method, target))) = tokio::time::timeout(REQUEST_TIMEOUT, read_request(&mut reader)).await else {
        return;
    };
    let path = target_path(&target);
    let response = if method != "GET" && method != "HEAD" {
        http_response("405 Method Not Allowed", b"Only GET is supported\n", TEXT, &["Allow: GET"])
    } else if path == "/events" && method == "GET" {
        stream_events(&mut writer, hub).await;
        return;
    } else {
        let response = route(hub, path);
        if method == "GET" {
            response
        } else {
            let end = response.windows(4).position(|w| w == b"\r\n\r\n").map_or(response.len(), |i| i + 4);
            response[..end].to_vec()
        }
    };
    let _ = writer.write_all(&response).await;
    let _ = writer.shutdown().await;
}

async fn stream_events(writer: &mut tokio::net::tcp::WriteHalf<'_>, hub: &EventHub) {
    let subscriber = hub.subscribe();
    let streamed = async {
        writer.write_all(SSE_HEADERS).await?;
        while !subscriber.is_closed() {
            if !subscriber.wait(HEARTBEAT).await {
                writer.write_all(b": keepalive\n\n").await?;
                continue;
            }
            let data = subscriber.take();
            if !data.is_empty() {
                writer.write_all(&data).await?;
            }
            tokio::time::sleep(FLUSH_INTERVAL).await;
        }
        std::io::Result::Ok(())
    };
    let _ = streamed.await;
    hub.unsubscribe(&subscriber);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_of_request_targets() {
        assert_eq!(target_path("/static/demo.js?v=1"), "/static/demo.js");
        assert_eq!(target_path("http://localhost:8765/api/status"), "/api/status");
        assert_eq!(target_path("/#x"), "/");
        assert_eq!(target_path("http://host"), "");
    }

    #[test]
    fn slow_browsers_get_the_newest_image_and_a_bounded_backlog() {
        let subscriber = Subscriber::new(3);
        for index in 0..5 {
            subscriber.push("frame", Rc::new(format!("f{index}").into_bytes()));
            subscriber.push("gaze", Rc::new(format!("g{index}").into_bytes()));
        }
        assert_eq!(subscriber.take(), b"g2g3g4f4");
        assert_eq!(subscriber.dropped.get(), 2);
        assert!(subscriber.take().is_empty());
    }
}
