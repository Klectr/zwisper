//! Runtime hot-swapping of strategies.

use arc_swap::ArcSwap;
use parking_lot::Mutex;
use std::sync::Arc;

/// Holds the current implementation of a port. A swap is atomic and happens
/// *between* jobs: a job that already grabbed `current()` keeps using the old
/// instance until it finishes; the next job gets the new one.
pub struct Swappable<T: ?Sized> {
    inner: ArcSwap<Mutex<Box<T>>>,
}

impl<T: ?Sized> Swappable<T> {
    pub fn new(value: Box<T>) -> Self {
        Swappable {
            inner: ArcSwap::from_pointee(Mutex::new(value)),
        }
    }

    /// The instance to use for one job.
    pub fn current(&self) -> Arc<Mutex<Box<T>>> {
        self.inner.load_full()
    }

    /// Replace the implementation.
    pub fn swap(&self, value: Box<T>) {
        self.inner.store(Arc::new(Mutex::new(value)));
    }

    /// Run `f` with exclusive access to the current implementation.
    pub fn with<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        let cur = self.current();
        let mut guard = cur.lock();
        f(&mut **guard)
    }
}
