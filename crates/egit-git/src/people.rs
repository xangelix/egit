//! The people of a repository's history: who wrote its recent commits, most often first (for
//! offering co-authors).

use std::collections::HashMap;

use bstr::{BString, ByteSlice as _};

use crate::cancel::CancelToken;
use crate::commit::Person;
use crate::error::GitError;
use crate::git::Git;
use crate::repo::Repository;

/// An author, and how many of the commits read they wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Author {
    /// Who, named as in their latest commit read.
    pub person: Person,
    /// How many of the commits read they wrote.
    pub commits: u32,
}

/// Who wrote the last `limit` commits of every branch, most commits first.
///
/// `log --all`, merges and the stash left out, read through `.mailmap` (`%aN`, `%aE`: one
/// person however many addresses they wrote with); each email (in any case) once, named as
/// they last were. Nobody before the first commit.
///
/// # Errors
///
/// When git fails or is cancelled.
pub fn recent_authors(
    git: &Git,
    repository: &Repository,
    limit: usize,
    cancel: &CancelToken,
) -> Result<Vec<Author>, GitError> {
    let output = git
        .read("log")
        .args(["--no-merges", "--format=%aN%x00%aE"])
        .arg(format!("--max-count={limit}"))
        .args(["--exclude=refs/stash", "--all", "--"])
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()?;
    let mut found: HashMap<BString, (usize, Author)> = HashMap::new();
    for (at, line) in output.stdout.lines().enumerate() {
        let Some((name, email)) = line.split_once_str(b"\0") else {
            continue;
        };
        if email.is_empty() {
            continue;
        }
        let key = email.to_ascii_lowercase().into();
        let entry = found.entry(key).or_insert_with(|| {
            (
                at,
                Author {
                    person: Person {
                        name: name.into(),
                        email: email.into(),
                    },
                    commits: 0,
                },
            )
        });
        entry.1.commits = entry.1.commits.saturating_add(1);
    }
    let mut authors: Vec<(usize, Author)> = found.into_values().collect();
    authors
        .sort_by(|(a_first, a), (b_first, b)| b.commits.cmp(&a.commits).then(a_first.cmp(b_first)));
    Ok(authors.into_iter().map(|(_, author)| author).collect())
}
