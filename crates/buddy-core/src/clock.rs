use std::time::{Duration, Instant};

/// Monotonic session clock, started when the recording session begins.
///
/// Every capture event is stamped from this one clock. `Instant` cannot move
/// backwards when the system wall clock is adjusted, so audio chunks, frames
/// and bookmarks stay ordered relative to each other. No wall-clock time is
/// used anywhere in capture.
pub struct SessionClock {
    origin: Instant,
}

impl SessionClock {
    /// Start a new clock at the current instant.
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
        }
    }

    /// Time elapsed since construction. Monotonic: repeated reads never
    /// decrease, even if the operating system changes the wall clock.
    pub fn now(&self) -> Duration {
        self.origin.elapsed()
    }
}

impl Default for SessionClock {
    fn default() -> Self {
        Self::new()
    }
}
