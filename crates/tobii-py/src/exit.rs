//! Keeps native work on daemon threads from crashing the interpreter at exit.
//!
//! Before Python 3.14, a daemon thread that reattaches to a finalizing interpreter is
//! ended with `pthread_exit`, which cannot unwind through Rust frames and aborts the
//! process. Once `atexit` runs, threads other than the main thread that return from
//! detached native work, or want to call back into Python, park forever instead; the
//! process then exits normally, as Python 3.14 does with such threads.

use std::cell::Cell;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, ThreadId};

use pyo3::Python;
use pyo3::prelude::*;
use pyo3::types::PyModule;

static EXITING: AtomicBool = AtomicBool::new(false);
static MAIN_THREAD: OnceLock<ThreadId> = OnceLock::new();

thread_local! {
    static DETACHED: Cell<bool> = const { Cell::new(false) };
}

/// Runs `f` with this thread's detached state set to `detached`.
fn with_detached<R>(detached: bool, f: impl FnOnce() -> R) -> R {
    let previous = DETACHED.replace(detached);
    let result = f();
    DETACHED.set(previous);
    result
}

#[pyfunction]
fn interpreter_exiting() {
    EXITING.store(true, Ordering::SeqCst);
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    let main = m.py().import("threading")?.call_method0("main_thread")?;
    let current = m.py().import("threading")?.call_method0("current_thread")?;
    if main.is(&current) {
        let _ = MAIN_THREAD.set(thread::current().id());
    }
    let hook = wrap_pyfunction!(interpreter_exiting, m)?;
    m.py().import("atexit")?.call_method1("register", (hook,))?;
    Ok(())
}

/// Parks a thread that does not hold the interpreter, never the main thread.
fn park_if_exiting() {
    if EXITING.load(Ordering::SeqCst) && DETACHED.get() && MAIN_THREAD.get() != Some(&thread::current().id()) {
        loop {
            thread::park();
        }
    }
}

/// `py.detach(f)`, parking this thread afterwards when the interpreter is exiting.
pub fn detached<T: Send, F: Send + FnOnce() -> T>(py: Python<'_>, f: F) -> T {
    py.detach(|| {
        with_detached(true, || {
            let result = f();
            park_if_exiting();
            result
        })
    })
}

/// `Python::try_attach(f)` from detached native code; parks instead when the interpreter
/// is exiting.
pub fn reattach<R>(f: impl for<'py> FnOnce(Python<'py>) -> R) -> Option<R> {
    park_if_exiting();
    with_detached(false, || Python::try_attach(f))
}
