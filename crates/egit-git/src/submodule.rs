//! A repository's submodules: each one `.gitmodules` names and the index records (its name,
//! path, address and branch, the commit recorded, and whether `submodule init` set it up here,
//! with the address it set up); where each stands (`submodule status`: not set up, at the
//! commit recorded, at another, in conflict; nested ones too); one's details (its `HEAD`, a
//! branch if it is on one, how far it is from the commit recorded) and the commits between two
//! of its commits; and everything done to them: set up, update (to the commits recorded, or
//! the latest of their branches), sync their addresses, deinit, add, remove, change an
//! address or a branch.
//!
//! What a submodule is, for whoever meets one: another repository checked out inside this
//! one, at a commit this one records (a "gitlink" in its tree). It is checked out at that
//! commit, not on a branch (its `HEAD` detached): that is normal. Committing in it moves its
//! `HEAD` away from the commit recorded; staging its path here records the new one.

use bstr::{BStr, BString, ByteSlice as _};

use crate::cancel::CancelToken;
use crate::error::GitError;
use crate::git::Git;
use crate::object_id::ObjectId;
use crate::progress::Progress;
use crate::repo::Repository;

/// A submodule of the repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Submodule {
    /// Its name in `.gitmodules` (often its path).
    pub name: BString,
    /// Where it is, from the work tree's top.
    pub path: BString,
    /// Where it comes from, as `.gitmodules` says.
    pub url: Option<BString>,
    /// The commit the index records for it.
    pub recorded: ObjectId,
    /// Whether it is set up here (`submodule.<name>.url` in the repository's configuration, as
    /// `submodule init` sets it).
    pub initialized: bool,
    /// The branch `.gitmodules` names for updating it to its latest (`--remote`), if any.
    pub branch: Option<BString>,
    /// The address set up here (`submodule.<name>.url`): what fetches use, which
    /// `.gitmodules` may since have changed (`submodule sync` brings it along).
    pub configured_url: Option<BString>,
}

/// The submodules `.gitmodules` names that the index records, by path.
///
/// # Errors
///
/// When git fails or prints something unexpected.
pub async fn submodules(git: &Git, repository: &Repository) -> Result<Vec<Submodule>, GitError> {
    let declared =
        config_entries(git, repository, &["--file", ".gitmodules"], "^submodule\\.").await?;
    // Each name's path, address, and branch, in the order `.gitmodules` gives them.
    let mut named: Vec<Declared> = Vec::new();
    for (key, value) in declared {
        let Some((name, variable)) = key
            .strip_prefix(b"submodule.")
            .and_then(|rest| rest.rsplit_once_str(b"."))
        else {
            continue;
        };
        let entry = if let Some(at) = named.iter().position(|known| known.name == name) {
            named.get_mut(at)
        } else {
            named.push(Declared {
                name: BString::from(name),
                path: None,
                url: None,
                branch: None,
            });
            named.last_mut()
        };
        let Some(entry) = entry else {
            continue;
        };
        match variable {
            b"path" => entry.path = Some(value),
            b"url" => entry.url = Some(value),
            b"branch" => entry.branch = Some(value),
            _ => {}
        }
    }
    let paths: Vec<&BString> = named
        .iter()
        .filter_map(|known| known.path.as_ref())
        .collect();
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let recorded = gitlinks(git, repository, &paths).await?;
    // Each name set up here, and the address it was set up with.
    let initialized: Vec<(BString, BString)> =
        config_entries(git, repository, &["--local"], "^submodule\\..*\\.url$")
            .await?
            .into_iter()
            .filter_map(|(key, value)| {
                key.strip_prefix(b"submodule.")
                    .and_then(|rest| rest.strip_suffix(b".url"))
                    .map(|name| (BString::from(name), value))
            })
            .collect();
    let mut found: Vec<Submodule> = named
        .into_iter()
        .filter_map(
            |Declared {
                 name,
                 path,
                 url,
                 branch,
             }| {
                let path = path?;
                let (_, commit) = recorded.iter().find(|(known, _)| *known == path)?;
                let configured_url = initialized
                    .iter()
                    .find(|(known, _)| *known == name)
                    .map(|(_, url)| url.clone());
                Some(Submodule {
                    initialized: configured_url.is_some(),
                    configured_url,
                    recorded: *commit,
                    name,
                    path,
                    url,
                    branch,
                })
            },
        )
        .collect();
    found.sort_unstable_by(|a, b| a.path.cmp(&b.path));
    Ok(found)
}

/// A submodule as `.gitmodules` declares it.
struct Declared {
    name: BString,
    path: Option<BString>,
    url: Option<BString>,
    branch: Option<BString>,
}

/// The keys matching `pattern` and their values in the configuration `place` names (`-z
/// --get-regexp`: `key\nvalue\0` each); none when nothing matches or the file is missing.
async fn config_entries(
    git: &Git,
    repository: &Repository,
    place: &[&str],
    pattern: &str,
) -> Result<Vec<(BString, BString)>, GitError> {
    let listed = git
        .read("config")
        .args(place)
        .args(["-z", "--get-regexp", pattern])
        .current_dir(repository.work_tree())
        .output_unchecked()
        .await?;
    match listed.status.code() {
        Some(0) => {}
        // Nothing matched, or no such file.
        Some(1) => return Ok(Vec::new()),
        _ => return Err(GitError::failed("config", listed.status, listed.stderr)),
    }
    Ok(listed
        .stdout
        .split_str(b"\0")
        .filter(|record| !record.is_empty())
        .map(|record| {
            let (key, value) = record.split_once_str(b"\n").unwrap_or((record, b""));
            (BString::from(key), BString::from(value))
        })
        .collect())
}

/// The commits the index records for those of `paths` that are submodules (`ls-files -s -z`,
/// literal paths), the first stage of each when in conflict.
async fn gitlinks(
    git: &Git,
    repository: &Repository,
    paths: &[&BString],
) -> Result<Vec<(BString, ObjectId)>, GitError> {
    let listed = git
        .read("ls-files")
        .args(["-s", "-z", "--"])
        .args(paths.iter().map(|path| path.to_os_str_lossy()))
        .env("GIT_LITERAL_PATHSPECS", "1")
        .current_dir(repository.work_tree())
        .output()
        .await?;
    let mut found: Vec<(BString, ObjectId)> = Vec::new();
    for record in listed
        .stdout
        .split_str(b"\0")
        .filter(|record| !record.is_empty())
    {
        let malformed = || GitError::unexpected("ls-files", "an index entry");
        let (meta, path) = record.split_once_str(b"\t").ok_or_else(malformed)?;
        let mut fields = meta.split_str(b" ");
        let (Some(mode), Some(id)) = (fields.next(), fields.next()) else {
            return Err(malformed());
        };
        if mode != b"160000" || found.iter().any(|(known, _)| known == path) {
            continue;
        }
        found.push((
            BString::from(path),
            ObjectId::from_hex(id).ok_or_else(malformed)?,
        ));
    }
    Ok(found)
}

/// Brings the submodules at `paths` (all of them when empty) to the commits recorded, cloning
/// and setting up those not here yet, and theirs in turn (`submodule update --init
/// --recursive`).
///
/// # Errors
///
/// When git fails (a submodule can't be fetched, has changes in the way) or is cancelled.
pub async fn update_submodules(
    git: &Git,
    repository: &Repository,
    paths: &[&BStr],
    progress: &Progress,
    cancel: &CancelToken,
) -> Result<(), GitError> {
    update_submodules_to(git, repository, paths, UpdateTo::Recorded, progress, cancel).await
}

/// Where an update takes submodules.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateTo {
    /// The commits this repository records (what checking it out asks for).
    Recorded,
    /// The latest commit of each one's branch on its remote (`--remote`: the branch
    /// `.gitmodules` names, else its remote's default), fetched first; this repository then
    /// shows them moved, to commit.
    Latest,
}

/// Updates the submodules at `paths` (all of them when empty), setting up those not here yet,
/// and theirs in turn, to where `to` says (`submodule update --init --recursive`, `--remote`
/// for the latest).
///
/// # Errors
///
/// When git fails (a submodule can't be fetched, has changes in the way) or is cancelled.
pub async fn update_submodules_to(
    git: &Git,
    repository: &Repository,
    paths: &[&BStr],
    to: UpdateTo,
    progress: &Progress,
    cancel: &CancelToken,
) -> Result<(), GitError> {
    let mut command =
        git.write("submodule")
            .args(["update", "--init", "--recursive", "--progress"]);
    if to == UpdateTo::Latest {
        command = command.arg("--remote");
    }
    command
        .arg("--")
        .args(paths.iter().map(|path| path.to_os_str_lossy()))
        .env("GIT_LITERAL_PATHSPECS", "1")
        .prompt_context("Updating submodules")
        .current_dir(repository.work_tree())
        .progress(progress)
        .cancel_token(cancel)
        .output()
        .await
        .map(drop)
}

/// Where a submodule stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Checkout {
    /// Not set up here: its folder is empty.
    NotInitialized,
    /// At the commit recorded.
    AtRecorded,
    /// At another commit than the one recorded (moved by a commit in it, a checkout in it, or
    /// a pull here that recorded a new one it isn't updated to yet).
    Elsewhere,
    /// In a merge conflict: the sides record different commits.
    Conflict,
}

/// A submodule's place, as `submodule status` gives it: nested ones too.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubmoduleStatus {
    /// Its path from the top of this work tree (a nested one's through its parents').
    pub path: BString,
    /// Where it stands.
    pub checkout: Checkout,
    /// The commit it is at (the one recorded, when not set up).
    pub commit: ObjectId,
    /// That commit named by the nearest ref (`heads/main`, `v1.2-3-gabcdef`), when git found one.
    pub describe: Option<BString>,
}

/// Where each submodule stands (`submodule status`, `--recursive` for nested ones too).
///
/// # Errors
///
/// When git fails or prints something unexpected.
pub async fn submodule_status(
    git: &Git,
    repository: &Repository,
    recursive: bool,
    cancel: &CancelToken,
) -> Result<Vec<SubmoduleStatus>, GitError> {
    let mut command = git
        .read("submodule")
        .arg("status")
        .current_dir(repository.work_tree())
        .cancel_token(cancel);
    if recursive {
        command = command.arg("--recursive");
    }
    let listed = command.output().await?;
    listed
        .stdout
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            parse_status(line).ok_or_else(|| GitError::unexpected("submodule status", "a line"))
        })
        .collect()
}

/// Reads ` 1234… path (describe)`: its first character the state.
fn parse_status(line: &[u8]) -> Option<SubmoduleStatus> {
    let (&mark, rest) = line.split_first()?;
    let checkout = match mark {
        b'-' => Checkout::NotInitialized,
        b' ' => Checkout::AtRecorded,
        b'+' => Checkout::Elsewhere,
        b'U' => Checkout::Conflict,
        _ => return None,
    };
    let (id, rest) = rest.split_once_str(b" ")?;
    let commit = ObjectId::from_hex(id)?;
    let (path, describe) = match rest.rfind(b" (") {
        Some(open) if rest.ends_with(b")") => (
            rest.get(..open)?,
            Some(BString::from(
                rest.get(open.checked_add(2)?..rest.len().checked_sub(1)?)?,
            )),
        ),
        _ => (rest, None),
    };
    Some(SubmoduleStatus {
        path: path.into(),
        checkout,
        commit,
        describe,
    })
}

/// What a submodule set up here is at, inside.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubmoduleDetails {
    /// Its `HEAD`'s commit.
    pub head: Option<ObjectId>,
    /// The branch it is on, if it isn't detached (submodules usually are).
    pub branch: Option<BString>,
    /// Whether it has the commit recorded (else it must be fetched first).
    pub has_recorded: bool,
    /// Commits its `HEAD` has that the commit recorded hasn't.
    pub ahead: u32,
    /// Commits the commit recorded has that its `HEAD` hasn't.
    pub behind: u32,
}

/// What the submodule at `path` (set up here) is at, against `recorded`.
///
/// # Errors
///
/// When git fails in it.
pub async fn submodule_details(
    git: &Git,
    repository: &Repository,
    path: &BStr,
    recorded: ObjectId,
) -> Result<SubmoduleDetails, GitError> {
    let inside = repository.work_tree().join(path.to_path_lossy());
    let read = |args: &[&str]| {
        git.read("rev-parse")
            .args(args)
            .current_dir(&inside)
            .output_unchecked()
    };
    let head = read(&["--verify", "--quiet", "HEAD"]).await?;
    let head = head
        .status
        .success()
        .then(|| ObjectId::from_hex(head.stdout.trim_ascii()))
        .flatten();
    let branch = git
        .read("symbolic-ref")
        .args(["--quiet", "--short", "HEAD"])
        .current_dir(&inside)
        .output_unchecked()
        .await?;
    let branch = branch
        .status
        .success()
        .then(|| BString::from(branch.stdout.trim_ascii()));
    let recorded_name = format!("{recorded}^{{commit}}");
    let has_recorded = read(&["--verify", "--quiet", "--end-of-options", &recorded_name])
        .await?
        .status
        .success();
    let (mut ahead, mut behind) = (0, 0);
    if let Some(head) = head
        && has_recorded
        && head != recorded
    {
        let counted = git
            .read("rev-list")
            .args(["--left-right", "--count"])
            .arg(format!("{head}...{recorded}"))
            .arg("--")
            .current_dir(&inside)
            .output()
            .await?;
        let mut counts = counted
            .stdout
            .trim_ascii()
            .split(u8::is_ascii_whitespace)
            .filter_map(|count| count.to_str().ok()?.parse().ok());
        ahead = counts.next().unwrap_or(0);
        behind = counts.next().unwrap_or(0);
    }
    Ok(SubmoduleDetails {
        head,
        branch,
        has_recorded,
        ahead,
        behind,
    })
}

/// The commits between `from` and `to` in the submodule at `path`: what a change of the
/// commit recorded brings.
///
/// `log from..to`, newest first, at most `limit` (with neither ancestor of the other, what
/// `to` has). `None` when the submodule isn't set up here or lacks them.
///
/// # Errors
///
/// When git can't be run.
pub async fn submodule_commits(
    git: &Git,
    repository: &Repository,
    path: &BStr,
    (from, to): (Option<ObjectId>, ObjectId),
    limit: usize,
) -> Result<Option<Vec<(ObjectId, BString)>>, GitError> {
    let inside = repository.work_tree().join(path.to_path_lossy());
    if !git
        .files()
        .exists(&inside.join(".git"))
        .await
        .unwrap_or(false)
    {
        return Ok(None);
    }
    let mut command = git
        .read("log")
        .arg(format!("--max-count={limit}"))
        .args(["-z", "--format=%H %s", "--end-of-options"])
        .arg(to.to_string());
    if let Some(from) = from {
        command = command.arg(format!("^{from}"));
    }
    let listed = command
        .arg("--")
        .current_dir(&inside)
        .output_unchecked()
        .await?;
    if !listed.status.success() {
        return Ok(None);
    }
    Ok(Some(
        listed
            .stdout
            .split_str(b"\0")
            .filter_map(|record| {
                let (id, subject) = record.trim_ascii_start().split_once_str(b" ")?;
                Some((ObjectId::from_hex(id)?, BString::from(subject)))
            })
            .collect(),
    ))
}

/// What is done to submodules (each at `paths`, all of them when empty).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubmoduleChange {
    /// Set them up from `.gitmodules`, without cloning (`submodule init`).
    Init,
    /// Bring the addresses set up here (and in their own submodules) to what `.gitmodules`
    /// says now (`submodule sync --recursive`).
    Sync,
    /// Forget them here: their folders emptied, their setup removed (`submodule deinit`;
    /// `force` though they have changes); still recorded, set up again with an update.
    Deinit {
        /// Though they have changes, which are lost.
        force: bool,
    },
}

/// Does `change` to the submodules at `paths` (all of them when empty).
///
/// # Errors
///
/// When git fails (a deinit of a submodule with changes, unforced).
pub async fn change_submodules(
    git: &Git,
    repository: &Repository,
    paths: &[&BStr],
    change: &SubmoduleChange,
) -> Result<(), GitError> {
    let mut command = git
        .write("submodule")
        .env("GIT_LITERAL_PATHSPECS", "1")
        .current_dir(repository.work_tree());
    command = match change {
        SubmoduleChange::Init => command.args(["--quiet", "init"]),
        SubmoduleChange::Sync => command.args(["--quiet", "sync", "--recursive"]),
        SubmoduleChange::Deinit { force } => {
            let command = command.args(["--quiet", "deinit"]);
            match (force, paths.is_empty()) {
                (true, true) => command.args(["--force", "--all"]),
                (true, false) => command.arg("--force"),
                (false, true) => command.arg("--all"),
                (false, false) => command,
            }
        }
    };
    command
        .arg("--")
        .args(paths.iter().map(|path| path.to_os_str_lossy()))
        .output()
        .await
        .map(drop)
}

/// Adds the repository at `url` as a submodule at `path` (`submodule add`, following `branch`
/// when given), cloning it: `.gitmodules` and the new gitlink staged, to commit.
///
/// # Errors
///
/// When git fails: a path taken, an address it can't clone.
pub async fn add_submodule(
    git: &Git,
    repository: &Repository,
    url: &str,
    path: &BStr,
    branch: Option<&str>,
    progress: &Progress,
    cancel: &CancelToken,
) -> Result<(), GitError> {
    let mut command = git
        .write("submodule")
        .args(["add", "--progress"])
        .prompt_context(format!("Adding the submodule {url}"))
        .current_dir(repository.work_tree())
        .progress(progress)
        .cancel_token(cancel);
    if let Some(branch) = branch {
        command = command.args(["--branch", branch]);
    }
    command
        .arg("--")
        .arg(url)
        .arg(path.to_os_str_lossy())
        .output()
        .await
        .map(drop)
}

/// Removes the submodule at `path` (`git rm`): its folder, gitlink, and `.gitmodules` section.
///
/// That is staged, to commit, with whatever else `.gitmodules` holds that isn't staged yet (git
/// refuses to edit it otherwise). Its repository stays in the git dir (`modules/`), so checking
/// out a commit that has it brings it back without cloning.
///
/// # Errors
///
/// When git fails: changes in it, unforced.
pub async fn remove_submodule(
    git: &Git,
    repository: &Repository,
    path: &BStr,
    force: bool,
) -> Result<(), GitError> {
    if git
        .files()
        .is_file(&repository.work_tree().join(".gitmodules"))
        .await
    {
        git.write("add")
            .args(["--", ".gitmodules"])
            .current_dir(repository.work_tree())
            .output()
            .await?;
    }
    let mut command = git
        .write("rm")
        .arg("--quiet")
        .env("GIT_LITERAL_PATHSPECS", "1")
        .current_dir(repository.work_tree());
    if force {
        command = command.arg("--force");
    }
    command
        .arg("--")
        .arg(path.to_os_str_lossy())
        .output()
        .await
        .map(drop)
}

/// Points the submodule at `path` at `url` in `.gitmodules` and here (`submodule set-url`),
/// to commit.
///
/// # Errors
///
/// When git fails.
pub async fn set_submodule_url(
    git: &Git,
    repository: &Repository,
    path: &BStr,
    url: &str,
) -> Result<(), GitError> {
    git.write("submodule")
        .args(["--quiet", "set-url", "--"])
        .arg(path.to_os_str_lossy())
        .arg(url)
        .current_dir(repository.work_tree())
        .output()
        .await
        .map(drop)
}

/// Sets the branch the submodule at `path` follows for `--remote` updates (`submodule
/// set-branch`; `None`: its remote's default again), in `.gitmodules`, to commit.
///
/// # Errors
///
/// When git fails.
pub async fn set_submodule_branch(
    git: &Git,
    repository: &Repository,
    path: &BStr,
    branch: Option<&str>,
) -> Result<(), GitError> {
    let mut command = git
        .write("submodule")
        .args(["--quiet", "set-branch"])
        .current_dir(repository.work_tree());
    command = match branch {
        Some(branch) => command.args(["--branch", branch]),
        None => command.arg("--default"),
    };
    command
        .arg("--")
        .arg(path.to_os_str_lossy())
        .output()
        .await
        .map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "87523ccdc7076cf5e94b51236f4682cdf5f48a95";

    #[test]
    fn status_lines_are_read() {
        let line = format!("+{ID} libs/lib one (heads/main)");
        let status = parse_status(line.as_bytes()).unwrap();
        assert_eq!(status.checkout, Checkout::Elsewhere);
        assert_eq!(status.path, "libs/lib one");
        assert_eq!(status.describe, Some(BString::from("heads/main")));
        let line = format!("-{ID} vendor/x");
        let status = parse_status(line.as_bytes()).unwrap();
        assert_eq!(status.checkout, Checkout::NotInitialized);
        assert_eq!(status.describe, None);
        assert_eq!(
            parse_status(format!("U{ID} a").as_bytes())
                .unwrap()
                .checkout,
            Checkout::Conflict
        );
        assert!(parse_status(b"?nonsense").is_none());
    }
}
