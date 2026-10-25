//! The people of a repository's history: who wrote its recent commits, most often first (for
//! offering co-authors).

use std::collections::HashMap;

use bstr::{BStr, BString, ByteSlice as _};

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
pub async fn recent_authors(
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
        .output()
        .await?;
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

/// Each of `contacts` (`Name <email>`) as the repository's mailmap names its person, in order.
///
/// Through `check-mailmap --stdin`: one the mailmap doesn't know, or that isn't a contact git
/// can read, comes back as it is.
///
/// # Errors
///
/// When git fails or is cancelled.
pub async fn map_contacts(
    git: &Git,
    repository: &Repository,
    contacts: &[&BStr],
    cancel: &CancelToken,
) -> Result<Vec<BString>, GitError> {
    // `check-mailmap` refuses (and stops at) a line that isn't `Name <email>`.
    let readable = |contact: &BStr| {
        contact.ends_with(b">") && contact.contains(&b'<') && !contact.contains(&b'\n')
    };
    let mut stdin = Vec::new();
    for contact in contacts.iter().filter(|contact| readable(contact)) {
        stdin.extend_from_slice(contact);
        stdin.push(b'\n');
    }
    if stdin.is_empty() {
        return Ok(contacts
            .iter()
            .map(|contact| (*contact).to_owned())
            .collect());
    }
    let mapped = git
        .read("check-mailmap")
        .arg("--stdin")
        .current_dir(repository.work_tree())
        .stdin(stdin)
        .cancel_token(cancel)
        .output()
        .await?;
    let mut answers = mapped.stdout.lines();
    contacts
        .iter()
        .map(|contact| {
            if !readable(contact) {
                return Ok((*contact).to_owned());
            }
            answers
                .next()
                .map(BString::from)
                .ok_or_else(|| GitError::unexpected("check-mailmap", "a contact for each given"))
        })
        .collect()
}
