//! Screens from the flow thread to the window, acknowledgements and actions back.
//!
//! The flow publishes screens ([`UiChannel::show`]); every screen has a sequence number and
//! the window acknowledges it once drawn, so the flow can wait for a calibration target to
//! be on screen before collecting. Values that change many times a second (the eye
//! positions) go into `live` without a new screen. The window reports user actions with the
//! screen they were made on; actions for an older screen are ignored, except
//! [`Action::Cancel`], which stands until the flow handles it. The window polls
//! [`UiChannel::snapshot`] from the GTK main loop, which also tells the flow that the window
//! is still there.

use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use thiserror::Error;

use crate::screen::{Action, EyesLive, Screen, ScreenKind, View, fills_screen};

pub const PAGE_GONE_S: f64 = 20.0;
const WAIT_SLICE: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum ChannelError {
    #[error("Cancelled in the setup window")]
    Cancelled,
    #[error("{0}")]
    WindowGone(String),
    #[error("{0}")]
    Timeout(String),
}

/// One report from the window: an acknowledgement or an action for screen `seq`, with the
/// window's view and the monitor it is on.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WindowEvent {
    pub seq: Option<u64>,
    pub ack: bool,
    pub action: Option<Action>,
    pub view: Option<View>,
    pub monitor: Option<String>,
}

/// What the window polls: the current screen and the live values.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub screen: Screen,
    pub eyes: Option<EyesLive>,
}

#[derive(Debug)]
struct State {
    screen: Screen,
    eyes: Option<EyesLive>,
    acked: u64,
    actions: Vec<Action>,
    view: Option<View>,
    last_contact: Option<Instant>,
    cancel_requested: bool,
    monitor: Option<String>,
    window_monitor: Option<String>,
}

/// What a wait ended with.
#[derive(Debug, Clone, PartialEq)]
pub enum Waited<T> {
    Action(Action),
    Ready(T),
}

/// The channel between the flow thread and the window; cheap to clone, safe across threads.
#[derive(Debug, Clone)]
pub struct UiChannel {
    shared: Arc<(Mutex<State>, Condvar)>,
}

impl Default for UiChannel {
    fn default() -> Self {
        Self::new()
    }
}

impl UiChannel {
    pub fn new() -> Self {
        let state = State {
            screen: Screen { seq: 0, monitor: None, kind: ScreenKind::Connecting },
            eyes: None,
            acked: 0,
            actions: Vec::new(),
            view: None,
            last_contact: None,
            cancel_requested: false,
            monitor: None,
            window_monitor: None,
        };
        Self { shared: Arc::new((Mutex::new(state), Condvar::new())) }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.shared.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn notify(&self) {
        self.shared.1.notify_all();
    }

    /// Publishes a screen; pending actions of the previous one are dropped. Returns its seq.
    pub fn show(&self, kind: ScreenKind) -> u64 {
        let seq = {
            let mut state = self.lock();
            let seq = state.screen.seq + 1;
            state.screen = Screen { seq, monitor: state.monitor.clone(), kind };
            state.actions.clear();
            seq
        };
        self.notify();
        seq
    }

    pub fn set_eyes(&self, eyes: Option<EyesLive>) {
        self.lock().eyes = eyes;
    }

    pub fn current(&self) -> Screen {
        self.lock().screen.clone()
    }

    /// What the window draws; polling also tells the flow that the window is still there.
    pub fn snapshot(&self) -> Snapshot {
        let mut state = self.lock();
        state.last_contact = Some(Instant::now());
        Snapshot { screen: state.screen.clone(), eyes: state.eyes }
    }

    pub fn clear_cancel(&self) {
        self.lock().cancel_requested = false;
    }

    /// Accepts events only from a window placed on monitor `key` (`None`: from any window).
    pub fn follow_monitor(&self, key: Option<String>) {
        let mut state = self.lock();
        state.monitor = key;
        state.window_monitor = None;
    }

    pub fn monitor(&self) -> Option<String> {
        self.lock().monitor.clone()
    }

    pub fn window_monitor(&self) -> Option<String> {
        self.lock().window_monitor.clone()
    }

    /// An acknowledgement or action from the window; false for one made on an older screen
    /// or reported from another monitor than the one the setup follows.
    pub fn window_event(&self, event: WindowEvent) -> bool {
        let current = {
            let mut state = self.lock();
            if state.monitor.is_some() && event.monitor != state.monitor {
                return false;
            }
            state.last_contact = Some(Instant::now());
            state.window_monitor = event.monitor.clone();
            if event.view.is_some() {
                state.view = event.view;
            }
            if event.action == Some(Action::Cancel) {
                state.cancel_requested = true;
            }
            let current = event.seq == Some(state.screen.seq);
            if current {
                state.acked = state.acked.max(state.screen.seq);
                if let Some(action) = event.action {
                    state.actions.push(action);
                }
            }
            current
        };
        self.notify();
        current
    }

    pub fn has_action(&self, accept: impl Fn(&Action) -> bool) -> bool {
        self.lock().actions.iter().any(accept)
    }

    pub fn acked(&self) -> u64 {
        self.lock().acked
    }

    pub fn view(&self) -> Option<View> {
        self.lock().view
    }

    pub fn fullscreen(&self) -> bool {
        fills_screen(self.lock().view.as_ref())
    }

    /// Fails with [`ChannelError::Cancelled`] once the user cancelled, with
    /// [`ChannelError::WindowGone`] once the window stopped polling.
    pub fn check_cancel(&self) -> Result<(), ChannelError> {
        check(&self.lock())
    }

    /// Blocks until the window drew screen `seq`; `idle` runs between waits (it may read the device).
    pub fn wait_ack<E: From<ChannelError>>(
        &self,
        seq: u64,
        timeout: Duration,
        idle: Option<&mut dyn FnMut() -> Result<(), E>>,
    ) -> Result<(), E> {
        let failure = format!("The window did not show screen {seq}");
        let mut acked = |state: &mut State| (state.acked >= seq).then_some(Waited::Ready(()));
        self.wait(&mut acked, None, Some(timeout), idle, &failure).map(drop)
    }

    /// The first action `accept` takes (removed from the queue), or `ready()`'s value once it
    /// has one. A cancel ends the wait with [`ChannelError::Cancelled`]. `ready` runs without
    /// the channel's lock, so it may ask the channel itself.
    pub fn wait_action<T, E: From<ChannelError>>(
        &self,
        accept: impl Fn(&Action) -> bool,
        timeout: Option<Duration>,
        idle: Option<&mut dyn FnMut() -> Result<(), E>>,
        ready: Option<&mut dyn FnMut() -> Option<T>>,
    ) -> Result<Waited<T>, E> {
        let mut taken = |state: &mut State| {
            let at = state.actions.iter().position(&accept)?;
            Some(Waited::Action(state.actions.remove(at)))
        };
        self.wait(&mut taken, ready, timeout, idle, "No answer from the window")
    }

    fn wait<T, E: From<ChannelError>>(
        &self,
        locked: &mut dyn FnMut(&mut State) -> Option<Waited<T>>,
        mut ready: Option<&mut dyn FnMut() -> Option<T>>,
        timeout: Option<Duration>,
        mut idle: Option<&mut dyn FnMut() -> Result<(), E>>,
        failure: &str,
    ) -> Result<Waited<T>, E> {
        let deadline = timeout.map(|timeout| Instant::now() + timeout);
        loop {
            {
                let mut state = self.lock();
                check(&state)?;
                if let Some(result) = locked(&mut state) {
                    return Ok(result);
                }
            }
            if let Some(value) = ready.as_mut().and_then(|ready| ready()) {
                return Ok(Waited::Ready(value));
            }
            match idle.as_mut() {
                Some(idle) => idle()?,
                None => {
                    let remaining = deadline.map_or(WAIT_SLICE, |deadline| {
                        WAIT_SLICE.min(deadline.saturating_duration_since(Instant::now()))
                    });
                    let state = self.lock();
                    let (mut state, _) =
                        self.shared.1.wait_timeout(state, remaining).unwrap_or_else(PoisonError::into_inner);
                    check(&state)?;
                    if let Some(result) = locked(&mut state) {
                        return Ok(result);
                    }
                }
            }
            if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                return Err(ChannelError::Timeout(failure.to_string()).into());
            }
        }
    }
}

fn check(state: &State) -> Result<(), ChannelError> {
    if state.cancel_requested {
        return Err(ChannelError::Cancelled);
    }
    if state.last_contact.is_some_and(|contact| contact.elapsed().as_secs_f64() > PAGE_GONE_S) {
        return Err(ChannelError::WindowGone(format!("The setup window has not been seen for {PAGE_GONE_S:.0} s")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(seq: u64, action: Action) -> WindowEvent {
        WindowEvent { seq: Some(seq), action: Some(action), ..WindowEvent::default() }
    }

    #[test]
    fn actions_from_an_older_screen_are_ignored_but_cancel_stands() {
        let channel = UiChannel::new();
        let first = channel.show(ScreenKind::AspectWarning);
        channel.show(ScreenKind::CalibrationFailed);
        assert!(!channel.window_event(event(first, Action::Yes)));
        assert!(!channel.has_action(|a| *a == Action::Yes));
        channel.window_event(event(first, Action::Cancel));
        let waited: Result<Waited<()>, ChannelError> =
            channel.wait_action(|a| *a == Action::Retry, Some(Duration::from_secs(1)), None, None);
        assert_eq!(waited, Err(ChannelError::Cancelled));
    }

    #[test]
    fn acknowledgements_and_actions_of_the_current_screen_count() {
        let channel = UiChannel::new();
        let seq = channel.show(ScreenKind::Busy { title: "Saving".into() });
        assert!(channel.window_event(WindowEvent { seq: Some(seq), ack: true, ..WindowEvent::default() }));
        channel.wait_ack::<ChannelError>(seq, Duration::from_secs(1), None).unwrap();
        channel.window_event(event(seq, Action::Retry));
        let waited: Waited<()> =
            channel.wait_action::<(), ChannelError>(|a| *a == Action::Retry, None, None, None).unwrap();
        assert_eq!(waited, Waited::Action(Action::Retry));
    }

    #[test]
    fn a_wait_times_out_and_ready_ends_it() {
        let channel = UiChannel::new();
        let timeout = Some(Duration::from_millis(50));
        let waited: Result<Waited<()>, ChannelError> = channel.wait_action(|_| true, timeout, None, None);
        assert!(matches!(waited, Err(ChannelError::Timeout(_))));
        let mut calls = 0;
        let mut ready = || {
            calls += 1;
            (calls > 2).then_some(calls)
        };
        let mut idle = || Ok::<(), ChannelError>(());
        let waited = channel.wait_action(|_| false, None, Some(&mut idle), Some(&mut ready)).unwrap();
        assert_eq!(waited, Waited::Ready(3));
    }

    #[test]
    fn after_a_move_only_the_new_monitor_is_heard() {
        let channel = UiChannel::new();
        channel.follow_monitor(Some("HDMI-1".into()));
        let seq = channel.show(ScreenKind::Moving { to: "HDMI-1".into() });
        assert!(!channel.window_event(WindowEvent {
            seq: Some(seq),
            monitor: Some("DP-1".into()),
            ..Default::default()
        }));
        assert_eq!(channel.window_monitor(), None);
        assert!(channel.window_event(WindowEvent {
            seq: Some(seq),
            monitor: Some("HDMI-1".into()),
            ..Default::default()
        }));
        assert_eq!(channel.window_monitor().as_deref(), Some("HDMI-1"));
        assert_eq!(channel.current().monitor.as_deref(), Some("HDMI-1"));
    }
}
