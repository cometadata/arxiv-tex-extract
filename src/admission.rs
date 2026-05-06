//! Counting-semaphore admission control for per-paper extractor threads.
//!
//! At most `max_inflight` extractor threads hold in-flight allocations at
//! any time, bounding peak RSS regardless of rayon worker count.

use std::sync::{Arc, Condvar, Mutex};

/// Fixed-capacity counting semaphore. `acquire_owned` blocks until a
/// permit is available; the returned `OwnedAdmissionPermit` releases the
/// permit on drop.
pub struct AdmissionControl {
    permits: Mutex<usize>,
    cv: Condvar,
}

impl AdmissionControl {
    pub fn new(n: usize) -> Self {
        Self {
            permits: Mutex::new(n),
            cv: Condvar::new(),
        }
    }

    /// Blocking acquire. Moves an Arc clone into the permit so it can
    /// cross thread boundaries (`thread::spawn` requires 'static).
    pub fn acquire_owned(self: &Arc<Self>) -> OwnedAdmissionPermit {
        let mut p = self.permits.lock().unwrap();
        while *p == 0 {
            p = self.cv.wait(p).unwrap();
        }
        *p -= 1;
        OwnedAdmissionPermit {
            ac: Arc::clone(self),
        }
    }

    /// Snapshot the current free-permit count. Only useful for tests and
    /// instrumentation — the value may change before the caller acts on it.
    pub fn available_permits(&self) -> usize {
        *self.permits.lock().unwrap()
    }
}

/// RAII permit that releases itself on drop (normal completion, panic, or
/// early return), so a panicking extractor thread cannot deadlock later
/// acquirers.
pub struct OwnedAdmissionPermit {
    ac: Arc<AdmissionControl>,
}

impl Drop for OwnedAdmissionPermit {
    fn drop(&mut self) {
        *self.ac.permits.lock().unwrap() += 1;
        self.ac.cv.notify_one();
    }
}
