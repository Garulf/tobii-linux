use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// A stop request shared between threads, like Python's `threading.Event`.
pub trait StopSignal: Send + Sync {
    fn is_set(&self) -> bool;
    /// Waits up to `timeout`; true when the signal is set.
    fn wait(&self, timeout: Duration) -> bool;
}

#[derive(Debug, Clone, Default)]
pub struct StopFlag(Arc<(Mutex<bool>, Condvar)>);

impl StopFlag {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&self) {
        let (flag, changed) = &*self.0;
        *flag.lock().unwrap_or_else(|p| p.into_inner()) = true;
        changed.notify_all();
    }

    pub fn clear(&self) {
        *self.0.0.lock().unwrap_or_else(|p| p.into_inner()) = false;
    }
}

impl StopSignal for StopFlag {
    fn is_set(&self) -> bool {
        *self.0.0.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn wait(&self, timeout: Duration) -> bool {
        let (flag, changed) = &*self.0;
        let guard = flag.lock().unwrap_or_else(|p| p.into_inner());
        let (guard, _) = changed.wait_timeout_while(guard, timeout, |set| !*set).unwrap_or_else(|p| p.into_inner());
        *guard
    }
}
