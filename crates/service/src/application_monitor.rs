//! Scheduling policy is separate from network work and never installs an update.
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Clone)]
pub(crate) struct UserActivity(Arc<Mutex<Instant>>);
impl Default for UserActivity {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(Instant::now())))
    }
}
impl UserActivity {
    pub fn touch(&self) {
        if let Ok(mut last) = self.0.lock() {
            *last = Instant::now();
        }
    }
    pub fn idle_for(&self) -> Duration {
        self.0.lock().map(|last| last.elapsed()).unwrap_or_default()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    Download,
    Probe,
}

pub(crate) struct Schedule {
    startup: bool,
    after: Instant,
}
impl Schedule {
    pub fn new(now: Instant) -> Self {
        Self {
            startup: true,
            after: now + Duration::from_secs(3),
        }
    }
    pub fn due(&self, now: Instant, idle: Duration, free: bool, pending: bool) -> Option<Mode> {
        if !free || now < self.after {
            return None;
        }
        if self.startup && !pending {
            Some(Mode::Download)
        } else if idle >= Duration::from_secs(60) {
            Some(Mode::Probe)
        } else {
            None
        }
    }
    pub fn attempted(&mut self, now: Instant, success: bool) {
        self.startup = false;
        self.after = now + Duration::from_secs(if success { 4 * 3600 } else { 30 * 60 });
    }
}
