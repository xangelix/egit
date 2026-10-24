//! Commits picked together, exported as a task: as patches, one each (into a folder, or copied
//! as one mailbox `git am` takes), their changes as one patch (into a file, or copied), or a
//! changelog copied: a Markdown list of their subjects, each linked to its page on the
//! repository's forge.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use egit_forge::ForgeRepository;
use egit_git::{
    CancelToken, CombineError, Git, GitError, ListedCommit, ObjectId, Repository, combined_patch,
    format_series, format_series_into, list_commits,
};

use crate::format;
use crate::problem::Problem;
use crate::tasks::Outcome;

/// The digits of a commit's name a changelog shows.
const ID_DIGITS: usize = 8;

/// What commits picked together become.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Export {
    /// One patch each, the oldest first, into a folder.
    Patches(PathBuf),
    /// One patch each, copied as one text.
    CopyPatches,
    /// Their changes as one patch, into a file.
    OnePatch(PathBuf),
    /// Their changes as one patch, copied.
    CopyOnePatch,
    /// A Markdown list of their subjects, the oldest first, linked to their pages on the
    /// repository's forge when it is on one.
    CopyChangelog(Option<ForgeRepository>),
}

impl Export {
    /// What it is doing, for the task bar.
    pub(crate) const fn doing(&self) -> &'static str {
        match self {
            Self::Patches(_) | Self::CopyPatches => "Making the patches",
            Self::OnePatch(_) | Self::CopyOnePatch => "Combining the changes",
            Self::CopyChangelog(_) => "Listing the commits",
        }
    }

    /// What it did with `commits` commits (`made`: the patches, or the changelog's lines), in
    /// words.
    pub(crate) fn done(&self, commits: usize, made: usize, home: Option<&Path>) -> String {
        let left_out = if made < commits {
            " Merges are left out: git makes no patch of a merge."
        } else {
            ""
        };
        match self {
            Self::Patches(folder) => format!(
                "Saved {} to {}.{left_out}",
                plural(made, "patch", "patches"),
                format::display_path(folder, home)
            ),
            Self::CopyPatches => format!(
                "Copied {} as patches.{left_out}",
                plural(made, "commit", "commits")
            ),
            Self::OnePatch(path) => format!(
                "Saved the changes of {} as one patch to {}.",
                plural(commits, "commit", "commits"),
                format::display_path(path, home)
            ),
            Self::CopyOnePatch => format!(
                "Copied the changes of {} as one patch.",
                plural(commits, "commit", "commits")
            ),
            Self::CopyChangelog(_) => format!(
                "Copied a changelog of {}.{}",
                plural(made, "commit", "commits"),
                if made < commits {
                    " Merges are left out."
                } else {
                    ""
                }
            ),
        }
    }
}

/// `count` and its noun.
fn plural(count: usize, one: &str, many: &str) -> String {
    if count == 1 {
        format!("1 {one}")
    } else {
        format!("{count} {many}")
    }
}

/// Exports `commits` (the newest first, as a history lists them) as `export` says.
pub(crate) async fn run(
    git: &Git,
    repository: &Repository,
    commits: &[ObjectId],
    export: &Export,
    cancel: &CancelToken,
) -> Result<Outcome, Problem> {
    let made = match export {
        Export::Patches(folder) => format_series_into(git, repository, commits, folder, cancel)
            .await
            .map(|written| (written.len(), None)),
        Export::CopyPatches => format_series(git, repository, commits, cancel)
            .await
            .map(|series| (series.patches, Some(series.text))),
        Export::OnePatch(file) => {
            let combined = match combined_patch(git, repository, commits, cancel).await {
                Ok(combined) => combined,
                Err(error) => return combine_failed(error),
            };
            return std::fs::write(file, combined)
                .map(|()| Outcome::Exported {
                    made: commits.len(),
                    copied: None,
                })
                .map_err(|error| {
                    Problem::new(
                        "The patch couldn't be saved",
                        format!("{}: {error}.", file.display()),
                    )
                });
        }
        Export::CopyOnePatch => match combined_patch(git, repository, commits, cancel).await {
            Ok(patch) => Ok((commits.len(), Some(patch))),
            Err(error) => return combine_failed(error),
        },
        Export::CopyChangelog(forge) => list_commits(git, repository, commits, cancel)
            .await
            .map(|listed| changelog(&listed, forge.as_ref()))
            .map(|(text, lines)| (lines, Some(text.into_bytes()))),
    };
    match made {
        Ok((0, _)) => Err(Problem::new(
            "There is nothing to export",
            "They are all merges: git makes no patch of a merge.",
        )),
        Ok((made, None)) => Ok(Outcome::Exported { made, copied: None }),
        Ok((made, Some(text))) => {
            let text = String::from_utf8(text).map_err(|_| {
                Problem::new(
                    "It can't be copied",
                    "It holds text that isn't UTF-8, which the clipboard can't take: save it \
                     instead.",
                )
            })?;
            Ok(Outcome::Exported {
                made,
                copied: Some(Arc::from(text)),
            })
        }
        Err(GitError::Cancelled { .. }) => Ok(Outcome::Cancelled),
        Err(error) => Err(Problem::git("The commits couldn't be exported", &error)),
    }
}

/// Why their changes couldn't be combined, for people (cancelling isn't a failure).
fn combine_failed(error: CombineError) -> Result<Outcome, Problem> {
    const TITLE: &str = "Their changes don't combine into one patch";
    match error {
        CombineError::Git(GitError::Cancelled { .. }) => Ok(Outcome::Cancelled),
        CombineError::Git(error) => Err(Problem::git(TITLE, &error)),
        CombineError::Nothing => Err(Problem::new(TITLE, "No commits were picked.")),
        CombineError::Conflict(commit) => Err(Problem::new(
            TITLE,
            format!(
                "{} conflicts with the commits picked before it: it needs a commit left out \
                 between them. Pick that one too, or save them as separate patches.",
                commit.abbreviated(ID_DIGITS)
            ),
        )),
    }
}

/// A Markdown list of `listed`'s subjects (given the newest first), the oldest first, each
/// with its name, linked to its page on `forge` when there is one; merges are left out unless
/// they are all there is. Returns it, and how many lines it holds.
pub(crate) fn changelog(
    listed: &[ListedCommit],
    forge: Option<&ForgeRepository>,
) -> (String, usize) {
    let merges_only = listed.iter().all(|commit| commit.parents.len() > 1);
    let mut text = String::new();
    let mut lines = 0_usize;
    for commit in listed
        .iter()
        .rev()
        .filter(|commit| merges_only || commit.parents.len() < 2)
    {
        let short = commit.id.abbreviated(ID_DIGITS);
        // Writing into a `String` can't fail.
        let _ = match forge {
            Some(repository) => writeln!(
                text,
                "- {} ([{short}]({}))",
                commit.subject,
                repository.commit_url(commit.id)
            ),
            None => writeln!(text, "- {} ({short})", commit.subject),
        };
        lines = lines.saturating_add(1);
    }
    (text, lines)
}

#[cfg(test)]
mod tests {
    use egit_forge::{ForgeHosts, forge_repository};

    use super::*;

    fn listed(digit: &str, subject: &str, parents: usize) -> ListedCommit {
        let id = |digit: &str| ObjectId::from_hex(digit.repeat(40).as_bytes()).unwrap();
        ListedCommit {
            id: id(digit),
            parents: (0..parents).map(|_| id("f")).collect(),
            subject: subject.to_owned(),
        }
    }

    #[test]
    fn a_changelog_lists_subjects_the_oldest_first_linked_on_a_forge() {
        // The newest first, as picked.
        let commits = [
            listed("c", "Fix the parser", 1),
            listed("b", "Merge branch 'topic'", 2),
            listed("a", "Add a parser", 1),
        ];
        let (text, lines) = changelog(&commits, None);
        assert_eq!(
            text,
            "- Add a parser (aaaaaaaa)\n- Fix the parser (cccccccc)\n"
        );
        assert_eq!(lines, 2, "the merge left out");
        let repository = forge_repository(
            "https://github.com/octo/parser.git",
            &ForgeHosts::default(),
            |_| None,
        )
        .unwrap();
        let (text, _) = changelog(&commits[..1], Some(&repository));
        assert_eq!(
            text,
            format!(
                "- Fix the parser ([cccccccc](https://github.com/octo/parser/commit/{}))\n",
                "c".repeat(40)
            )
        );
        // Merges alone: listed after all.
        let (_, lines) = changelog(&commits[1..2], None);
        assert_eq!(lines, 1);
    }

    #[test]
    fn says_what_it_made() {
        let home = Path::new("/home/ada");
        let folder = Export::Patches(PathBuf::from("/home/ada/out"));
        assert_eq!(folder.done(3, 3, Some(home)), "Saved 3 patches to ~/out.");
        assert!(folder.done(3, 2, None).contains("Merges are left out"));
        assert_eq!(
            Export::CopyOnePatch.done(1, 1, None),
            "Copied the changes of 1 commit as one patch."
        );
        assert_eq!(
            Export::CopyChangelog(None).done(4, 3, None),
            "Copied a changelog of 3 commits. Merges are left out."
        );
    }
}
