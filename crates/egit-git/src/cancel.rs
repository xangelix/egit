//! Cancelling running git commands.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex};

use crate::host::{RemotePid, SshHost};
use crate::process::{self, Group};

/// A command running on another computer: where, and its process id there once it said it.
#[derive(Clone, Debug)]
pub(crate) struct Elsewhere {
    pub(crate) host: SshHost,
    pub(crate) pid: RemotePid,
}

impl Elsewhere {
    /// Asks its process group there to stop, from a thread of its own (a command there): once
    /// its id is known, waiting a little for it.
    fn terminate(&self) {
        let elsewhere = self.clone();
        let spawned = std::thread::Builder::new()
            .name("stopping git elsewhere".to_owned())
            .spawn(move || {
                let until = Instant::now().checked_add(Duration::from_secs(3));
                loop {
                    if let Some(&pid) = elsewhere.pid.get() {
                        elsewhere.host.terminate(pid);
                        return;
                    }
                    if until.is_none_or(|until| Instant::now() >= until) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            });
        if let Err(error) = spawned {
            tracing::warn!(%error, "couldn't start stopping git elsewhere");
        }
    }
}

/// Cancels the git commands run with it.
///
/// Cancelling sends SIGTERM to each running command's process group (git, its hooks, ssh, and
/// helpers), which lets git remove its lock files; a command started after the token was
/// cancelled stops as soon as it starts. Every command that observes the cancellation returns
/// [`GitError::Cancelled`](crate::GitError::Cancelled).
///
/// Clones share the same state, so one can be handed to the interface while another runs the
/// work.
#[derive(Clone, Debug, Default)]
pub struct CancelToken(Arc<Shared>);

#[derive(Debug, Default)]
struct Shared {
    cancelled: AtomicBool,
    /// The process groups running under this token right now (and, for a command on another
    /// computer, where). Each is removed only after its leader has exited and before it is
    /// reaped, so a group id here is never a reused one.
    running: Mutex<Vec<(Group, Option<Elsewhere>)>>,
    /// Wakes [`CancelToken::sleep`] when the token is cancelled.
    woken: Condvar,
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
        for (group, elsewhere) in running.iter() {
            // There first: the session here ending doesn't stop it.
            if let Some(elsewhere) = elsewhere {
                elsewhere.terminate();
            }
            process::terminate_group(*group);
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
    pub(crate) fn sleep(&self, duration: Duration) -> bool {
        let deadline = Instant::now().checked_add(duration);
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
        let mut running = self.0.running.lock();
        // A cancel that came between the check before spawning and now: it may not have seen
        // this group, so stop it here.
        if self.is_cancelled() {
            if let Some(elsewhere) = &elsewhere {
                elsewhere.terminate();
            }
            process::terminate_group(group);
        }
        running.push((group, elsewhere));
        drop(running);
        Registration { token: self, group }
    }
}

/// A process group running under a [`CancelToken`].
#[must_use = "the group stays signallable only while the registration lives"]
pub(crate) struct Registration<'a> {
    token: &'a CancelToken,
    group: Group,
}

impl Drop for Registration<'_> {
    fn drop(&mut self) {
        let mut running = self.token.0.running.lock();
        if let Some(index) = running.iter().position(|(group, _)| *group == self.group) {
            running.swap_remove(index);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(!token.sleep(Duration::from_millis(20)));
        assert!(started.elapsed() >= Duration::from_millis(20));
    }

    #[test]
    fn cancel_wakes_a_sleeper() {
        let token = CancelToken::new();
        let sleeper = {
            let token = token.clone();
            std::thread::spawn(move || {
                let started = Instant::now();
                (token.sleep(Duration::from_secs(60)), started.elapsed())
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
        assert!(token.sleep(Duration::from_secs(60)));
    }
}
