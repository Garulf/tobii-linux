//! Fan-out of samples to every streaming subscription on the service's event loop.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

type Sink<T> = Rc<dyn Fn(&T)>;
type Watch = Rc<dyn Fn()>;

/// Delivers each published item to every subscribed sink; never blocks the producer.
pub struct Hub<T> {
    sinks: RefCell<Vec<(u64, Sink<T>)>>,
    next: Cell<u64>,
    watch: RefCell<Option<Watch>>,
}

impl<T> std::fmt::Debug for Hub<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hub").field("sinks", &self.sinks.borrow().len()).finish()
    }
}

impl<T> Default for Hub<T> {
    fn default() -> Self {
        Self { sinks: RefCell::new(Vec::new()), next: Cell::new(1), watch: RefCell::new(None) }
    }
}

impl<T> Hub<T> {
    pub fn subscribe(&self, sink: impl Fn(&T) + 'static) -> u64 {
        let id = self.next.get();
        self.next.set(id + 1);
        self.sinks.borrow_mut().push((id, Rc::new(sink)));
        self.changed();
        id
    }

    pub fn unsubscribe(&self, id: u64) {
        let before = self.subscribers();
        self.sinks.borrow_mut().retain(|(sink, _)| *sink != id);
        if self.subscribers() != before {
            self.changed();
        }
    }

    /// Runs `watch` after each subscribe and each unsubscribe that removed a sink.
    pub fn set_watch(&self, watch: impl Fn() + 'static) {
        *self.watch.borrow_mut() = Some(Rc::new(watch));
    }

    fn changed(&self) {
        let watch = self.watch.borrow().clone();
        if let Some(watch) = watch {
            watch();
        }
    }

    pub fn subscribers(&self) -> usize {
        self.sinks.borrow().len()
    }

    pub fn publish(&self, item: &T) {
        let sinks: Vec<Sink<T>> = self.sinks.borrow().iter().map(|(_, sink)| Rc::clone(sink)).collect();
        for sink in sinks {
            sink(item);
        }
    }
}
