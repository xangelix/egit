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
mod credential;
mod diff;
mod env;
mod error;
mod format_patch;
mod forward;
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
mod search;
mod signatures;
mod span;
mod stage;
mod stash;
mod status;
#[cfg(feature = "testing")]
pub mod testing;
mod tree;
mod version;
mod worktree;

pub use batch::GitProcess;
pub use blame::{BlameCommit, BlameRun, FileCommit, Moves, blame, file_log};
pub use branch::{
    BadRefName, DeleteBranchError, SwitchError, SwitchTo, check_ref_name, create_branch,
    create_tag, delete_branch, delete_tag, rename_branch, set_upstream, switch,
};
pub use cancel::CancelToken;
pub use command::{Access, GitCommand, SecretOutput, Streamed};
pub use commit::{Commit, CommitParseError, Signature};
pub use committing::{Committed, NewCommit, create_commit};
pub use config::{
    ConfigEntry, ConfigPlace, ConfigScope, ConfigSnapshot, ConfigTarget, add_config_value,
    set_config, unset_config, unset_config_value,
};
pub use conflict::{
    ConflictLines, ConflictSides, IncomingCommit, Merge, MergedText, Pick, Piece, Resolution,
    ResolveError, WholeOnly, conflict_sides, incoming_commit, merge_sides, resolve_conflict,
};
pub use credential::{
    Asking, Credential, CredentialPlace, approve_credential, fill_credential, reject_credential,
};
pub use diff::{
    DiffLine, DiffLineKind, DiffOptions, FileDiff, Uncommitted, WHOLE_FILE, file_diff,
    uncommitted_diff,
};
pub use error::{Failure, GitError};
pub use format_patch::{format_patch, patch_file_name};
pub use forward::{ForwardFrom, Forwarded, fast_forward_branch, fast_forward_current};
pub use git::{Git, GitOptions};
pub use history::{
    ChangeKind, CountedFile, FileChange, LineCounts, RefKind, Reference, TreeEntry, Walked,
    commit_changes, commits_between, line_counts, list_references, walk_history, walk_history_from,
    write_commit_graph,
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
    abort_operation, clone, continue_operation, delete_remote_branch, fetch, fetch_refspecs, pull,
    push, push_tag, remote_urls, remotes,
};
pub use repo::{
    BranchHead, CommitSummary, DiscoverError, Head, Operation, Repository, Tracking, Upstream,
    trust_directory,
};
pub use search::{CommitFilter, find_commits, resolve_commit};
pub use signatures::{SignatureCheck, Verdict, check_signatures};
pub use stage::{stage, unstage};
pub use stash::{Stash, StashError, stash_apply, stash_drop, stash_push, stashes};
pub use status::{Change, Conflict, EntryKind, Status, StatusEntry, SubmoduleChanges};
pub use tree::{ignored_paths, tree_entry, tree_folders};
pub use version::{GitVersion, VersionParseError};
pub use worktree::{WorktreeFrom, add_worktree};
