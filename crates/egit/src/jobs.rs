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
//! In a browser a job's future is the page's (`egui-async` spawns it there): git is a relay's
//! messages awaited, nothing blocks, so it runs where it is awaited; detached work is spawned on
//! the page too. A long computation there shares the page's thread through a [`Pace`].

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

/// In a browser, `work` itself: it never blocks.
#[cfg(target_arch = "wasm32")]
pub(crate) async fn blocking<T, E>(
    work: impl Future<Output = Result<T, E>> + Send + 'static,
) -> Result<T, E>
where
    T: Send + 'static,
    E: From<Problem> + Send + 'static,
{
    work.await
}

/// In a browser, `work` spawned on the page.
#[cfg(target_arch = "wasm32")]
pub(crate) fn detached(work: impl Future<Output = ()> + Send + 'static) {
    wasm_bindgen_futures::spawn_local(work);
}

/// How long a computation runs in a browser before the page has its thread back.
const SLICE: Duration = Duration::from_millis(10);

/// How many steps go by between looks at the clock (each a call into the page).
const STEPS_A_LOOK: u32 = 256;

/// Lets a long computation share the thread it runs on: in a browser, the page's one, it
/// yields every [`SLICE`], so the page draws and answers meanwhile; on a computer, where a
/// job has a thread of its own, it never waits.
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
