use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tobii_control::protocol::{
    FRAME_CONTROL_EVENT, FRAME_CONTROL_HELLO, FRAME_CONTROL_REPLY, FRAME_CONTROL_REQUEST, decode_request, encode,
    split_frame,
};
use tobii_control::{Event, Reply, Request, Status};
use tobii_experience::{Link, LinkUpdate};

const WAIT: Duration = Duration::from_secs(5);

fn status() -> Status {
    Status {
        attached: true,
        enabled: true,
        released: false,
        display_area: true,
        profile: Some("Garulf".into()),
        calibration_matches_profile: true,
    }
}

/// A control service that greets every connection with a status event and answers each
/// request with the same status, unless it is silent. It records the requests it receives.
/// Dropping it closes the listener and the connection.
struct Stub {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    requests: Arc<Mutex<Vec<Request>>>,
}

impl Stub {
    fn bind(addr: SocketAddr) -> Stub {
        Stub::start(addr, true)
    }

    /// Accepts the hello and the requests but never replies to them.
    fn bind_silent(addr: SocketAddr) -> Stub {
        Stub::start(addr, false)
    }

    fn received(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }

    fn wait_for_request(&self, wanted: &Request) {
        let deadline = Instant::now() + WAIT;
        while !self.received().contains(wanted) {
            assert!(Instant::now() < deadline, "the stub never received {wanted:?}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn start(addr: SocketAddr, answer: bool) -> Stub {
        let deadline = Instant::now() + WAIT;
        let listener = loop {
            match TcpListener::bind(addr) {
                Ok(listener) => break listener,
                Err(_) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(error) => panic!("cannot bind {addr}: {error}"),
            }
        };
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let log = requests.clone();
        let thread = std::thread::spawn(move || {
            while !flag.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => return serve(stream, &flag, &log, answer),
                    Err(_) => std::thread::sleep(Duration::from_millis(10)),
                }
            }
        });
        Stub { stop, thread: Some(thread), requests }
    }
}

impl Drop for Stub {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

fn serve(mut stream: std::net::TcpStream, stop: &AtomicBool, log: &Mutex<Vec<Request>>, answer: bool) {
    stream.set_nonblocking(false).unwrap();
    stream.set_read_timeout(Some(Duration::from_millis(20))).unwrap();
    let mut received = Vec::new();
    let mut chunk = [0u8; 4096];
    while !stop.load(Ordering::Relaxed) {
        match stream.read(&mut chunk) {
            Ok(0) => return,
            Ok(read) => received.extend_from_slice(&chunk[..read]),
            Err(error) if matches!(error.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(_) => return,
        }
        while let Some((kind, payload, used)) = split_frame(&received).unwrap() {
            received.drain(..used);
            if kind == FRAME_CONTROL_HELLO {
                stream.write_all(&encode(FRAME_CONTROL_EVENT, &Event::Status(status())).unwrap()).unwrap();
            } else if kind == FRAME_CONTROL_REQUEST {
                let (id, request) = decode_request(&payload).unwrap();
                log.lock().unwrap().push(request);
                if answer {
                    let reply = Reply { id, result: Ok(status()) };
                    stream.write_all(&encode(FRAME_CONTROL_REPLY, &reply).unwrap()).unwrap();
                }
            }
        }
    }
}

fn collect() -> (impl Fn(LinkUpdate) + Send + 'static, Receiver<LinkUpdate>) {
    let (sender, receiver) = mpsc::channel();
    let sender = Mutex::new(sender);
    (move |update| drop(sender.lock().unwrap().send(update)), receiver)
}

fn wait_for(receiver: &Receiver<LinkUpdate>, wanted: impl Fn(&LinkUpdate) -> bool) -> LinkUpdate {
    let deadline = Instant::now() + WAIT;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let update = receiver.recv_timeout(remaining).expect("the expected update did not arrive");
        if wanted(&update) {
            return update;
        }
    }
}

fn free_port_addr() -> SocketAddr {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    listener.local_addr().unwrap()
}

#[test]
fn client_reconnects_after_service_restart() {
    let addr = free_port_addr();
    let (on_incoming, updates) = collect();
    let stub = Stub::bind(addr);
    let _link = Link::start(addr, on_incoming);

    wait_for(&updates, |update| matches!(update, LinkUpdate::Status(Some(_))));
    drop(stub);
    wait_for(&updates, |update| matches!(update, LinkUpdate::Status(None)));

    let _restarted = Stub::bind(addr);
    let update = wait_for(&updates, |update| matches!(update, LinkUpdate::Status(Some(_))));
    assert_eq!(update, LinkUpdate::Status(Some(status())));
}

#[test]
fn unreachable_service_reports_no_status() {
    let (on_incoming, updates) = collect();
    let _link = Link::start(free_port_addr(), on_incoming);
    assert_eq!(wait_for(&updates, |_| true), LinkUpdate::Status(None));
}

#[test]
fn requests_while_disconnected_fail() {
    let (on_incoming, updates) = collect();
    let link = Link::start(free_port_addr(), on_incoming);
    link.send(Request::SetEnabled(false));
    wait_for(&updates, |update| matches!(update, LinkUpdate::Failed(_)));
}

#[test]
fn requests_reach_the_service_when_connected() {
    let addr = free_port_addr();
    let _stub = Stub::bind(addr);
    let (on_incoming, updates) = collect();
    let link = Link::start(addr, on_incoming);
    wait_for(&updates, |update| matches!(update, LinkUpdate::Status(Some(_))));
    link.send(Request::SetEnabled(false));
    wait_for(&updates, |update| matches!(update, LinkUpdate::Status(Some(_))));
}

#[test]
fn dropping_the_link_stops_its_thread() {
    let (on_incoming, updates) = collect();
    let link = Link::start(free_port_addr(), on_incoming);
    wait_for(&updates, |_| true);
    let started = Instant::now();
    drop(link);
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[test]
fn dropping_the_link_does_not_wait_for_every_queued_request() {
    let addr = free_port_addr();
    let stub = Stub::bind_silent(addr);
    let (on_incoming, updates) = collect();
    let link = Link::start(addr, on_incoming);
    wait_for(&updates, |update| matches!(update, LinkUpdate::Status(Some(_))));
    for _ in 0..5 {
        link.send(Request::SetEnabled(false));
    }
    stub.wait_for_request(&Request::SetEnabled(false));
    let started = Instant::now();
    drop(link);
    assert!(started.elapsed() < Duration::from_millis(3500), "drop took {:?}", started.elapsed());
}

#[test]
fn gaze_subscription_requested_while_the_service_is_down_is_sent_on_connect() {
    let addr = free_port_addr();
    let (on_incoming, updates) = collect();
    let link = Link::start(addr, on_incoming);
    wait_for(&updates, |update| matches!(update, LinkUpdate::Status(None)));
    link.send(Request::SubscribeGaze(true));

    let stub = Stub::bind(addr);
    stub.wait_for_request(&Request::SubscribeGaze(true));
    wait_for(&updates, |update| matches!(update, LinkUpdate::Status(Some(_))));
    assert!(updates.try_iter().all(|update| !matches!(update, LinkUpdate::Failed(_))));
}

#[test]
fn gaze_subscription_survives_a_service_restart() {
    let addr = free_port_addr();
    let stub = Stub::bind(addr);
    let (on_incoming, updates) = collect();
    let link = Link::start(addr, on_incoming);
    wait_for(&updates, |update| matches!(update, LinkUpdate::Status(Some(_))));
    link.send(Request::SubscribeGaze(true));
    stub.wait_for_request(&Request::SubscribeGaze(true));
    drop(stub);
    wait_for(&updates, |update| matches!(update, LinkUpdate::Status(None)));

    let restarted = Stub::bind(addr);
    restarted.wait_for_request(&Request::SubscribeGaze(true));
}

#[test]
fn gaze_subscription_switched_off_is_not_replayed() {
    let addr = free_port_addr();
    let (on_incoming, updates) = collect();
    let link = Link::start(addr, on_incoming);
    wait_for(&updates, |update| matches!(update, LinkUpdate::Status(None)));
    link.send(Request::SubscribeGaze(true));
    link.send(Request::SubscribeGaze(false));

    let stub = Stub::bind(addr);
    wait_for(&updates, |update| matches!(update, LinkUpdate::Status(Some(_))));
    std::thread::sleep(Duration::from_millis(300));
    assert!(!stub.received().contains(&Request::SubscribeGaze(true)));
}
