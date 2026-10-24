//! Where egit starts no program itself (egit in a browser: every program runs through a relay):
//! starting one says so.

#![allow(
    clippy::unused_self,
    clippy::unnecessary_wraps,
    clippy::missing_const_for_fn,
    reason = "the shape of every platform's own module, which these stand in for"
)]

use std::io;
use std::process::{Child, Command};

/// A process group: never one here.
pub(crate) type Group = u32;

pub(crate) fn spawn_in_new_session(_command: &mut Command) -> io::Result<Child> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "no program runs here: a relay runs them",
    ))
}

pub(crate) fn group_of(child: &Child) -> Group {
    child.id()
}

pub(crate) const fn terminate_group(_group: Group) {}

pub(crate) fn wait_until_exited(_group: Group) -> io::Result<()> {
    Err(io::Error::from(io::ErrorKind::Unsupported))
}
