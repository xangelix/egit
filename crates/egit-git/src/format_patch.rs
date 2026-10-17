//! A commit as a patch to mail or apply elsewhere (`git format-patch`): its message and
//! changes, as `git am` takes them.

use crate::cancel::CancelToken;
use crate::error::GitError;
use crate::git::Git;
use crate::object_id::ObjectId;
use crate::repo::Repository;

/// The longest name [`patch_file_name`] gives, as git's own.
const MAX_NAME: usize = 64;

/// `commit` as a patch (`git format-patch -1 --stdout`, with git's standard `a/` and `b/`
/// prefixes whatever `diff.noprefix` says, so `git am` takes it as it is).
///
/// # Errors
///
/// When git fails or is cancelled.
pub fn format_patch(
    git: &Git,
    repository: &Repository,
    commit: ObjectId,
    cancel: &CancelToken,
) -> Result<Vec<u8>, GitError> {
    git.read("format-patch")
        .args(["-1", "--stdout", "--default-prefix"])
        .arg(commit.to_string())
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output()
        .map(|done| done.stdout)
}

/// The file name git gives a commit's patch from its subject: `0001-` and the subject's
/// letters, digits, dots, and underscores, anything else becoming one `-`, cut to 64 bytes
/// with `.patch`.
#[must_use]
pub fn patch_file_name(subject: &str) -> String {
    const PREFIX: &str = "0001-";
    const SUFFIX: &str = ".patch";
    let room = MAX_NAME
        .saturating_sub(PREFIX.len())
        .saturating_sub(SUFFIX.len());
    let mut body = String::new();
    let mut dash = false;
    for c in subject.chars() {
        if c.is_ascii_alphanumeric() || c == '.' || c == '_' {
            if dash && !body.is_empty() {
                body.push('-');
            }
            dash = false;
            body.push(c);
        } else {
            dash = true;
        }
        if body.len() >= room {
            break;
        }
    }
    body.truncate(room);
    let kept = body.trim_end_matches(['.', '-']).len();
    body.truncate(kept);
    if body.is_empty() {
        body.push_str("patch");
    }
    format!("{PREFIX}{body}{SUFFIX}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_patch_is_named_as_git_names_it() {
        assert_eq!(
            patch_file_name("Fix a memory leak (v1.2)"),
            "0001-Fix-a-memory-leak-v1.2.patch"
        );
        assert_eq!(patch_file_name("  ...  "), "0001-patch.patch");
        assert_eq!(patch_file_name("Grüße, world!"), "0001-Gr-e-world.patch");
        // Cut (mid-word, as git does) to 64 bytes.
        let long = patch_file_name(&"word ".repeat(40));
        assert_eq!(long.len(), 64, "{long}");
        assert!(long.starts_with("0001-word-word-"));
        assert_eq!(&long[long.len() - 6..], ".patch");
    }
}
