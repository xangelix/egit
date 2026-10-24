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

use std::future::Future;
use std::sync::{Arc, OnceLock};

use tokio::sync::{Mutex, MutexGuard};

use crate::problem::Problem;

/// How many jobs run at once (each is usually one git process); more wait their turn.
const MAX_JOBS: usize = 16;

static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

/// Creates the application's runtime and hands it to `egui-async`, once per process (a second
/// application in the same process, as in tests, shares the first one's).
///
/// # Errors
///
/// When the system can't start the runtime's threads.
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
pub(crate) fn detached(work: impl Future<Output = ()> + Send + 'static) {
    match RUNTIME.get() {
        // The handle is dropped: the work runs to its end on its own.
        Some(runtime) => drop(runtime.spawn_blocking(move || egit_git::block_on(work))),
        None => egit_git::block_on(work),
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
