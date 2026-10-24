//! Running egit's futures.
//!
//! Every function here that runs git is `async`, so that one source serves both kinds of host:
//! in a browser, where a command is a message to a relay and its answer is awaited, and on a
//! computer of its own, where a command's process is run and read as it always was. There the
//! futures do their work while they are polled, blocking on git's pipes, so they are polled on
//! threads made for blocking (a job's, or one of their own) through [`block_on`], never on the
//! interface's. In a browser nothing blocks: work is spawned on the page, and waits are the
//! page's timers.

use std::future::Future;
#[cfg(not(target_arch = "wasm32"))]
use std::pin::pin;
use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use std::task::Wake;
use std::task::{Context, Poll, Waker};
#[cfg(not(target_arch = "wasm32"))]
use std::thread::{self, Thread};
use std::time::Duration;

/// Wakes the thread [`block_on`] parked.
#[cfg(not(target_arch = "wasm32"))]
struct Unpark(Thread);

#[cfg(not(target_arch = "wasm32"))]
impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}

/// Runs `future` to its end on this thread, parking it while the future waits.
#[cfg(not(target_arch = "wasm32"))]
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
#[cfg(not(target_arch = "wasm32"))]
pub trait Wait: Future + Sized {
    /// Runs the future to its end on this thread.
    fn wait(self) -> Self::Output {
        block_on(self)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl<F: Future> Wait for F {}

/// Runs `future` to its end apart, on a thread of its own named `name` (in a browser, a task
/// of the page's).
///
/// # Errors
///
/// When the thread can't be started.
#[cfg(not(target_arch = "wasm32"))]
pub fn spawn(name: &str, future: impl Future<Output = ()> + Send + 'static) -> std::io::Result<()> {
    thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || block_on(future))
        .map(drop)
}

/// Waits `duration`.
#[cfg(not(target_arch = "wasm32"))]
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
#[cfg(not(target_arch = "wasm32"))]
#[expect(
    clippy::unused_async,
    reason = "a browser's yields; here there is nothing to wait for"
)]
pub async fn yield_now() {}

/// In a browser, `future` spawned on the page (`name` is a thread's, and the page has one).
///
/// # Errors
///
/// Never: as a thread, which may not start.
#[cfg(target_arch = "wasm32")]
pub fn spawn(
    _name: &str,
    future: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    wasm_bindgen_futures::spawn_local(future);
    Ok(())
}

/// In a browser, a page's timer of `duration`.
#[cfg(target_arch = "wasm32")]
pub async fn sleep(duration: Duration) {
    Timer::new(duration).await;
}

/// In a browser, a timer of none: the page draws, and answers what came, before this goes on.
#[cfg(target_arch = "wasm32")]
pub async fn yield_now() {
    Timer::new(Duration::ZERO).await;
}

/// A page's timer, as a future: what it holds is shared with the page's callback (handed to
/// the page, which drops it once called), never the page's own objects, so it is `Send` as
/// every egit future is.
#[cfg(target_arch = "wasm32")]
struct Timer {
    after: Duration,
    state: Option<Arc<parking_lot::Mutex<Rang>>>,
}

#[cfg(target_arch = "wasm32")]
#[derive(Default)]
struct Rang {
    rang: bool,
    waker: Option<Waker>,
}

#[cfg(target_arch = "wasm32")]
impl Timer {
    const fn new(after: Duration) -> Self {
        Self { after, state: None }
    }
}

#[cfg(target_arch = "wasm32")]
impl Future for Timer {
    type Output = ();

    fn poll(mut self: std::pin::Pin<&mut Self>, context: &mut Context<'_>) -> Poll<()> {
        let after = self.after;
        let state = self.state.get_or_insert_with(|| {
            let state = Arc::new(parking_lot::Mutex::new(Rang::default()));
            let ringing = Arc::clone(&state);
            let callback = wasm_bindgen::closure::Closure::once_into_js(move || {
                let mut rang = ringing.lock();
                rang.rang = true;
                if let Some(waker) = rang.waker.take() {
                    waker.wake();
                }
            });
            let millis = i32::try_from(after.as_millis()).unwrap_or(i32::MAX);
            let set = web_sys::window().map(|window| {
                window.set_timeout_with_callback_and_timeout_and_arguments_0(
                    wasm_bindgen::JsCast::unchecked_ref(&callback),
                    millis,
                )
            });
            // No window (a worker): it rings at once rather than never.
            if !matches!(set, Some(Ok(_))) {
                state.lock().rang = true;
            }
            state
        });
        let mut rang = state.lock();
        if rang.rang {
            return Poll::Ready(());
        }
        rang.waker = Some(context.waker().clone());
        Poll::Pending
    }
}

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
