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

pub mod askpass;
mod batch;
mod cancel;
mod command;
mod commit;
mod config;
mod diff;
mod env;
mod error;
mod git;
mod history;
mod lines;
mod object_id;
mod objects;
mod output;
mod patch;
mod process;
mod profile;
mod repo;
mod span;
mod stage;
mod status;
#[cfg(feature = "testing")]
pub mod testing;
mod version;

pub use batch::GitProcess;
pub use cancel::CancelToken;
pub use command::{Access, GitCommand, Streamed};
pub use commit::{Commit, CommitParseError, Signature};
pub use config::{
    ConfigEntry, ConfigPlace, ConfigScope, ConfigSnapshot, ConfigTarget, set_config, unset_config,
};
pub use diff::{
    DiffLine, DiffLineKind, DiffOptions, FileDiff, Uncommitted, WHOLE_FILE, file_diff,
    uncommitted_diff,
};
pub use error::{Failure, GitError};
pub use git::{Git, GitOptions};
pub use history::{
    ChangeKind, CountedFile, FileChange, LineCounts, RefKind, Reference, TreeEntry, Walked,
    commit_changes, line_counts, list_references, walk_history, write_commit_graph,
};
pub use object_id::{Abbreviated, ObjectFormat, ObjectId};
pub use objects::{Object, ObjectKind, ObjectReader};
pub use output::{Completed, Stderr};
pub use patch::{Direction, LinesError, apply_lines, write_index_content};
pub use profile::{PullFastForward, PullRebase, Setting, Signing, SigningFormat};
pub use repo::{
    BranchHead, CommitSummary, DiscoverError, Head, Operation, Repository, Tracking, Upstream,
    trust_directory,
};
pub use stage::{stage, unstage};
pub use status::{Change, Conflict, EntryKind, Status, StatusEntry, SubmoduleChanges};
pub use version::{GitVersion, VersionParseError};
