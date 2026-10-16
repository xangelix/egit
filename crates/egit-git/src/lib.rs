//! egit's interface to the git binary.
//!
//! egit never reads or writes a repository itself: every operation runs the `git` installed on
//! the system, so hooks, signing, credential helpers, filters, and every repository format git
//! understands work exactly as they do on the command line. This crate runs that binary safely
//! and turns what it prints into types.
//!
//! [`Git`] is the binary (found, version-checked, with a scrubbed environment); [`Git::read`] and
//! [`Git::write`] build a [`GitCommand`], which runs git in a session of its own and can be
//! cancelled through a [`CancelToken`].

mod cancel;
mod command;
mod config;
mod env;
mod error;
mod git;
mod object_id;
mod output;
mod process;
mod profile;
mod repo;
#[cfg(feature = "testing")]
pub mod testing;
mod version;

pub use cancel::CancelToken;
pub use command::{Access, GitCommand, Streamed};
pub use config::{
    ConfigEntry, ConfigPlace, ConfigScope, ConfigSnapshot, ConfigTarget, set_config, unset_config,
};
pub use error::{Failure, GitError};
pub use git::{Git, GitOptions};
pub use object_id::{Abbreviated, ObjectFormat, ObjectId};
pub use output::{Completed, Stderr};
pub use profile::{PullFastForward, PullRebase, Setting, Signing, SigningFormat};
pub use repo::{
    BranchHead, CommitSummary, DiscoverError, Head, Operation, Repository, Tracking, Upstream,
    trust_directory,
};
pub use version::{GitVersion, VersionParseError};
