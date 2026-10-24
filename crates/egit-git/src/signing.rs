//! Signing commits already made, as `git rebase --force-rebase --gpg-sign` signs them, without
//! a rebase: each commit is made again by `git commit-tree -S` with its own tree, author, and
//! message, on its parents as made again before it, and the branch is moved to the new tip by
//! one `update-ref` that checks it is still where it was.
//!
//! Nothing is checked out or merged again: the work tree and the index stay as they are
//! (uncommitted changes don't stop it), no conflict can arise, and a merge keeps the resolution
//! it recorded. Every commit after one signed is made again too, its parent being new, and is
//! signed as well; whoever signs becomes their committer, now, as a rebase makes them. No hook
//! runs. Headers `commit-tree` can't be given (a merged tag's `mergetag`) are left behind.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::ops::ControlFlow;

use bstr::{BString, ByteSlice as _};

use crate::cancel::CancelToken;
use crate::commit::Commit;
use crate::error::GitError;
use crate::git::Git;
use crate::lines::Lines;
use crate::object_id::ObjectId;
use crate::objects::{ObjectKind, ObjectReader};
use crate::repo::Repository;

/// The most commits a signing makes again.
pub const MAX_SIGNED: usize = 10_000;

/// The most commits checked against the upstream (each a `merge-base`): those asked for that
/// no other one asked for is under, which a selection of rows nearly always makes one.
const UPSTREAM_CHECKS: usize = 64;

/// How many commits are read at once, each batch then made again one by one.
const READ_AT_ONCE: usize = 128;

/// What signing some commits would make again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SigningPlan {
    /// The branch checked out, by its full name (`refs/heads/main`).
    pub branch: BString,
    /// Its commit, the tip moved.
    pub tip: ObjectId,
    /// The commits made again, parents before children: those asked for, and every commit of
    /// the branch after them.
    pub commits: Vec<ObjectId>,
    /// How many of them were asked for.
    pub asked: usize,
    /// How many of them are merges.
    pub merges: usize,
    /// Whether the branch's upstream holds any of them: pushing after takes a force push.
    pub pushed: bool,
}

/// Why commits can't be signed.
#[derive(Debug, thiserror::Error)]
pub enum SignError {
    /// No commit was asked for.
    #[error("no commit to sign")]
    Nothing,
    /// `HEAD` isn't on a branch.
    #[error("HEAD isn't on a branch")]
    Detached,
    /// A commit asked for isn't in the branch's history.
    #[error("not every commit is in the current branch's history")]
    NotInBranch,
    /// More than [`MAX_SIGNED`] commits would be made again.
    #[error("more than {MAX_SIGNED} commits would be made again")]
    TooMany,
    /// A commit couldn't be read.
    #[error("commit {0} couldn't be read")]
    Unreadable(ObjectId),
    /// The branch moved while its commits were made again: nothing changed.
    #[error("the branch moved meanwhile")]
    Moved,
    /// Git failed (signing too: no key, a wrong passphrase), or was cancelled.
    #[error(transparent)]
    Git(#[from] GitError),
}

/// Commits signed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Signed {
    /// Where the branch was.
    pub from: ObjectId,
    /// Where it is now.
    pub to: ObjectId,
    /// How many commits were made again.
    pub made: usize,
}

/// Works out what signing `asked` (commits of the branch checked out) makes again: them, and
/// every commit of the branch after them, parents first.
///
/// # Errors
///
/// When none is asked for, `HEAD` is detached, one isn't in the branch's history, too many
/// would be made again, or git fails or is cancelled.
pub async fn plan_signing(
    git: &Git,
    repository: &Repository,
    asked: &[ObjectId],
    cancel: &CancelToken,
) -> Result<SigningPlan, SignError> {
    let asked: HashSet<ObjectId> = asked.iter().copied().collect();
    if asked.is_empty() {
        return Err(SignError::Nothing);
    }
    let head = repository.head_ref(git).await?;
    let (Some(branch), Some(tip)) = (head.reference, head.commit) else {
        return Err(SignError::Detached);
    };
    // In the branch's history, every one: nothing they reach that its tip doesn't.
    let mut input = String::new();
    for id in &asked {
        let _ = writeln!(input, "{id}");
    }
    let _ = writeln!(input, "^{tip}");
    let outside = git
        .read("rev-list")
        .args(["--count", "--stdin"])
        .stdin(input.into_bytes())
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()
        .await?;
    if outside.stdout.trim_ascii() != b"0" {
        return Err(SignError::NotInBranch);
    }
    let listed = newest_down_to(git, repository, tip, &asked, cancel).await?;
    // Oldest first: each made again when asked for, or when a parent is.
    let mut made: HashSet<ObjectId> = HashSet::with_capacity(listed.len());
    let (mut commits, mut merges, mut lowest) = (Vec::new(), 0_usize, Vec::new());
    for (id, parents) in listed.iter().rev() {
        let after = parents.iter().any(|parent| made.contains(parent));
        if asked.contains(id) || after {
            if asked.contains(id) && !after {
                lowest.push(*id);
            }
            made.insert(*id);
            commits.push(*id);
            if parents.len() > 1 {
                merges = merges.saturating_add(1);
            }
        }
    }
    // Pushed when the upstream holds one: then it holds one of the lowest asked for too.
    let pushed = match repository.upstream_tip(git).await? {
        Some((_, upstream)) => {
            let mut pushed = false;
            for commit in lowest.iter().take(UPSTREAM_CHECKS) {
                if reaches(git, repository, *commit, upstream).await? {
                    pushed = true;
                    break;
                }
            }
            pushed
        }
        None => false,
    };
    Ok(SigningPlan {
        branch,
        tip,
        commits,
        asked: asked.len(),
        merges,
        pushed,
    })
}

/// The branch's commits from its `tip` down, children before parents (`--topo-order`), each
/// with its parents, until every one `asked` for has come: by then every commit after them
/// has too.
async fn newest_down_to(
    git: &Git,
    repository: &Repository,
    tip: ObjectId,
    asked: &HashSet<ObjectId>,
    cancel: &CancelToken,
) -> Result<Vec<Listed>, SignError> {
    let mut listed = Vec::new();
    let (mut left, mut too_many) = (asked.len(), false);
    let mut handle = |line: &[u8]| {
        let mut ids = line.split_str(" ").filter_map(ObjectId::from_hex);
        let Some(id) = ids.next() else {
            return ControlFlow::Continue(());
        };
        listed.push((id, ids.collect()));
        if asked.contains(&id) {
            left = left.saturating_sub(1);
        }
        if left == 0 {
            return ControlFlow::Break(());
        }
        if listed.len() > MAX_SIGNED {
            too_many = true;
            return ControlFlow::Break(());
        }
        ControlFlow::Continue(())
    };
    let mut lines = Lines::default();
    git.read("rev-list")
        .args(["--topo-order", "--parents"])
        .arg(tip.to_string())
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .stream(|chunk| lines.feed(chunk, &mut handle))
        .await?;
    if too_many {
        return Err(SignError::TooMany);
    }
    if left > 0 {
        return Err(SignError::NotInBranch);
    }
    Ok(listed)
}

/// A commit listed, with its parents.
type Listed = (ObjectId, Box<[ObjectId]>);

/// Whether `from`'s history holds `commit` (`merge-base --is-ancestor`).
async fn reaches(
    git: &Git,
    repository: &Repository,
    commit: ObjectId,
    from: ObjectId,
) -> Result<bool, GitError> {
    let reached = git
        .read("merge-base")
        .arg("--is-ancestor")
        .arg(commit.to_string())
        .arg(from.to_string())
        .current_dir(repository.work_tree())
        .output_unchecked()
        .await?;
    match reached.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(GitError::failed(
            "merge-base",
            reached.status,
            reached.stderr,
        )),
    }
}

/// Signs `plan`'s commits, then moves the branch to the new tip if it is still at the plan's.
///
/// Each is made again and signed (`commit-tree -S`, by the configured key and format; a
/// passphrase asked through askpass, labelled "Signing the commits"). `done` hears how many are
/// made so far. Nothing changes until the branch moves: cancelled before, it stays (the new
/// commits, unreachable, go with git's next clean-up).
///
/// # Errors
///
/// When a commit can't be read, signing fails (no key, a wrong passphrase), the branch moved,
/// or git fails or is cancelled.
pub async fn sign_commits(
    git: &Git,
    repository: &Repository,
    plan: &SigningPlan,
    cancel: &CancelToken,
    mut done: impl FnMut(usize),
) -> Result<Signed, SignError> {
    let mut reader = ObjectReader::start(git, repository.work_tree()).await?;
    let mut made: HashMap<ObjectId, ObjectId> = HashMap::with_capacity(plan.commits.len());
    for batch in plan.commits.chunks(READ_AT_ONCE) {
        let mut read = Vec::with_capacity(batch.len());
        reader
            .read(batch, |id, object| {
                let parsed = object
                    .filter(|object| object.kind == ObjectKind::Commit)
                    .and_then(|object| Commit::parse(object.data).ok());
                read.push((id, parsed));
            })
            .await?;
        for (id, commit) in read {
            let commit = commit.ok_or(SignError::Unreadable(id))?;
            if cancel.is_cancelled() {
                return Err(GitError::Cancelled {
                    subcommand: "commit-tree",
                }
                .into());
            }
            let signed = make_again(git, repository, &commit, &made, cancel).await?;
            made.insert(id, signed);
            done(made.len());
        }
    }
    let to = *made.get(&plan.tip).ok_or(SignError::Unreadable(plan.tip))?;
    let moved = git
        .write("update-ref")
        .arg("-m")
        .arg(format!("egit: signed {} commits", made.len()))
        .arg("--")
        .arg(plan.branch.to_os_str_lossy())
        .arg(to.to_string())
        .arg(plan.tip.to_string())
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output_unchecked()
        .await?;
    if !moved.status.success() {
        return Err(if moved.stderr.as_bytes().contains_str("but expected") {
            SignError::Moved
        } else {
            GitError::failed("update-ref", moved.status, moved.stderr).into()
        });
    }
    Ok(Signed {
        from: plan.tip,
        to,
        made: made.len(),
    })
}

/// `commit` made again and signed, on its parents as `made` maps them: its tree, author (name,
/// email, and date in their own time zone), and message as they were, byte for byte.
async fn make_again(
    git: &Git,
    repository: &Repository,
    commit: &Commit,
    made: &HashMap<ObjectId, ObjectId>,
    cancel: &CancelToken,
) -> Result<ObjectId, GitError> {
    let author = &commit.author;
    let offset = author.offset_minutes.unsigned_abs();
    let date = format!(
        "@{} {}{:02}{:02}",
        author.time,
        if author.offset_minutes < 0 { '-' } else { '+' },
        offset.div_euclid(60),
        offset.rem_euclid(60)
    );
    let mut command = git
        .write("commit-tree")
        .arg(commit.tree.to_string())
        .arg("-S")
        .env("GIT_AUTHOR_NAME", author.name.to_os_str_lossy())
        .env("GIT_AUTHOR_EMAIL", author.email.to_os_str_lossy())
        .env("GIT_AUTHOR_DATE", date)
        .prompt_context("Signing the commits")
        .stdin(commit.message.to_vec())
        .current_dir(repository.work_tree())
        .cancel_token(cancel);
    for parent in &commit.parents {
        let parent = made.get(parent).unwrap_or(parent);
        command = command.arg("-p").arg(parent.to_string());
    }
    // A message in another encoding keeps it: `commit-tree` writes the header it is told.
    if let Some(encoding) = &commit.encoding {
        command = command.config("i18n.commitEncoding", encoding.to_os_str_lossy());
    }
    let output = command.output().await?;
    ObjectId::from_hex(output.stdout.trim_ascii())
        .ok_or_else(|| GitError::failed("commit-tree", output.status, output.stderr))
}
