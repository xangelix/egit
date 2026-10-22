//! Repositories to suggest on Home: where two repositories egit knows (open in a tab, or among the
//! recent) sit side by side in one folder, the other repositories in that folder are likely the
//! user's work too. Each such folder's repositories (its direct children holding a `.git`) are
//! gathered, those egit knows left out, and the most recently active first: when git last moved
//! the repository's `HEAD` (a commit, a switch, a pull, a rebase: its reflog's newest entry), else
//! its last commit's time; at most [`MOST`].
//!
//! Only repositories on this computer: a folder on another is listed only by a session there.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use bstr::ByteSlice as _;
use egit_git::Git;

/// The most suggested.
pub(crate) const MOST: usize = 20;

/// How many of a folder's entries are looked at, at most (a folder of everything).
const MOST_LOOKED_AT: usize = 500;

/// How many repositories are asked when they were last active, at most (the most recently
/// changed `.git`s first beyond).
const MOST_ASKED: usize = 300;

/// How many asked at once.
const AT_ONCE: usize = 8;

/// A repository suggested: its folder, and when it was last active (seconds since the Unix
/// epoch), where git says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Suggestion {
    pub(crate) path: PathBuf,
    pub(crate) active: Option<i64>,
}

/// The folders holding at least two of `known` directly.
pub(crate) fn shared_folders(known: &[PathBuf]) -> Vec<PathBuf> {
    let mut held: BTreeMap<&Path, BTreeSet<&Path>> = BTreeMap::new();
    for repository in known {
        if let Some(parent) = repository
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            held.entry(parent).or_default().insert(repository);
        }
    }
    held.into_iter()
        .filter(|(_, children)| children.len() >= 2)
        .map(|(parent, _)| parent.to_owned())
        .collect()
}

/// The repositories to suggest, given those egit knows: the others in the folders holding two of
/// them, the most recently active first, at most [`MOST`]; through `git` (this computer's).
pub(crate) fn suggest(git: &Git, known: &[PathBuf]) -> Vec<Suggestion> {
    let is_known = |path: &Path| known.iter().any(|repository| repository == path);
    let mut candidates: Vec<(PathBuf, Option<std::time::SystemTime>)> = Vec::new();
    for folder in shared_folders(known) {
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten().take(MOST_LOOKED_AT) {
            let path = entry.path();
            if is_known(&path) || !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            // A repository (or a worktree, whose `.git` is a file); its own time, to choose
            // among very many.
            let Ok(marker) = std::fs::symlink_metadata(path.join(".git")) else {
                continue;
            };
            if !candidates.iter().any(|(known, _)| *known == path) {
                candidates.push((path, marker.modified().ok()));
            }
        }
    }
    if candidates.len() > MOST_ASKED {
        candidates.sort_by_key(|(_, changed)| std::cmp::Reverse(*changed));
        candidates.truncate(MOST_ASKED);
    }
    let paths: Vec<PathBuf> = candidates.into_iter().map(|(path, _)| path).collect();
    let mut found: Vec<Suggestion> = Vec::with_capacity(paths.len());
    let size = paths.len().div_ceil(AT_ONCE).max(1);
    std::thread::scope(|scope| {
        let asked: Vec<_> = paths
            .chunks(size)
            .map(|chunk| {
                scope.spawn(move || {
                    chunk
                        .iter()
                        .map(|path| Suggestion {
                            active: last_active(git, path),
                            path: path.clone(),
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        for thread in asked {
            if let Ok(some) = thread.join() {
                found.extend(some);
            }
        }
    });
    // The most recently active first; those git says nothing of last, by name.
    found.sort_by(|a, b| b.active.cmp(&a.active).then_with(|| a.path.cmp(&b.path)));
    found.truncate(MOST);
    found
}

/// When git last moved `repository`'s `HEAD` (its reflog's newest entry), else its last
/// commit's time; none where git can't say (not a repository, no commit).
fn last_active(git: &Git, repository: &Path) -> Option<i64> {
    // `HEAD@{1700000000}`: the reflog entry's own time.
    let moved = git
        .read("log")
        .args(["-g", "-1", "--date=unix", "--format=%gd", "HEAD"])
        .current_dir(repository)
        .output()
        .ok()
        .and_then(|done| {
            let said = done.stdout.trim().to_str().ok()?;
            said.strip_prefix("HEAD@{")?
                .strip_suffix('}')?
                .parse::<i64>()
                .ok()
        });
    moved.or_else(|| {
        git.read("log")
            .args(["-1", "--format=%ct", "HEAD"])
            .current_dir(repository)
            .output()
            .ok()
            .and_then(|done| done.stdout.trim().to_str().ok()?.parse().ok())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_folders_holding_two_known_repositories_are_shared() {
        let known = [
            PathBuf::from("/work/a"),
            PathBuf::from("/work/b"),
            PathBuf::from("/play/c"),
            PathBuf::from("/deep/x/d"),
            PathBuf::from("/deep/e"),
            // The same one twice counts once.
            PathBuf::from("/play/c"),
        ];
        assert_eq!(shared_folders(&known), [PathBuf::from("/work")]);
    }
}
