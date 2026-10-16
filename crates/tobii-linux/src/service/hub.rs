//! Fan-out of samples to every streaming subscription on the service's event loop.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

type Sink<T> = Rc<dyn Fn(&T)>;

/// Delivers each published item to every subscribed sink; never blocks the producer.
pub struct Hub<T> {
    sinks: RefCell<Vec<(u64, Sink<T>)>>,
    next: Cell<u64>,
}

impl<T> std::fmt::Debug for Hub<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hub").field("sinks", &self.sinks.borrow().len()).finish()
    }
}

impl<T> Default for Hub<T> {
    fn default() -> Self {
        Self { sinks: RefCell::new(Vec::new()), next: Cell::new(1) }
    }
}

impl<T> Hub<T> {
    pub fn subscribe(&self, sink: impl Fn(&T) + 'static) -> u64 {
        let id = self.next.get();
        self.next.set(id + 1);
        self.sinks.borrow_mut().push((id, Rc::new(sink)));
        id
    }

    pub fn unsubscribe(&self, id: u64) {
        self.sinks.borrow_mut().retain(|(sink, _)| *sink != id);
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
