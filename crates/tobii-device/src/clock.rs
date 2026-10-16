//! Device timestamps onto a host clock.

use std::collections::VecDeque;

/// A host clock in microseconds. A clock with a generation can change its source; users
/// that keep offsets to it start over when the generation changes.
pub trait HostClock: Send {
    fn now_us(&self) -> i64;
    fn generation(&self) -> Option<i64> {
        None
    }
}

/// CLOCK_MONOTONIC in microseconds.
#[derive(Debug, Clone, Copy, Default)]
pub struct MonotonicClock;

impl HostClock for MonotonicClock {
    fn now_us(&self) -> i64 {
        let now = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
        now.tv_sec * 1_000_000 + now.tv_nsec / 1000
    }
}

impl<F: Fn() -> i64 + Send> HostClock for F {
    fn now_us(&self) -> i64 {
        self()
    }
}

pub const CLOCK_WINDOW: usize = 64;

/// The mapping state of [`ClockMapper`], fed with host clock readings by its caller.
#[derive(Debug, Clone)]
pub struct ClockOffsets {
    offsets: VecDeque<i64>,
    window: usize,
    offset: Option<i64>,
    generation: Option<i64>,
}

impl ClockOffsets {
    pub fn new(window: usize, generation: Option<i64>) -> Self {
        let window = window.max(1);
        Self { offsets: VecDeque::with_capacity(window), window, offset: None, generation }
    }

    pub fn offset(&self) -> Option<i64> {
        self.offset
    }

    /// Receipt delays kept for the minimum, oldest first.
    pub fn offsets(&self) -> &VecDeque<i64> {
        &self.offsets
    }

    /// Records the receipt of `device_us` at host time `now_us`; a new clock generation
    /// starts the mapping over.
    pub fn observe(&mut self, generation: Option<i64>, now_us: i64, device_us: i64) {
        if generation != self.generation {
            self.generation = generation;
            self.offsets.clear();
        }
        if self.offsets.len() == self.window {
            self.offsets.pop_front();
        }
        self.offsets.push_back(now_us - device_us);
        self.offset = self.offsets.iter().copied().min();
    }

    /// Host time of `device_us`, never later than `now_us`.
    pub fn map(&self, now_us: i64, generation: Option<i64>, device_us: i64) -> i64 {
        match self.offset {
            Some(offset) if generation == self.generation => (device_us + offset).min(now_us),
            _ => now_us,
        }
    }
}

/// Maps device timestamps onto a host clock using the smallest recent receipt delay.
#[derive(Debug, Clone)]
pub struct ClockMapper<C> {
    clock: C,
    state: ClockOffsets,
}

impl<C: HostClock> ClockMapper<C> {
    pub fn new(clock: C) -> Self {
        Self::with_window(clock, CLOCK_WINDOW)
    }

    pub fn with_window(clock: C, window: usize) -> Self {
        let state = ClockOffsets::new(window, clock.generation());
        Self { clock, state }
    }

    pub fn clock(&self) -> &C {
        &self.clock
    }

    pub fn offset(&self) -> Option<i64> {
        self.state.offset()
    }

    pub fn offsets(&self) -> &VecDeque<i64> {
        self.state.offsets()
    }

    /// Records the receipt of a device timestamp now.
    pub fn observe(&mut self, device_us: i64) {
        let generation = self.clock.generation();
        self.state.observe(generation, self.clock.now_us(), device_us);
    }

    /// Host time of a device timestamp, never later than the host clock's current time.
    pub fn map(&self, device_us: i64) -> i64 {
        let now = self.clock.now_us();
        self.state.map(now, self.clock.generation(), device_us)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    struct Script(std::sync::Mutex<RefCell<Vec<i64>>>);

    impl HostClock for Script {
        fn now_us(&self) -> i64 {
            self.0.lock().unwrap().borrow_mut().remove(0)
        }
    }

    fn script(values: &[i64]) -> Script {
        Script(std::sync::Mutex::new(RefCell::new(values.to_vec())))
    }

    #[test]
    fn uses_the_smallest_receipt_delay() {
        let mut mapper = ClockMapper::new(script(&[1_000_600, 1_031_000, 1_060_550, 2_000_000]));
        for device_us in [500, 30_500, 60_500] {
            mapper.observe(device_us);
        }
        assert_eq!(mapper.map(90_500), 1_090_550);
    }

    #[test]
    fn never_maps_into_the_future() {
        let mut mapper = ClockMapper::new(script(&[1_000, 1_010]));
        mapper.observe(0);
        assert_eq!(mapper.map(500), 1_010);
    }

    #[test]
    fn window_forgets_old_offsets() {
        let mut mapper = ClockMapper::with_window(script(&[100, 1_000, 2_000, 5_000]), 2);
        for _ in 0..3 {
            mapper.observe(0);
        }
        assert_eq!(mapper.map(0), 1_000);
    }

    struct Switching {
        now: std::sync::atomic::AtomicI64,
        generation: std::sync::atomic::AtomicI64,
    }

    impl HostClock for Switching {
        fn now_us(&self) -> i64 {
            self.now.load(std::sync::atomic::Ordering::SeqCst)
        }

        fn generation(&self) -> Option<i64> {
            Some(self.generation.load(std::sync::atomic::Ordering::SeqCst))
        }
    }

    #[test]
    fn a_new_clock_generation_starts_over() {
        use std::sync::atomic::Ordering::SeqCst;
        let clock = Switching { now: 1_000.into(), generation: 0.into() };
        let mut mapper = ClockMapper::new(clock);
        mapper.observe(0);
        mapper.clock().generation.store(1, SeqCst);
        mapper.clock().now.store(50_000, SeqCst);
        assert_eq!(mapper.map(10), 50_000);
        mapper.observe(40_000);
        assert_eq!(mapper.map(40_000), 50_000);
        assert_eq!(mapper.offset(), Some(10_000));
    }
}
