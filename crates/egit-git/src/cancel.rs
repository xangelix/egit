//! Cancelling running git commands.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use web_time::Instant;

use parking_lot::{Condvar, Mutex};

use crate::host::{RemotePid, SshHost};
use crate::process::{self, Group};
use crate::relay::Relay;

/// A command running on another computer: where, and its process id there once it said it.
#[derive(Clone, Debug)]
pub(crate) struct Elsewhere {
    pub(crate) host: SshHost,
    pub(crate) pid: RemotePid,
}

impl Elsewhere {
    /// Asks its process group there to stop, from a thread of its own (a command there): once
    /// its id is known (waiting a little: it may still be on its way), then the session here,
    /// `group`, if `shared` still runs it. The session is left alive until then, so the id
    /// can still arrive through it.
    fn terminate(&self, shared: Arc<Shared>, group: Group) {
        let elsewhere = self.clone();
        let spawned = std::thread::Builder::new()
            .name("stopping git elsewhere".to_owned())
            .spawn(move || {
                let until = Instant::now().checked_add(Duration::from_secs(3));
                loop {
                    if let Some(&pid) = elsewhere.pid.get() {
                        elsewhere.host.terminate(pid);
                        break;
                    }
                    if until.is_none_or(|until| Instant::now() >= until) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                // Still registered: its leader hasn't been reaped, so the id is still its.
                let running = shared.running.lock();
                if running
                    .iter()
                    .any(|(_, running)| matches!(running, Running::Group(running, _) if *running == group))
                {
                    process::terminate_group(group);
                }
            });
        if let Err(error) = spawned {
            tracing::warn!(%error, "couldn't start stopping git elsewhere");
            process::terminate_group(group);
        }
    }
}

/// Cancels the git commands run with it.
///
/// Cancelling sends SIGTERM to each running command's process group (git, its hooks, ssh, and
/// helpers; through a relay, it asks the relay to), which lets git remove its lock files; a
/// command started after the token was cancelled stops as soon as it starts. Every command that observes the cancellation returns
/// [`GitError::Cancelled`](crate::GitError::Cancelled).
///
/// Clones share the same state, so one can be handed to the interface while another runs the
/// work.
#[derive(Clone, Debug, Default)]
pub struct CancelToken(Arc<Shared>);

#[derive(Debug, Default)]
struct Shared {
    cancelled: AtomicBool,
    /// What runs under this token right now, each under the number of its registration.
    running: Mutex<Vec<(u64, Running)>>,
    /// The number of the next registration.
    next: AtomicU64,
    /// Wakes [`CancelToken::sleep`] when the token is cancelled.
    woken: Condvar,
}

/// A command running under a token.
#[derive(Debug)]
enum Running {
    /// A process group here (and, for a command on another computer, where): removed only after
    /// its leader has exited and before it is reaped, so its id here is never a reused one.
    Group(Group, Option<Elsewhere>),
    /// A process a relay runs, by its id there.
    Relayed(Relay, u32),
    /// A program other than git (`Git::run_there`), wherever it runs.
    Program(crate::running::Stopping),
}

impl Running {
    /// Asks it to stop.
    fn stop(&self, shared: &Arc<Shared>) {
        match self {
            // There first: the session here ending doesn't stop it.
            Self::Group(group, Some(elsewhere)) => {
                elsewhere.terminate(Arc::clone(shared), *group);
            }
            Self::Group(group, None) => process::terminate_group(*group),
            Self::Relayed(relay, id) => relay.terminate(*id),
            Self::Program(stopping) => stopping.stop(),
        }
    }
}

impl CancelToken {
    /// A token that hasn't been cancelled.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Cancels every command running under this token, and every one started later.
    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::Release);
        let running = self.0.running.lock();
        for (_, running) in running.iter() {
            running.stop(&self.0);
        }
        drop(running);
        self.0.woken.notify_all();
    }

    /// Whether [`Self::cancel`] has been called.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::Acquire)
    }

    /// Waits for `duration`, or less if the token is cancelled meanwhile. Returns whether it was
    /// cancelled.
    #[cfg(target_arch = "wasm32")]
    pub(crate) async fn sleep(&self, duration: Duration) -> bool {
        // The page's one thread can't wait for a cancel to wake it: it looks, now and then.
        const LOOKS: Duration = Duration::from_millis(25);
        let deadline = Instant::now().checked_add(duration);
        while !self.is_cancelled() {
            let left = deadline.map_or(LOOKS, |deadline| {
                deadline.saturating_duration_since(Instant::now())
            });
            if left.is_zero() {
                break;
            }
            crate::exec::sleep(left.min(LOOKS)).await;
        }
        self.is_cancelled()
    }

    /// Waits for `duration`, or less if the token is cancelled meanwhile. Returns whether it was
    /// cancelled.
    #[cfg(not(target_arch = "wasm32"))]
    #[allow(
        clippy::unused_async,
        clippy::unused_async_trait_impl,
        reason = "here it blocks; through a relay it will await"
    )]
    pub(crate) async fn sleep(&self, duration: Duration) -> bool {
        // The condition variable's clock, the standard library's.
        let deadline = std::time::Instant::now().checked_add(duration);
        let mut running = self.0.running.lock();
        while !self.is_cancelled() {
            if let Some(deadline) = deadline {
                if self.0.woken.wait_until(&mut running, deadline).timed_out() {
                    break;
                }
            } else {
                // A duration past the end of time: wait for the cancel alone.
                self.0.woken.wait(&mut running);
            }
        }
        drop(running);
        self.is_cancelled()
    }

    /// Records a process group running under this token until the returned guard is dropped.
    ///
    /// Drop the guard after the group's leader has exited and before reaping it.
    pub(crate) fn register(&self, group: Group, elsewhere: Option<Elsewhere>) -> Registration<'_> {
        self.record(Running::Group(group, elsewhere))
    }

    /// Records a process a relay runs under this token until the returned guard is dropped.
    pub(crate) fn register_relayed(&self, process: &crate::relay::Process) -> Registration<'_> {
        self.record(Running::Relayed(process.relay().clone(), process.id()))
    }

    /// Records a program other than git running under this token until the returned guard is
    /// dropped.
    pub(crate) fn register_program(&self, stopping: crate::running::Stopping) -> Registration<'_> {
        self.record(Running::Program(stopping))
    }

    fn record(&self, running: Running) -> Registration<'_> {
        let number = self.0.next.fetch_add(1, Ordering::Relaxed);
        let mut list = self.0.running.lock();
        // A cancel that came between the check before starting and now: it may not have seen
        // this one, so stop it here.
        if self.is_cancelled() {
            running.stop(&self.0);
        }
        list.push((number, running));
        drop(list);
        Registration {
            token: self,
            number,
        }
    }
}

/// Something running under a [`CancelToken`].
#[must_use = "it stays stoppable only while the registration lives"]
pub(crate) struct Registration<'a> {
    token: &'a CancelToken,
    number: u64,
}

impl Drop for Registration<'_> {
    fn drop(&mut self) {
        let mut running = self.token.0.running.lock();
        if let Some(index) = running
            .iter()
            .position(|(number, _)| *number == self.number)
        {
            running.swap_remove(index);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::Wait as _;

    #[test]
    fn starts_uncancelled_and_clones_share_state() {
        let token = CancelToken::new();
        let clone = token.clone();
        assert!(!token.is_cancelled());
        clone.cancel();
        assert!(token.is_cancelled());
    }

    #[test]
    fn sleep_runs_its_course_without_a_cancel() {
        let token = CancelToken::new();
        let started = Instant::now();
        assert!(!token.sleep(Duration::from_millis(20)).wait());
        assert!(started.elapsed() >= Duration::from_millis(20));
    }

    #[test]
    fn cancel_wakes_a_sleeper() {
        let token = CancelToken::new();
        let sleeper = {
            let token = token.clone();
            std::thread::spawn(move || {
                let started = Instant::now();
                (
                    token.sleep(Duration::from_secs(60)).wait(),
                    started.elapsed(),
                )
            })
        };
        std::thread::sleep(Duration::from_millis(20));
        token.cancel();
        let (cancelled, slept) = sleeper.join().unwrap();
        assert!(cancelled);
        assert!(slept < Duration::from_secs(10));
    }

    #[test]
    fn a_cancelled_token_sleeps_not_at_all() {
        let token = CancelToken::new();
        token.cancel();
        assert!(token.sleep(Duration::from_secs(60)).wait());
    }
}
