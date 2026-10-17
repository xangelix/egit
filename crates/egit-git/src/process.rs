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
//! but for the askpass's `getpeereid` on macOS.

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub(crate) use unix::{Group, group_of, spawn_in_new_session, terminate_group, wait_until_exited};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub(crate) use windows::{
    Group, group_of, spawn_in_new_session, terminate_group, wait_until_exited,
};
