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
mod blame;
mod branch;
mod cancel;
mod command;
mod commit;
mod committing;
mod config;
mod conflict;
mod diff;
mod env;
mod error;
mod git;
mod history;
mod integrate;
mod lines;
mod object_id;
mod objects;
mod output;
mod patch;
mod process;
mod profile;
mod progress;
mod rebase;
mod remote;
mod repo;
mod signatures;
mod span;
mod stage;
mod stash;
mod status;
#[cfg(feature = "testing")]
pub mod testing;
mod tree;
mod version;

pub use batch::GitProcess;
pub use blame::{BlameCommit, BlameRun, FileCommit, blame, file_log};
pub use branch::{
    BadRefName, DeleteBranchError, SwitchError, SwitchTo, check_ref_name, create_branch,
    create_tag, delete_branch, delete_tag, rename_branch, switch,
};
pub use cancel::CancelToken;
pub use command::{Access, GitCommand, Streamed};
pub use commit::{Commit, CommitParseError, Signature};
pub use committing::{Committed, NewCommit, create_commit};
pub use config::{
    ConfigEntry, ConfigPlace, ConfigScope, ConfigSnapshot, ConfigTarget, set_config, unset_config,
};
pub use conflict::{
    ConflictLines, ConflictSides, IncomingCommit, Merge, MergedText, Pick, Piece, Resolution,
    ResolveError, WholeOnly, conflict_sides, incoming_commit, merge_sides, resolve_conflict,
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
pub use integrate::{Integrated, ResetMode, cherry_pick, merge, rebase, reset, revert};
pub use object_id::{Abbreviated, ObjectFormat, ObjectId};
pub use objects::{Object, ObjectKind, ObjectReader};
pub use output::{Completed, Stderr};
pub use patch::{Direction, LinesError, apply_lines, write_index_content};
pub use profile::{PullFastForward, PullRebase, Setting, Signing, SigningFormat};
pub use progress::{Progress, ProgressReport};
pub use rebase::{
    MAX_PLANNED, Planned, RangeCommit, RangeError, RebaseError, RebaseRange, Step,
    rebase_interactive, rebase_range,
};
pub use remote::{
    CloneOptions, FetchOptions, PullMode, Pulled, PushFlag, PushOptions, Pushed, PushedRef,
    abort_operation, clone, continue_operation, delete_remote_branch, fetch, pull, push, push_tag,
    remotes,
};
pub use repo::{
    BranchHead, CommitSummary, DiscoverError, Head, Operation, Repository, Tracking, Upstream,
    trust_directory,
};
pub use signatures::{SignatureCheck, Verdict, check_signatures};
pub use stage::{stage, unstage};
pub use stash::{Stash, StashError, stash_apply, stash_drop, stash_push, stashes};
pub use status::{Change, Conflict, EntryKind, Status, StatusEntry, SubmoduleChanges};
pub use tree::{ignored_paths, tree_entry, tree_folders};
pub use version::{GitVersion, VersionParseError};
