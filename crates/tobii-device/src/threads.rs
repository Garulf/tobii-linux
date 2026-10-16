//! Readers on their own threads, so a slow consumer never delays reading the tracker.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

struct State<T, E> {
    items: VecDeque<T>,
    error: Option<E>,
    done: bool,
}

struct Shared<T, E> {
    state: Mutex<State<T, E>>,
    changed: Condvar,
}

/// Consumer side of a reader thread; see [`newest`] and [`drained`].
pub struct Relay<T, E> {
    shared: Arc<Shared<T, E>>,
}

impl<T, E> std::fmt::Debug for Relay<T, E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Relay").finish_non_exhaustive()
    }
}

fn spawn<T, E, I, F>(source: I, depth: usize, mut on_receive: F, name: &str) -> Relay<T, E>
where
    T: Send + 'static,
    E: Send + 'static,
    I: Iterator<Item = Result<T, E>> + Send + 'static,
    F: FnMut(&T) + Send + 'static,
{
    let shared = Arc::new(Shared {
        state: Mutex::new(State { items: VecDeque::new(), error: None, done: false }),
        changed: Condvar::new(),
    });
    let worker = Arc::clone(&shared);
    thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            for item in source {
                match item {
                    Ok(item) => {
                        on_receive(&item);
                        let mut state = worker.state.lock().unwrap_or_else(|p| p.into_inner());
                        if state.items.len() == depth {
                            state.items.pop_front();
                        }
                        state.items.push_back(item);
                    }
                    Err(error) => {
                        worker.state.lock().unwrap_or_else(|p| p.into_inner()).error = Some(error);
                        break;
                    }
                }
                worker.changed.notify_all();
            }
            worker.state.lock().unwrap_or_else(|p| p.into_inner()).done = true;
            worker.changed.notify_all();
        })
        .expect("spawn a reader thread");
    Relay { shared }
}

/// Reads `source` on its own thread and yields only the newest item: items that arrive
/// while the consumer is busy are dropped. `on_receive` runs on the reader thread for
/// every item as it arrives. An error of the source ends the iteration after the item
/// before it.
pub fn newest<T, E, I, F>(source: I, on_receive: F) -> Relay<T, E>
where
    T: Send + 'static,
    E: Send + 'static,
    I: Iterator<Item = Result<T, E>> + Send + 'static,
    F: FnMut(&T) + Send + 'static,
{
    spawn(source, 1, on_receive, "tracker-frames")
}

/// Reads `source` on its own thread; when the consumer falls `depth` items behind, the
/// oldest waiting items are dropped.
pub fn drained<T, E, I>(source: I, depth: usize) -> Relay<T, E>
where
    T: Send + 'static,
    E: Send + 'static,
    I: Iterator<Item = Result<T, E>> + Send + 'static,
{
    spawn(source, depth.max(1), |_| {}, "tracker-drain")
}

impl<T, E> Iterator for Relay<T, E> {
    type Item = Result<T, E>;

    fn next(&mut self) -> Option<Self::Item> {
        let state = self.shared.state.lock().unwrap_or_else(|p| p.into_inner());
        let mut state = self
            .shared
            .changed
            .wait_while(state, |state| state.items.is_empty() && !state.done && state.error.is_none())
            .unwrap_or_else(|p| p.into_inner());
        if let Some(item) = state.items.pop_front() {
            return Some(Ok(item));
        }
        state.error.take().map(Err)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    #[test]
    fn items_arriving_while_busy_are_dropped() {
        let (consumed_tx, consumed_rx) = mpsc::channel::<()>();
        let (produced_tx, produced_rx) = mpsc::channel::<()>();
        let source = (1..=3).map(move |n| {
            if n == 2 {
                consumed_rx.recv().unwrap();
            }
            if n == 3 {
                produced_tx.send(()).unwrap();
            }
            Ok::<_, ()>(n)
        });
        let mut frames = newest(source, |_| {});
        assert_eq!(frames.next(), Some(Ok(1)));
        consumed_tx.send(()).unwrap();
        produced_rx.recv().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert_eq!(frames.collect::<Vec<_>>(), [Ok(3)]);
    }

    #[test]
    fn reader_errors_reach_the_consumer_and_on_receive_sees_every_item() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let record = Arc::clone(&seen);
        let items: Vec<_> =
            newest([Ok(1), Ok(2), Ok(3)].into_iter(), move |n| record.lock().unwrap().push(*n)).collect();
        assert_eq!(items.last(), Some(&Ok::<_, &str>(3)));
        assert_eq!(*seen.lock().unwrap(), [1, 2, 3]);
        let failing = newest([Ok(1), Err("unplugged")].into_iter(), |_| {});
        assert!(failing.into_iter().any(|item| item == Err("unplugged")));
    }

    #[test]
    fn drained_keeps_the_newest_depth_items() {
        let (go_tx, go_rx) = mpsc::channel::<()>();
        let source = (0..10).map(move |n| {
            if n == 0 {
                go_rx.recv().unwrap();
            }
            Ok::<_, ()>(n)
        });
        let relay = drained(source, 3);
        go_tx.send(()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(relay.map(Result::unwrap).collect::<Vec<_>>(), [7, 8, 9]);
    }
}
