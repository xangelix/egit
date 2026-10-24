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

// In a browser the standard library's processes are stand-ins that can't exist (a relay runs
// every program): what drops or borrows one there is never reached.
#![cfg_attr(
    target_arch = "wasm32",
    allow(clippy::drop_non_drop, clippy::uninhabited_references)
)]

mod applying;
pub mod askpass;
mod batch;
mod bisect;
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
mod discard;
mod env;
mod error;
pub mod exec;
mod files;
mod foresee;
mod format_patch;
mod forward;
mod git;
mod history;
mod hooks;
mod host;
mod ignore;
mod integrate;
mod lfs;
mod lines;
mod looking;
mod object_id;
mod objects;
mod output;
mod patch;
mod people;
mod process;
mod profile;
mod progress;
mod rebase;
mod reflog;
pub mod relay;
mod remote;
mod remote_setup;
mod repo;
mod running;
mod search;
mod signatures;
mod signing;
mod span;
mod ssh;
mod stage;
mod stash;
mod status;
mod submodule;
#[cfg(feature = "testing")]
pub mod testing;
mod transcript;
mod tree;
mod version;
mod worktree;

pub use applying::{
    Applies, ApplyAs, MAX_SUMMARIZED, MailboxProgress, PatchChange, PatchCommit, PatchFile,
    PatchKind, PatchSummary, apply_patch, current_patch, looks_like_patch, patch_kind,
    summarize_patch,
};
pub use batch::GitProcess;
pub use bisect::{BisectLeft, BisectMark, BisectTerms, Bisection, bisect_mark, bisection};
pub use blame::{BlameCommit, BlameRun, FileCommit, Moves, blame, file_log};
pub use branch::{
    BadRefName, DeleteBranchError, SwitchError, SwitchTo, check_ref_name, create_branch,
    create_tag, delete_branch, delete_tag, move_branch, rename_branch, set_upstream, switch,
};
pub use cancel::CancelToken;
pub use command::{Access, GitCommand, SecretOutput, Streamed};
pub use commit::{CO_AUTHORED_BY, Commit, CommitParseError, Person, Signature};
pub use committing::{Committed, NewCommit, create_commit, reword_head};
pub use config::{
    ConfigEntry, ConfigPlace, ConfigScope, ConfigSnapshot, ConfigTarget, add_config_value,
    set_config, unset_config, unset_config_value,
};
pub use conflict::{
    ConflictLines, ConflictSides, IncomingCommit, Merge, MergedText, Pick, Piece, Resolution,
    ResolveError, WholeOnly, conflict_sides, incoming_commit, merge_sides, resolve_conflict,
};
pub use credential::{
    Asking, Credential, CredentialPlace, approve_credential, erase_credential, fill_credential,
    installed_helpers, reject_credential, store_credential,
};
pub use diff::{
    DiffLine, DiffLineKind, DiffOptions, FileDiff, Uncommitted, WHOLE_FILE, file_diff,
    uncommitted_diff,
};
pub use discard::{
    DiscardError, DiscardSide, DiscardedFile, KEEP_BYTES, KEEP_FILES, KeptFile, discard_files,
    discard_lines, put_back,
};
pub use error::{Failure, GitError};
#[cfg(not(target_arch = "wasm32"))]
pub use exec::{Wait, block_on};
pub use files::{Entry, Files, Put, TempDir, is_absolute};
pub use foresee::{Foreseen, MAX_REPLAYED, foresee_merge, foresee_pick, foresee_rebase};
pub use format_patch::{
    Changes, CombineError, Series, changes_patch, combined_patch, format_patch, format_patches,
    format_series, format_series_into, patch_file_name,
};
pub use forward::{ForwardFrom, Forwarded, fast_forward_branch, fast_forward_current};
pub use git::{Git, GitOptions};
pub use history::{
    ChangeKind, CommitLines, CountedFile, FileChange, LineCounts, ListedCommit, RefKind, Reference,
    TreeEntry, Walked, commit_changes, commit_lines, commits_between, count_between, history_tips,
    line_counts, list_commits, list_references, walk_history, walk_history_from,
    write_commit_graph,
};
pub use hooks::{Hooks, KNOWN as KNOWN_HOOKS, installed_hooks};
pub use host::{Host, SshHost};
pub use ignore::{
    IgnoreError, IgnoreIn, Ignored, extension_pattern, ignore, ignore_again, path_pattern, unignore,
};
pub use integrate::{
    FastForward, Integrated, ResetMode, cherry_pick, cherry_pick_all, merge, rebase, reset, revert,
    revert_all,
};
pub use lfs::{
    LfsFile, LfsLock, LfsPattern, LfsTransfer, LfsVersion, lfs_files, lfs_install, lfs_lock,
    lfs_locks, lfs_patterns, lfs_prune, lfs_set_up, lfs_track, lfs_transfer, lfs_untrack,
    lfs_version,
};
pub use looking::{
    Follow, INOTIFYWAIT, Liveness, Looked, Looking, Seen, classify, look_at_files_there,
    look_at_folders_there, look_there,
};
pub use object_id::{Abbreviated, ObjectFormat, ObjectId};
pub use objects::{Object, ObjectKind, ObjectReader};
pub use output::{Completed, Exit, Stderr};
pub use patch::{Direction, LinesError, apply_lines, write_index_content};
pub use people::{Author, map_contacts, recent_authors};
pub use process::{Detached, Stopper};
pub use profile::{PullFastForward, PullRebase, Setting, Signing, SigningFormat};
pub use progress::{Progress, ProgressReport};
pub use rebase::{
    MAX_PLANNED, Planned, RangeCommit, RangeError, RebaseError, RebaseRange, Step,
    rebase_interactive, rebase_keeping_merges, rebase_range,
};
pub use reflog::{ReflogEntry, create_ref, delete_ref, ref_target, reflog};
pub use relay::{Relay, Transport, Welcome};
pub use remote::{
    CloneOptions, FetchOptions, PullMode, Pulled, PushFlag, PushOptions, Pushed, PushedRef,
    abort_operation, clone, continue_operation, delete_remote_branch, fetch, fetch_refspecs,
    fetch_remote, pull, push, push_tag, remote_urls, remotes, skip_operation,
};
pub use remote_setup::{add_remote, remove_remote, rename_remote, set_remote_url};
pub use repo::{
    BranchHead, CommitSummary, DiscoverError, Head, HeadRef, Operation, Repository, Tracking,
    Upstream, init_repository, trust_directory,
};
pub use running::{Input, Running, Stopping};
pub use search::{CommitFilter, find_commits, resolve_commit};
pub use signatures::{SignatureCheck, Verdict, check_signatures};
pub use signing::{MAX_SIGNED, SignError, Signed, SigningPlan, plan_signing, sign_commits};
pub use ssh::{Holding, KEPT_IDLE, SshSharing};
pub use stage::{stage, unstage};
pub use stash::{Stash, StashError, stash_apply, stash_drop, stash_push, stash_store, stashes};
pub use status::{Change, Conflict, EntryKind, Status, StatusEntry, SubmoduleChanges};
pub use submodule::{
    Checkout, Submodule, SubmoduleChange, SubmoduleDetails, SubmoduleStatus, UpdateTo,
    add_submodule, change_submodules, remove_submodule, set_submodule_branch, set_submodule_url,
    submodule_commits, submodule_details, submodule_status, submodules, update_submodules,
    update_submodules_to,
};
pub use transcript::{
    Color, Line, LineKind, Log, MAX_LINE, MAX_LINES, Span, Style, Transcript, apply_sgr,
    escape_sequence,
};
pub use tree::{git_path, ignored_paths, tree_entry, tree_files, tree_folders};
pub use version::{GitVersion, VersionParseError};
pub use worktree::{
    RemoveWorktreeError, Worktree, WorktreeFrom, add_worktree, lock_worktree, prune_worktrees,
    remove_worktree, unlock_worktree, worktrees,
};
