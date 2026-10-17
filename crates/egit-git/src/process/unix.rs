//! Unix: git in a session of its own, signalled and waited on safely.
//!
//! Every git process starts a new session (`setsid`) before it runs. That has two effects egit
//! relies on:
//!
//! - It has no controlling terminal, so nothing it starts (ssh, gpg's pinentry, a credential
//!   helper) can prompt on the terminal egit was launched from, where nobody is looking. Prompts
//!   go through the askpass helper or fail.
//! - It leads its own process group, so cancelling signals git together with every hook, ssh,
//!   and helper it started.
//!
//! Signalling a process group by id is only safe while that id can't be reused, i.e. until the
//! process is reaped. [`wait_until_exited`] waits for the exit *without* reaping, so the owner can
//! stop signalling first and reap afterwards.
//!
//! Its `unsafe` code is the `pre_exec` hook that calls `setsid`.

#![expect(
    unsafe_code,
    reason = "`pre_exec` is the only way to run `setsid` in the child"
)]

use std::io;
use std::os::unix::process::CommandExt as _;
use std::process::{Child, Command};

use rustix::io::Errno;
use rustix::process::{Pid, Signal, WaitId, WaitIdOptions};

/// A process group: the id of the git that leads it.
pub(crate) type Group = Pid;

/// Spawns `command` as the leader of a new session.
pub(crate) fn spawn_in_new_session(command: &mut Command) -> io::Result<Child> {
    // SAFETY: the hook runs in the child between `fork` and `exec`, where only
    // async-signal-safe operations are allowed. `setsid` is a single system call (rustix calls
    // it directly, without allocating or taking locks), and the error conversion only moves an
    // integer.
    unsafe {
        command.pre_exec(|| {
            rustix::process::setsid()?;
            Ok(())
        });
    }
    command.spawn()
}

/// The process group a spawned git leads (its id is the child's).
pub(crate) fn group_of(child: &Child) -> Group {
    Pid::from_child(child)
}

/// Asks every process in the group to stop (SIGTERM). Git removes its lock files on SIGTERM.
pub(crate) fn terminate_group(group: Group) {
    match rustix::process::kill_process_group(group, Signal::TERM) {
        // Everything in the group has already exited.
        Ok(()) | Err(Errno::SRCH) => {}
        Err(error) => tracing::warn!(%error, group = group.as_raw_pid(), "could not signal git"),
    }
}

/// Blocks until `pid` has exited, leaving it unreaped so its id stays reserved.
pub(crate) fn wait_until_exited(pid: Group) -> io::Result<()> {
    loop {
        match rustix::process::waitid(
            WaitId::Pid(pid),
            WaitIdOptions::EXITED | WaitIdOptions::NOWAIT,
        ) {
            Ok(_) => return Ok(()),
            Err(Errno::INTR) => {}
            Err(error) => return Err(error.into()),
        }
    }
}
