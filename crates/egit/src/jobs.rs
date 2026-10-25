//! Work off the interface's thread.
//!
//! Every git command runs as a job: a future on Tokio's blocking pool, polled there to its end
//! (git's functions block on its pipes while they are polled), whose result reaches the
//! interface through an `egui-async` `Bind` polled each frame. The interface thread never waits
//! for git. The application owns one runtime ([`install_runtime`]); its blocking pool is capped,
//! so opening many repositories at once queues git processes rather than starting them all.
//!
//! Commands that write to a repository take its [`WriteLock`] inside their job, so they run one
//! at a time, in order; reads never wait for it.
//!
//! In a browser a job runs on one of the page's workers (`egit_git::exec`'s pool, web workers
//! sharing the page's memory), never its main thread, which only draws and polls the `Bind`
//! awaiting it; git there is a relay's messages awaited. A long computation shares its worker
//! with the worker's other jobs through a [`Pace`].

use std::future::Future;
use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::OnceLock;
use std::time::Duration;

use tokio::sync::{Mutex, MutexGuard};
use web_time::Instant;

use crate::problem::Problem;

/// How many jobs run at once (each is usually one git process); more wait their turn.
#[cfg(not(target_arch = "wasm32"))]
const MAX_JOBS: usize = 16;

#[cfg(not(target_arch = "wasm32"))]
static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

/// Creates the application's runtime and hands it to `egui-async`, once per process (a second
/// application in the same process, as in tests, shares the first one's).
///
/// # Errors
///
/// When the system can't start the runtime's threads.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn install_runtime() -> std::io::Result<()> {
    if RUNTIME.get().is_none() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .max_blocking_threads(MAX_JOBS)
            .thread_name("egit-job")
            .build()?;
        // Another thread may have won the race; its runtime is the one kept.
        let _ = RUNTIME.set(runtime);
    }
    if let Some(runtime) = RUNTIME.get() {
        // Already set to this same runtime by an earlier application.
        let _ = egui_async::set_global_runtime(runtime.handle().clone());
    }
    Ok(())
}

/// Runs `work` to its end on the blocking pool; the future resolves to its result.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) async fn blocking<T, E>(
    work: impl Future<Output = Result<T, E>> + Send + 'static,
) -> Result<T, E>
where
    T: Send + 'static,
    E: From<Problem> + Send + 'static,
{
    tokio::task::spawn_blocking(move || egit_git::block_on(work))
        .await
        .unwrap_or_else(|error| {
            tracing::error!(%error, "a job failed");
            Err(Problem::interrupted().into())
        })
}

/// Runs `work` to its end on the blocking pool without waiting for it (or here, without a
/// runtime).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn detached(work: impl Future<Output = ()> + Send + 'static) {
    match RUNTIME.get() {
        // The handle is dropped: the work runs to its end on its own.
        Some(runtime) => drop(runtime.spawn_blocking(move || egit_git::block_on(work))),
        None => egit_git::block_on(work),
    }
}

/// In a browser, nothing: the page runs every future.
#[cfg(target_arch = "wasm32")]
#[expect(
    clippy::unnecessary_wraps,
    reason = "as a computer's, which can fail to start"
)]
pub(crate) const fn install_runtime() -> std::io::Result<()> {
    Ok(())
}

/// In a browser, `work` run by one of the page's workers, never its main thread; the future
/// resolves to its result.
#[cfg(target_arch = "wasm32")]
pub(crate) async fn blocking<T, E>(
    work: impl Future<Output = Result<T, E>> + Send + 'static,
) -> Result<T, E>
where
    T: Send + 'static,
    E: From<Problem> + Send + 'static,
{
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let spawned = egit_git::exec::spawn("job", async move {
        // Nobody waits for it any more: nothing to tell.
        let _told = sender.send(work.await);
    });
    if let Err(error) = spawned {
        tracing::error!(%error, "a job couldn't be started");
        return Err(Problem::interrupted().into());
    }
    receiver.await.unwrap_or_else(|_| {
        tracing::error!("a job ended without its result");
        Err(Problem::interrupted().into())
    })
}

/// In a browser, `work` handed to one of the page's workers, not waited for.
#[cfg(target_arch = "wasm32")]
pub(crate) fn detached(work: impl Future<Output = ()> + Send + 'static) {
    if let Err(error) = egit_git::exec::spawn("job", work) {
        tracing::error!(%error, "a job couldn't be started");
    }
}

/// How long a computation runs in a browser before its worker's other work has the thread.
const SLICE: Duration = Duration::from_millis(10);

/// How many steps go by between looks at the clock (each a call into the page).
const STEPS_A_LOOK: u32 = 256;

/// Lets a long computation share the thread it runs on: in a browser, a worker of the page's,
/// whose other work (waiting on the relay, mostly) it would hold up, it yields every [`SLICE`];
/// on a computer, where a job has a thread of its own, it never waits.
pub(crate) struct Pace {
    since: Instant,
    steps: u32,
}

impl Pace {
    /// Its slice begins now.
    pub(crate) fn new() -> Self {
        Self {
            since: Instant::now(),
            steps: 0,
        }
    }

    /// A step of the computation is done: yields, in a browser, once its slice is over.
    pub(crate) async fn step(&mut self) {
        if !crate::desktop::IN_BROWSER {
            return;
        }
        self.steps = self.steps.wrapping_add(1);
        if self.steps.is_multiple_of(STEPS_A_LOOK) && self.since.elapsed() >= SLICE {
            egit_git::exec::yield_now().await;
            self.since = Instant::now();
        }
    }
}

/// Serializes the commands that write to one place (a repository, or the user's configuration).
#[derive(Clone, Debug, Default)]
pub(crate) struct WriteLock(Arc<Mutex<()>>);

impl WriteLock {
    /// Waits for the commands queued before, inside a job (never on the interface's thread).
    pub(crate) async fn hold(&self) -> MutexGuard<'_, ()> {
        self.0.lock().await
    }
}
