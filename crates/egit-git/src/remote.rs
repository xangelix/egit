//! Moving commits between repositories: clone, fetch, pull, and push, each reporting its
//! progress (`--progress`) and stoppable, with git asking for credentials and host keys through
//! askpass (labelled by what is being done) unless it mustn't ask at all.

use std::path::Path;

use bstr::{BStr, BString, ByteSlice as _};

use crate::cancel::CancelToken;
use crate::error::GitError;
use crate::git::Git;
use crate::hooks::Checks;
use crate::integrate::{Integrated, carried_on};
use crate::progress::Progress;
use crate::repo::{Operation, Repository};

/// How a clone is made.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CloneOptions {
    /// Clone the submodules too (`--recurse-submodules`).
    pub submodules: bool,
    /// A partial clone: file contents are fetched when first needed (`--filter=blob:none`).
    pub partial: bool,
}

/// Clones `url` into `destination` (which must not exist, or be an empty folder).
///
/// It runs outside any repository. Cancelled, git removes what it made.
///
/// # Errors
///
/// When git fails (a wrong address, refused credentials, an existing destination) or is
/// cancelled.
pub async fn clone(
    git: &Git,
    url: &str,
    destination: &Path,
    options: CloneOptions,
    progress: &Progress,
    cancel: &CancelToken,
) -> Result<(), GitError> {
    let mut command = git
        .write("clone")
        .arg("--progress")
        .prompt_context(format!("Cloning {url}"))
        .current_dir(crate::git::outside_repositories())
        .progress(progress)
        .cancel_token(cancel);
    if options.submodules {
        command = command.arg("--recurse-submodules");
    }
    if options.partial {
        command = command.arg("--filter=blob:none");
        if options.submodules {
            command = command.arg("--also-filter-submodules");
        }
    }
    command
        .arg("--")
        .arg(url)
        .arg(destination)
        .output()
        .await
        .map(drop)
}

/// How a fetch is made.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FetchOptions {
    /// Every remote, not only the current branch's (`--all`).
    pub all: bool,
    /// Remove remote-tracking branches whose branches are gone (`--prune`).
    pub prune: bool,
    /// Nobody is watching: git may not ask anything (a prompt fails at once), and doesn't start
    /// its housekeeping after.
    pub background: bool,
}

/// Fetches from the repository's remotes, as `options` says.
///
/// # Errors
///
/// When git fails (a remote that can't be reached, refused credentials, a prompt in the
/// background) or is cancelled.
pub async fn fetch(
    git: &Git,
    repository: &Repository,
    options: FetchOptions,
    progress: &Progress,
    cancel: &CancelToken,
) -> Result<(), GitError> {
    let mut command = git
        .write("fetch")
        .arg("--progress")
        .prompt_context("Fetching")
        .current_dir(repository.work_tree())
        .progress(progress)
        .cancel_token(cancel);
    if options.all {
        command = command.arg("--all");
    }
    if options.prune {
        command = command.arg("--prune");
    }
    if options.background {
        command = command.arg("--no-auto-maintenance").non_interactive();
    }
    command.output().await.map(drop)
}

/// Fetches from `remote` alone (`fetch <remote>`), removing its remote-tracking branches whose
/// branches are gone with `prune`.
///
/// # Errors
///
/// When git fails (the remote can't be reached, refused credentials) or is cancelled.
pub async fn fetch_remote(
    git: &Git,
    repository: &Repository,
    remote: &BStr,
    prune: bool,
    progress: &Progress,
    cancel: &CancelToken,
) -> Result<(), GitError> {
    let mut command = git
        .write("fetch")
        .arg("--progress")
        .prompt_context(format!("Fetching from {}", remote.to_str_lossy()))
        .current_dir(repository.work_tree())
        .progress(progress)
        .cancel_token(cancel);
    if prune {
        command = command.arg("--prune");
    }
    command
        .arg("--")
        .arg(remote.to_os_str_lossy())
        .output()
        .await
        .map(drop)
}

/// Fetches what `refspecs` name from `remote`, and nothing else (no tags).
///
/// A branch into its remote-tracking branch, say, or a forge's pull request
/// (`refs/pull/12/head`) into a branch. `context` labels any prompt ("Fetching pull request
/// #12").
///
/// # Errors
///
/// When git fails (the remote doesn't have it, a branch it would move isn't a fast-forward or
/// is checked out) or is cancelled.
pub async fn fetch_refspecs(
    git: &Git,
    repository: &Repository,
    remote: &BStr,
    refspecs: &[BString],
    context: &str,
    progress: &Progress,
    cancel: &CancelToken,
) -> Result<(), GitError> {
    git.write("fetch")
        .args(["--progress", "--no-tags", "--"])
        .arg(remote.to_os_str_lossy())
        .args(refspecs.iter().map(|refspec| refspec.to_os_str_lossy()))
        .prompt_context(context)
        .current_dir(repository.work_tree())
        .progress(progress)
        .cancel_token(cancel)
        .output()
        .await
        .map(drop)
}

/// How a pull brings in the upstream's commits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PullMode {
    /// As `pull.rebase` and `pull.ff` say.
    #[default]
    AsConfigured,
    /// Merge them (`--no-rebase`).
    Merge,
    /// Put the branch's own commits after them (`--rebase`).
    Rebase,
    /// Only move the branch forward to them (`--ff-only`).
    FastForwardOnly,
}

/// How a pull ended, when git didn't fail outright.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pulled {
    /// The branch has the upstream's commits (or had them already).
    Updated,
    /// The branch has the upstream's commits, but the changes stashed first (`autostash`)
    /// came back with conflicts: they are in the work tree to resolve, and stay in the stash
    /// list too until dropped.
    ChangesConflicted,
    /// The branch and its upstream each have commits the other hasn't, and nothing says how
    /// to bring them together (or only fast-forwards are allowed): ask.
    Diverged,
    /// The merge or rebase stopped at conflicts: the user resolves them, then continues or
    /// aborts.
    Conflicts,
    /// Changes not committed are in the way, and nothing was pulled: changes to `paths`
    /// (none named when a rebase wants the work tree clean), or files of theirs git doesn't
    /// track (`untracked`), which a stash first (`autostash`) doesn't take.
    ChangesInTheWay {
        /// The files, as git names them.
        paths: Vec<BString>,
        /// Whether they are files git doesn't track.
        untracked: bool,
    },
}

/// Pulls the current branch's upstream, as `mode` says.
///
/// With `autostash`, the changes not committed are stashed first and put back after
/// (`--autostash`: what was staged comes back not staged); a merge or rebase that stops puts
/// them back once continued or aborted. A merge's `pre-merge-commit` and `commit-msg` hooks
/// run as `checks` says (git passes `--no-verify` to the merge alone: a rebase's `pre-rebase`
/// runs either way).
///
/// # Errors
///
/// When git fails otherwise (no upstream, a remote that can't be reached) or is cancelled.
pub async fn pull(
    git: &Git,
    repository: &Repository,
    (mode, autostash, checks): (PullMode, bool, Checks),
    progress: &Progress,
    cancel: &CancelToken,
) -> Result<Pulled, GitError> {
    let stashed_before = if autostash {
        crate::stash::latest(git, repository, cancel).await?
    } else {
        None
    };
    let mut command = git
        .write("pull")
        .arg("--progress")
        .args(checks.flag())
        .prompt_context("Pulling")
        .current_dir(repository.work_tree())
        .progress(progress)
        .cancel_token(cancel);
    command = match mode {
        PullMode::AsConfigured => command,
        PullMode::Merge => command.arg("--no-rebase"),
        PullMode::Rebase => command.arg("--rebase"),
        PullMode::FastForwardOnly => command.arg("--ff-only"),
    };
    if autostash {
        command = command.arg("--autostash");
    }
    let Err(error) = command.output().await else {
        // Changes that came back with conflicts are kept as a stash (git's words for it differ
        // between versions; the stash list doesn't).
        if autostash && crate::stash::latest(git, repository, cancel).await? != stashed_before {
            return Ok(Pulled::ChangesConflicted);
        }
        return Ok(Pulled::Updated);
    };
    let Some(failure) = error.failure() else {
        return Err(error);
    };
    let said = failure.stderr.as_bytes();
    if said.contains_str("divergent branches") || said.contains_str("Not possible to fast-forward")
    {
        return Ok(Pulled::Diverged);
    }
    if let Some((paths, untracked)) = crate::branch::overwritten(said) {
        return Ok(Pulled::ChangesInTheWay { paths, untracked });
    }
    if said.contains_str("cannot pull with rebase") {
        return Ok(Pulled::ChangesInTheWay {
            paths: Vec::new(),
            untracked: false,
        });
    }
    let stopped = repository
        .operations(git)
        .await
        .map_err(|source| GitError::Io {
            subcommand: "pull",
            source,
        })?
        .iter()
        .any(|operation| matches!(operation, Operation::Merge | Operation::Rebase));
    if stopped {
        Ok(Pulled::Conflicts)
    } else {
        Err(error)
    }
}

/// What happened to a ref a push named.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PushFlag {
    /// Moved forward.
    FastForward,
    /// Replaced (a forced push).
    Forced,
    /// Deleted.
    Deleted,
    /// Made.
    New,
    /// Refused: see the reason.
    Rejected,
    /// Already there.
    UpToDate,
}

/// A ref a push named, and what happened to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PushedRef {
    /// What happened to it.
    pub flag: PushFlag,
    /// The local ref (empty when deleting).
    pub from: BString,
    /// The remote's ref.
    pub to: BString,
    /// Git's summary (`a1b2c3..d4e5f6`, `[new branch]`, `[rejected]`).
    pub summary: BString,
    /// Why it was refused, as git says (`fetch first`, `non-fast-forward`, `stale info`, a
    /// remote hook's words).
    pub reason: Option<BString>,
}

/// What a push did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pushed {
    /// Each ref named.
    pub refs: Vec<PushedRef>,
    /// What git and the remote printed besides (a link to open a pull request, say).
    pub notes: Vec<String>,
}

impl Pushed {
    /// Whether every ref went through.
    #[must_use]
    pub fn succeeded(&self) -> bool {
        self.refs
            .iter()
            .all(|pushed| pushed.flag != PushFlag::Rejected)
    }
}

/// How a push is made.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PushOptions {
    /// Where to: a remote's name.
    pub remote: BString,
    /// The local branch (its short name).
    pub branch: BString,
    /// The branch it becomes on the remote (its short name).
    pub remote_branch: BString,
    /// Make the remote's branch the local one's upstream (`--set-upstream`).
    pub set_upstream: bool,
    /// Replace the remote's branch, but only if it is still what was last fetched and that
    /// was brought into the local branch (`--force-with-lease --force-if-includes`).
    pub force: bool,
}

/// Pushes a branch, as `options` says. A refused ref isn't an error: [`Pushed`] says why.
///
/// # Errors
///
/// When git fails before any ref is pushed (a remote that can't be reached, refused
/// credentials) or is cancelled.
pub async fn push(
    git: &Git,
    repository: &Repository,
    options: &PushOptions,
    progress: &Progress,
    cancel: &CancelToken,
) -> Result<Pushed, GitError> {
    let mut refspec = b"refs/heads/".to_vec();
    refspec.extend_from_slice(&options.branch);
    refspec.extend_from_slice(b":refs/heads/");
    refspec.extend_from_slice(&options.remote_branch);
    let mut flags = Vec::new();
    if options.set_upstream {
        flags.push("--set-upstream");
    }
    if options.force {
        flags.extend(["--force-with-lease", "--force-if-includes"]);
    }
    let push = Push {
        remote: options.remote.as_ref(),
        refspec: refspec.as_bstr(),
        flags: &flags,
    };
    push.run(git, repository, progress, cancel).await
}

/// Pushes the tag `tag` to `remote` (`refs/tags/<tag>`, as it is here). A refused ref isn't
/// an error: [`Pushed`] says why.
///
/// # Errors
///
/// When git fails before any ref is pushed, or is cancelled.
pub async fn push_tag(
    git: &Git,
    repository: &Repository,
    remote: &BStr,
    tag: &BStr,
    progress: &Progress,
    cancel: &CancelToken,
) -> Result<Pushed, GitError> {
    let mut refspec = b"refs/tags/".to_vec();
    refspec.extend_from_slice(tag);
    refspec.push(b':');
    refspec.extend_from_slice(b"refs/tags/");
    refspec.extend_from_slice(tag);
    let push = Push {
        remote,
        refspec: refspec.as_bstr(),
        flags: &[],
    };
    push.run(git, repository, progress, cancel).await
}

/// Deletes the branch `branch` (its short name there) on `remote` (pushing nothing to
/// `refs/heads/<branch>`). A refused deletion isn't an error: [`Pushed`] says why.
///
/// # Errors
///
/// When git fails before any ref is pushed, or is cancelled.
pub async fn delete_remote_branch(
    git: &Git,
    repository: &Repository,
    remote: &BStr,
    branch: &BStr,
    progress: &Progress,
    cancel: &CancelToken,
) -> Result<Pushed, GitError> {
    let mut refspec = b":refs/heads/".to_vec();
    refspec.extend_from_slice(branch);
    let push = Push {
        remote,
        refspec: refspec.as_bstr(),
        flags: &[],
    };
    push.run(git, repository, progress, cancel).await
}

/// One refspec pushed to a remote, with flags.
struct Push<'a> {
    remote: &'a BStr,
    refspec: &'a BStr,
    flags: &'a [&'a str],
}

impl Push<'_> {
    /// Runs `push --porcelain`; a refused ref isn't an error.
    async fn run(
        &self,
        git: &Git,
        repository: &Repository,
        progress: &Progress,
        cancel: &CancelToken,
    ) -> Result<Pushed, GitError> {
        let done = git
            .write("push")
            .args(["--porcelain", "--progress"])
            .args(self.flags)
            .prompt_context(format!("Pushing to {}", self.remote))
            .current_dir(repository.work_tree())
            .progress(progress)
            .cancel_token(cancel)
            .arg("--")
            .arg(self.remote.to_os_str_lossy())
            .arg(self.refspec.to_os_str_lossy())
            .output_unchecked()
            .await?;
        let (refs, mut notes) = parse_porcelain(&done.stdout)?;
        if refs.is_empty() && !done.status.success() {
            return Err(GitError::failed("push", done.status, done.stderr));
        }
        notes.extend(progress.report().notes);
        Ok(Pushed { refs, notes })
    }
}

/// Reads `push --porcelain`: `To <url>`, then a line per ref (`<flag>\t<from>:<to>\t<summary>`,
/// a reason in parentheses after a refusal), then `Done`; other lines (`branch 'x' set up to
/// track 'origin/x'.`) are notes.
fn parse_porcelain(output: &[u8]) -> Result<(Vec<PushedRef>, Vec<String>), GitError> {
    let mut refs = Vec::new();
    let mut notes = Vec::new();
    for line in output.lines() {
        if line.starts_with(b"To ") || line == b"Done" || line.is_empty() {
            continue;
        }
        // A ref's line is its flag, then a tab.
        if line.get(1) != Some(&b'\t') {
            notes.push(line.to_str_lossy().into_owned());
            continue;
        }
        let malformed =
            || GitError::unexpected("push", format!("{} isn't a pushed ref", line.as_bstr()));
        let mut fields = line.splitn_str(3, b"\t");
        let (flag, names, summary) = (
            fields.next().ok_or_else(malformed)?,
            fields.next().ok_or_else(malformed)?,
            fields.next().ok_or_else(malformed)?,
        );
        let flag = match flag {
            b" " => PushFlag::FastForward,
            b"+" => PushFlag::Forced,
            b"-" => PushFlag::Deleted,
            b"*" => PushFlag::New,
            b"!" => PushFlag::Rejected,
            b"=" => PushFlag::UpToDate,
            _ => return Err(malformed()),
        };
        let (from, to) = names.split_once_str(b":").ok_or_else(malformed)?;
        let (summary, reason) = match summary.find_byte(b'(') {
            Some(open) if summary.ends_with(b")") => (
                summary.get(..open).unwrap_or_default().trim_ascii_end(),
                summary
                    .get(open.saturating_add(1)..summary.len().saturating_sub(1))
                    .map(BString::from),
            ),
            _ => (summary, None),
        };
        refs.push(PushedRef {
            flag,
            from: BString::from(from),
            to: BString::from(to),
            summary: BString::from(summary),
            reason,
        });
    }
    Ok((refs, notes))
}

/// The repository's remotes, by name.
///
/// # Errors
///
/// When git fails.
pub async fn remotes(git: &Git, repository: &Repository) -> Result<Vec<BString>, GitError> {
    let listed = git
        .read("remote")
        .current_dir(repository.work_tree())
        .output()
        .await?;
    Ok(listed
        .stdout
        .lines()
        .filter(|line| !line.is_empty())
        .map(BString::from)
        .collect())
}

/// Each remote's name and its URL as git reaches it (`url.<base>.insteadOf` applied), by
/// `git ls-remote --get-url`, which connects to nothing.
///
/// # Errors
///
/// When git fails.
pub async fn remote_urls(
    git: &Git,
    repository: &Repository,
) -> Result<Vec<(BString, BString)>, GitError> {
    let mut urls = Vec::new();
    for name in remotes(git, repository).await? {
        let url = git
            .read("ls-remote")
            .args(["--get-url", "--"])
            .arg(name.to_os_str_lossy())
            .current_dir(repository.work_tree())
            .output()
            .await?
            .stdout;
        let url = url.strip_suffix(b"\n").unwrap_or(&url);
        urls.push((name, BString::from(url)));
    }
    Ok(urls)
}

/// Gives up an operation git stopped in the middle of (a merge, a rebase, …), putting things
/// back as they were before it.
///
/// # Errors
///
/// When git fails or is cancelled.
pub async fn abort_operation(
    git: &Git,
    repository: &Repository,
    operation: Operation,
    cancel: &CancelToken,
) -> Result<(), GitError> {
    let (subcommand, argument) = match operation {
        Operation::Merge => ("merge", "--abort"),
        Operation::Rebase => ("rebase", "--abort"),
        Operation::ApplyMailbox => ("am", "--abort"),
        Operation::CherryPick => ("cherry-pick", "--abort"),
        Operation::Revert => ("revert", "--abort"),
        Operation::Bisect => ("bisect", "reset"),
    };
    git.write(subcommand)
        .arg(argument)
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()
        .await
        .map(drop)
}

/// Skips the commit (or patch) an operation stopped at, dropping its changes, and carries on.
///
/// `git rebase`, `am`, `cherry-pick`, or `revert --skip`; says how it then ended: done, or
/// stopped again (at conflicts, or a patch that doesn't apply). A merge or a bisect has nothing
/// to skip. The patches `am` applies next run its hooks as `checks` says (`am` doesn't keep
/// the `--no-verify` it began with); a commit the others replay runs no hook that checks.
///
/// # Errors
///
/// When there is nothing to skip, git refuses, or is cancelled.
pub async fn skip_operation(
    git: &Git,
    repository: &Repository,
    operation: Operation,
    checks: Checks,
    cancel: &CancelToken,
) -> Result<Integrated, GitError> {
    let subcommand = match operation {
        Operation::Rebase => "rebase",
        Operation::ApplyMailbox => "am",
        Operation::CherryPick => "cherry-pick",
        Operation::Revert => "revert",
        Operation::Merge | Operation::Bisect => {
            return Err(GitError::unexpected("skip", "nothing to skip"));
        }
    };
    let skipped = git
        .write(subcommand)
        .arg("--skip")
        .args(
            checks
                .flag()
                .filter(|_| operation == Operation::ApplyMailbox),
        )
        .prompt_context("Signing the commit")
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()
        .await
        .map(drop);
    carried_on(git, repository, skipped, cancel).await
}

/// Carries on with an operation git stopped in the middle of, once its conflicts are resolved
/// and staged (its commits keep their messages). A bisect has nothing to carry on with.
///
/// The commit a merge, cherry-pick, or revert then makes runs `pre-commit` and `commit-msg`,
/// and a patch `am` applies its own hooks, as `checks` says. Their `--continue` takes no
/// `--no-verify` (but `am`'s), so skipping them, the commit is made by `git commit --no-edit
/// --no-verify` (the message git prepared, as `--continue` would), then a cherry-pick or
/// revert of several commits goes on with the rest (`--continue`: its `sequencer` folder
/// says there are). A rebase runs no hook that checks as it goes on.
///
/// # Errors
///
/// When git refuses (conflicts left, say) or is cancelled.
pub async fn continue_operation(
    git: &Git,
    repository: &Repository,
    operation: Operation,
    checks: Checks,
    cancel: &CancelToken,
) -> Result<(), GitError> {
    if checks.skipped()
        && matches!(
            operation,
            Operation::Merge | Operation::CherryPick | Operation::Revert
        )
    {
        return commit_then_continue(git, repository, operation, cancel).await;
    }
    let subcommand = match operation {
        Operation::Merge => "merge",
        Operation::Rebase => "rebase",
        Operation::ApplyMailbox => "am",
        Operation::CherryPick => "cherry-pick",
        Operation::Revert => "revert",
        Operation::Bisect => {
            return Err(GitError::unexpected(
                "bisect",
                "a bisect has nothing to continue",
            ));
        }
    };
    git.write(subcommand)
        .arg("--continue")
        .args(
            checks
                .flag()
                .filter(|_| operation == Operation::ApplyMailbox),
        )
        .prompt_context("Signing the commit")
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()
        .await
        .map(drop)
}

/// [`continue_operation`]'s way for a merge, cherry-pick, or revert whose checks are skipped:
/// the commit made without them, then the rest of a sequence picked.
async fn commit_then_continue(
    git: &Git,
    repository: &Repository,
    operation: Operation,
    cancel: &CancelToken,
) -> Result<(), GitError> {
    git.write("commit")
        .args(["--no-edit", "--no-verify"])
        .prompt_context("Signing the commit")
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()
        .await?;
    let subcommand = match operation {
        Operation::CherryPick => "cherry-pick",
        Operation::Revert => "revert",
        _ => return Ok(()),
    };
    let sequence = repository.git_dir().join("sequencer");
    let more = git
        .files()
        .exists(&sequence)
        .await
        .map_err(|source| GitError::Io { subcommand, source })?;
    if !more {
        return Ok(());
    }
    git.write(subcommand)
        .arg("--continue")
        .prompt_context("Signing the commit")
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()
        .await
        .map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_a_push_did_to_each_ref() {
        // A fast-forward's flag is a space (written out: a continued line loses its own).
        let output = b"To ../origin.git\n\
            \x20\trefs/heads/main:refs/heads/main\t38e2ea9..0f9147a\n\
            *\trefs/heads/new:refs/heads/new\t[new branch]\n\
            !\trefs/heads/old:refs/heads/old\t[rejected] (fetch first)\n\
            !\trefs/heads/x:refs/heads/x\t[remote rejected] (hook declined: no thanks)\n\
            +\trefs/heads/f:refs/heads/f\ta..b (forced update)\n\
            =\trefs/heads/s:refs/heads/s\t[up to date]\n\
            branch 'new' set up to track 'origin/new'.\n\
            Done\n";
        let (refs, notes) = parse_porcelain(output).unwrap();
        assert_eq!(notes, ["branch 'new' set up to track 'origin/new'."]);
        let flags: Vec<PushFlag> = refs.iter().map(|pushed| pushed.flag).collect();
        assert_eq!(
            flags,
            [
                PushFlag::FastForward,
                PushFlag::New,
                PushFlag::Rejected,
                PushFlag::Rejected,
                PushFlag::Forced,
                PushFlag::UpToDate
            ]
        );
        assert_eq!(refs[0].summary, "38e2ea9..0f9147a");
        assert_eq!(refs[0].reason, None);
        assert_eq!(refs[2].summary, "[rejected]");
        assert_eq!(refs[2].reason, Some(BString::from("fetch first")));
        assert_eq!(
            refs[3].reason,
            Some(BString::from("hook declined: no thanks"))
        );
        assert_eq!(refs[4].summary, "a..b");
        assert!(parse_porcelain(b"?\ta:b\tc\n").is_err());
    }
}
