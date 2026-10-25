//! egit's locks: `parking_lot`'s.
//!
//! In a browser, egit's page runs threaded (workers sharing its memory), and its main thread may
//! never wait: a browser traps `memory.atomic.wait` there. A lock taken on the main thread spins
//! there until it is free, as the standard library's own allocator's does on the page, while a
//! worker parks as `parking_lot`'s do (by its parker for shared memory, `memory.atomic.wait32`,
//! which its `nightly` feature chooses). Locks are held for moments, never across an `.await`,
//! so the main thread spins for moments too.

#[cfg(not(target_arch = "wasm32"))]
pub use parking_lot::{Condvar, Mutex, MutexGuard};

#[cfg(target_arch = "wasm32")]
pub use page::{Mutex, MutexGuard, RawMutex};

/// The page's lock.
#[cfg(target_arch = "wasm32")]
#[allow(unsafe_code)]
mod page {
    /// A lock that never waits on the page's main thread.
    pub type Mutex<T> = lock_api::Mutex<RawMutex, T>;

    /// What holds a [`Mutex`].
    pub type MutexGuard<'a, T> = lock_api::MutexGuard<'a, RawMutex, T>;

    /// `parking_lot`'s lock, spun on the page's main thread rather than parked.
    pub struct RawMutex(parking_lot::RawMutex);

    // SAFETY: every operation is `parking_lot`'s own lock's, which keeps `lock_api::RawMutex`'s
    // contract; on the main thread `lock` only tries it again until it is taken, which takes it
    // exactly as parking_lot's `lock` does, without parking.
    unsafe impl lock_api::RawMutex for RawMutex {
        const INIT: Self = Self(parking_lot::RawMutex::INIT);

        type GuardMarker = lock_api::GuardNoSend;

        fn lock(&self) {
            if crate::exec::is_main() {
                while !self.0.try_lock() {
                    std::hint::spin_loop();
                }
            } else {
                self.0.lock();
            }
        }

        fn try_lock(&self) -> bool {
            self.0.try_lock()
        }

        unsafe fn unlock(&self) {
            // SAFETY: the caller holds the lock, as `lock_api` promises: `parking_lot`'s to
            // release.
            unsafe { self.0.unlock() };
        }

        fn is_locked(&self) -> bool {
            self.0.is_locked()
        }
    }
}
