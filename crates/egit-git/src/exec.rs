//! Running egit's futures.
//!
//! Every function here that runs git is `async`, so that one source serves both kinds of host:
//! in a browser, where a command is a message to a relay and its answer is awaited, and on a
//! computer of its own, where a command's process is run and read as it always was. There the
//! futures do their work while they are polled, blocking on git's pipes, so they are polled on
//! threads made for blocking (a job's, or one of their own) through [`block_on`], never on the
//! interface's.

use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::Duration;

/// Wakes the thread [`block_on`] parked.
struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}

/// Runs `future` to its end on this thread, parking it while the future waits.
pub fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let waker = Waker::from(Arc::new(Unpark(thread::current())));
    let mut context = Context::from_waker(&waker);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }
        // Woken spuriously, it polls again: a future says itself whether it is done.
        thread::park();
    }
}

/// [`block_on`] as a method: `fetch(…).wait()`, for tests and for threads of their own.
pub trait Wait: Future + Sized {
    /// Runs the future to its end on this thread.
    fn wait(self) -> Self::Output {
        block_on(self)
    }
}

impl<F: Future> Wait for F {}

/// Waits `duration`.
#[allow(
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "here it blocks; through a relay it will await"
)]
pub async fn sleep(duration: Duration) {
    thread::sleep(duration);
}

/// Lets other work go on before going on: in a browser, the page draws and answers meanwhile.
/// Here, where work has threads of its own, nothing.
#[expect(
    clippy::unused_async,
    reason = "a browser's yields; here there is nothing to wait for"
)]
pub async fn yield_now() {}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU8, Ordering};

    use super::*;

    /// Pending twice, waking itself from another thread each time.
    struct Twice(Arc<AtomicU8>);

    impl Future for Twice {
        type Output = u8;

        fn poll(self: std::pin::Pin<&mut Self>, context: &mut Context<'_>) -> Poll<u8> {
            let polls = self.0.fetch_add(1, Ordering::AcqRel);
            if polls >= 2 {
                return Poll::Ready(polls);
            }
            let waker = context.waker().clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(5));
                waker.wake();
            });
            Poll::Pending
        }
    }

    #[test]
    fn a_future_woken_elsewhere_runs_to_its_end() {
        let polls = Arc::new(AtomicU8::new(0));
        assert_eq!(Twice(Arc::clone(&polls)).wait(), 2);
        assert_eq!(polls.load(Ordering::Acquire), 3);
        assert_eq!(block_on(async { 7 }), 7);
    }
}
