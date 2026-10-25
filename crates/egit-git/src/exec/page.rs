//! The page's threads, in a browser: its main one, which draws and alone touches the page's own
//! objects (its document, its WebSocket), and a pool of workers (web workers sharing the page's
//! WebAssembly and memory) running egit's work, so the main thread never does.
//!
//! Work spawned ([`spawn`]) goes to the pool, each piece a task of the worker that takes it; a
//! worker takes the next while the pieces it has wait (on the relay, a timer), so one busy
//! computing leaves the rest to the others. What only the main thread may do is handed to it
//! ([`on_main`]): done at once there, else as soon as it is free. Waits are the page's timers,
//! which a worker has too.

use std::cell::Cell;
use std::collections::VecDeque;
use std::future::{Future, poll_fn};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use wasm_bindgen::JsCast as _;
use wasm_bindgen::prelude::*;

use crate::sync::Mutex;

/// A piece of work for the pool.
type Job = Pin<Box<dyn Future<Output = ()> + Send>>;

/// What the main thread is handed.
type ForMain = Box<dyn FnOnce() + Send>;

/// The pool's work not yet taken, and the workers waiting for some.
struct Pool {
    jobs: VecDeque<Job>,
    idle: Vec<Waker>,
}

static POOL: Mutex<Pool> = Mutex::new(Pool {
    jobs: VecDeque::new(),
    idle: Vec::new(),
});

/// What the main thread was handed, and its server's waker.
struct Main {
    work: VecDeque<ForMain>,
    waker: Option<Waker>,
}

static MAIN: Mutex<Main> = Mutex::new(Main {
    work: VecDeque::new(),
    waker: None,
});

thread_local! {
    /// Whether this thread is the page's main one.
    static IS_MAIN: Cell<bool> = const { Cell::new(false) };
}

#[wasm_bindgen]
extern "C" {
    /// The global `setTimeout`: the window's, or a worker's own.
    #[wasm_bindgen(catch, js_name = setTimeout)]
    fn set_timeout(handler: &JsValue, milliseconds: i32) -> Result<JsValue, JsValue>;
}

/// Whether this is the page's main thread (and so may never wait).
#[must_use]
pub fn is_main() -> bool {
    IS_MAIN.with(Cell::get)
}

/// Makes this thread the page's main one and serves what other threads hand it, from now on:
/// called once, from the page's start.
pub fn serve_main() {
    IS_MAIN.with(|main| main.set(true));
    wasm_bindgen_futures::spawn_local(async {
        loop {
            let handed = poll_fn(|context| {
                let mut main = MAIN.lock();
                if main.work.is_empty() {
                    main.waker = Some(context.waker().clone());
                    return Poll::Pending;
                }
                Poll::Ready(std::mem::take(&mut main.work))
            })
            .await;
            for work in handed {
                work();
            }
        }
    });
}

/// Runs `work` on the page's main thread: at once there, else as soon as it is free (in the
/// order handed).
pub fn on_main(work: impl FnOnce() + Send + 'static) {
    if is_main() {
        work();
        return;
    }
    let waker = {
        let mut main = MAIN.lock();
        main.work.push_back(Box::new(work));
        main.waker.take()
    };
    if let Some(waker) = waker {
        waker.wake();
    }
}

/// Starts `count` workers, each running `script`.
///
/// The script is a module that hands the message it is sent (the page's WebAssembly module and
/// memory) to wasm-bindgen's `init`, then calls the page's entry for a worker, which calls
/// [`work`].
///
/// # Errors
///
/// When the page can't make a worker, in its words.
pub fn start_workers(script: &str, count: usize) -> Result<(), String> {
    let options = web_sys::WorkerOptions::new();
    options.set_type(web_sys::WorkerType::Module);
    let failed = |error: JsValue| format!("{error:?}");
    for number in 0..count {
        options.set_name(&format!("egit {number}"));
        let worker = web_sys::Worker::new_with_options(script, &options).map_err(failed)?;
        let said = Closure::<dyn FnMut(web_sys::ErrorEvent)>::new(|event: web_sys::ErrorEvent| {
            tracing::error!(message = event.message(), "a worker of egit's failed");
        });
        worker.set_onerror(Some(said.as_ref().unchecked_ref()));
        // Kept for the worker's life, which is the page's.
        said.forget();
        let message = js_sys::Array::of2(&wasm_bindgen::module(), &wasm_bindgen::memory());
        worker.post_message(&message).map_err(failed)?;
    }
    Ok(())
}

/// A worker's own, once its WebAssembly runs: takes the pool's work, each piece a task of its
/// own, for the worker's life.
pub fn work() {
    wasm_bindgen_futures::spawn_local(async {
        loop {
            let job = poll_fn(|context| {
                let mut pool = POOL.lock();
                if let Some(job) = pool.jobs.pop_front() {
                    return Poll::Ready(job);
                }
                if !pool.idle.iter().any(|idle| idle.will_wake(context.waker())) {
                    pool.idle.push(context.waker().clone());
                }
                Poll::Pending
            })
            .await;
            wasm_bindgen_futures::spawn_local(job);
        }
    });
}

/// In a browser, `future` handed to the page's workers (`name` is a thread's, where work has
/// threads of its own).
///
/// # Errors
///
/// Never: as a thread, which may not start.
pub fn spawn(
    _name: &str,
    future: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    let idle = {
        let mut pool = POOL.lock();
        pool.jobs.push_back(Box::pin(future));
        pool.idle.pop()
    };
    if let Some(idle) = idle {
        idle.wake();
    }
    Ok(())
}

/// In a browser, a page's timer of `duration`.
pub async fn sleep(duration: Duration) {
    Timer::new(duration).await;
}

/// In a browser, a timer of none: what else this thread has to do (drawing, on the main one;
/// other work, on a worker) is done before this goes on.
pub async fn yield_now() {
    Timer::new(Duration::ZERO).await;
}

/// A timer of the thread's, as a future: what it holds is shared with the timer's callback
/// (handed to the page, which drops it once called), never the page's own objects, so it is
/// `Send` as every egit future is.
struct Timer {
    after: Duration,
    state: Option<Arc<Mutex<Rang>>>,
}

/// Whether a timer rang, and the task waiting for it.
#[derive(Default)]
struct Rang {
    rang: bool,
    waker: Option<Waker>,
}

impl Timer {
    const fn new(after: Duration) -> Self {
        Self { after, state: None }
    }
}

impl Future for Timer {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<()> {
        let after = self.after;
        let state = self.state.get_or_insert_with(|| {
            let state = Arc::new(Mutex::new(Rang::default()));
            let ringing = Arc::clone(&state);
            let callback = Closure::once_into_js(move || {
                let waker = {
                    let mut rang = ringing.lock();
                    rang.rang = true;
                    rang.waker.take()
                };
                if let Some(waker) = waker {
                    waker.wake();
                }
            });
            let milliseconds = i32::try_from(after.as_millis()).unwrap_or(i32::MAX);
            // Not set (a page that refuses timers): it rings at once rather than never.
            if set_timeout(&callback, milliseconds).is_err() {
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
