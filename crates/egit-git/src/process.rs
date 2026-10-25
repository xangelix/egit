//! Spawning git as the leader of a group of its own, stopping that group, and waiting on it
//! safely, on each platform.
//!
//! Every git process leads a group: cancelling stops git together with every hook, ssh, and
//! helper it started, in a way that lets git remove its lock files first (SIGTERM on Unix; on
//! Windows, git's own way of stopping a process gently). Stopping a group by its id is only
//! safe while that id can't be reused: [`wait_until_exited`] waits for the leader's exit while
//! its id stays reserved, so the owner can stop asking to stop it before letting the id go.
//!
//! These modules hold the crate's `unsafe` code, the calls the standard library doesn't make,
//! but for the askpass's `getpeereid` on macOS. [`Detached`] runs other programs the same way.

use std::io;
use std::process::{Child, Command, ExitStatus};
use std::sync::Arc;
use std::time::Duration;

use web_time::Instant;

use crate::sync::Mutex;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub(crate) use unix::{Group, group_of, spawn_in_new_session, terminate_group, wait_until_exited};

#[cfg(not(any(unix, windows)))]
mod unsupported;
#[cfg(not(any(unix, windows)))]
pub(crate) use unsupported::{
    Group, group_of, spawn_in_new_session, terminate_group, wait_until_exited,
};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub(crate) use windows::{
    Group, group_of, spawn_in_new_session, terminate_group, wait_until_exited,
};

/// A program egit runs beside git (an agent's bridge, say), started and stopped as git is.
///
/// It leads a group of its own without a terminal (on Windows, without a console window), and
/// is stopped together with everything it started, gently first. Its group is only signalled
/// while its leader isn't reaped, so its id can't have been reused.
#[derive(Debug)]
pub struct Detached {
    child: Child,
    group: Group,
    /// Reaped: its id may belong to another process now.
    reaped: bool,
    /// Its group, for whoever stops it from elsewhere, until it is reaped.
    stopper: Stopper,
}

/// Stops a [`Detached`]'s group from any thread, as cancelling git does, while its leader isn't
/// reaped (then nothing: its id may be another's). Cheap to clone: clones share it.
#[derive(Clone, Debug)]
pub struct Stopper(Arc<Mutex<Option<Group>>>);

impl Stopper {
    /// Asks its group to stop (SIGTERM on Unix; on Windows, Git for Windows' gentle way).
    pub fn terminate(&self) {
        // Held while signalling: it can't be reaped meanwhile.
        let group = self.0.lock();
        if let Some(group) = *group {
            terminate_group(group);
        }
    }
}

impl Detached {
    /// Spawns `command` leading a group of its own.
    ///
    /// # Errors
    ///
    /// When it can't be started.
    pub fn spawn(command: &mut Command) -> io::Result<Self> {
        let child = spawn_in_new_session(command)?;
        let group = group_of(&child);
        Ok(Self {
            child,
            group,
            reaped: false,
            stopper: Stopper(Arc::new(Mutex::new(Some(group)))),
        })
    }

    /// What stops its group from elsewhere, until it is reaped.
    #[must_use]
    pub fn stopper(&self) -> Stopper {
        self.stopper.clone()
    }

    /// Waits for it to end, and reaps it.
    ///
    /// # Errors
    ///
    /// When it can't be waited on.
    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        // Its leader exited, its id still reserved: nothing signals it from now on.
        wait_until_exited(self.group)?;
        *self.stopper.0.lock() = None;
        let status = self.child.wait();
        self.reaped |= status.is_ok();
        status
    }

    /// The process, to take its pipes.
    pub const fn child_mut(&mut self) -> &mut Child {
        &mut self.child
    }

    /// How it ended, once it has (reaping it).
    ///
    /// # Errors
    ///
    /// When it can't be waited on.
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        // Reaped under the stopper's lock: nothing signals its id once it may be another's.
        let mut group = self.stopper.0.lock();
        let status = self.child.try_wait()?;
        if status.is_some() {
            *group = None;
            self.reaped = true;
        }
        drop(group);
        Ok(status)
    }

    /// Waits up to `patience` for it to end by itself, then asks its group to stop (as
    /// cancelling git does) and waits as long again, then kills it.
    pub fn stop(&mut self, patience: Duration) {
        if self.ended_within(patience) {
            return;
        }
        if !self.reaped {
            terminate_group(self.group);
        }
        if self.ended_within(patience) {
            return;
        }
        if let Err(error) = self.child.kill() {
            tracing::debug!(%error, "couldn't kill a detached process");
        }
        let mut group = self.stopper.0.lock();
        *group = None;
        self.reaped |= self.child.wait().is_ok();
        drop(group);
    }

    /// Whether it ended within `patience`, looking every few milliseconds.
    fn ended_within(&mut self, patience: Duration) -> bool {
        let until = Instant::now().checked_add(patience);
        loop {
            // An error: not a child it can wait on any more, so nothing to stop.
            if !matches!(self.try_wait(), Ok(None)) {
                return true;
            }
            if until.is_none_or(|until| Instant::now() >= until) {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Drop for Detached {
    fn drop(&mut self) {
        if !self.reaped {
            self.stop(Duration::ZERO);
        }
    }
}
