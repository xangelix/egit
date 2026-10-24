//! Ignoring files git doesn't track: patterns added to the work tree's top `.gitignore`
//! (shared, committed with the project) or the repository's `info/exclude` (this clone alone),
//! each written so it names exactly what was asked for, and taken back again.
//!
//! `info/exclude` is the one file under the git directory written here: gitignore(5)
//! documents it for people to edit, and `rev-parse --git-path` finds it (in the common
//! directory, for a worktree).

use std::path::{Path, PathBuf};

use bstr::{BStr, BString, ByteSlice as _};

use crate::cancel::CancelToken;
use crate::error::GitError;
use crate::git::Git;
use crate::repo::Repository;

/// Where ignore patterns go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IgnoreIn {
    /// The work tree's top `.gitignore`, committed with the project.
    Shared,
    /// The repository's `info/exclude`: this clone alone.
    Private,
}

/// Why ignoring (or taking it back) failed.
#[derive(Debug, thiserror::Error)]
pub enum IgnoreError {
    /// A path holding a line break: no pattern can name it.
    #[error("a name holding a line break can't be ignored by a pattern")]
    LineBreak,
    /// The file no longer ends with what was added to it.
    #[error("{} changed since", .0.display())]
    Changed(PathBuf),
    /// The file couldn't be read or written.
    #[error("{}: {source}", path.display())]
    Io {
        /// The file.
        path: PathBuf,
        /// Why.
        #[source]
        source: std::io::Error,
    },
    /// Git failed, or was cancelled.
    #[error(transparent)]
    Git(#[from] GitError),
}

/// Patterns added to an ignore file, to take them back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ignored {
    /// The file.
    pub file: PathBuf,
    /// What was added at its end (nothing when every pattern was there already).
    pub added: BString,
    /// Whether the file was made for them.
    pub created: bool,
}

/// A path's pattern (`/`-separated, from the work tree's top), anchored there so it names
/// that path alone: `/src/build.log`, or `/build/` for a folder (folders only).
///
/// # Errors
///
/// [`IgnoreError::LineBreak`] for a path holding one.
pub fn path_pattern(path: &BStr, folder: bool) -> Result<BString, IgnoreError> {
    let mut pattern = BString::from("/");
    escape_into(&mut pattern, path.trim_end_with(|c| c == '/').as_bstr())?;
    if folder {
        pattern.push(b'/');
    }
    Ok(pattern)
}

/// The pattern of every file ending with `.extension`, in any folder: `*.log`.
///
/// # Errors
///
/// [`IgnoreError::LineBreak`] for an extension holding one.
pub fn extension_pattern(extension: &BStr) -> Result<BString, IgnoreError> {
    let mut pattern = BString::from("*.");
    escape_into(&mut pattern, extension)?;
    Ok(pattern)
}

/// `name` written into a pattern so it matches itself alone: the glob characters and the
/// escape escaped, and trailing spaces (which git would strip) too.
fn escape_into(pattern: &mut BString, name: &BStr) -> Result<(), IgnoreError> {
    let trailing = name
        .len()
        .saturating_sub(name.iter().rev().take_while(|byte| **byte == b' ').count());
    for (at, byte) in name.iter().enumerate() {
        match byte {
            b'\n' | b'\r' => return Err(IgnoreError::LineBreak),
            b'\\' | b'*' | b'?' | b'[' => pattern.push(b'\\'),
            b' ' if at >= trailing => pattern.push(b'\\'),
            _ => {}
        }
        pattern.push(*byte);
    }
    Ok(())
}

/// Adds `patterns` to the end of the ignore file `place` names, those it doesn't hold already,
/// one a line (as its lines end).
///
/// # Errors
///
/// When the file can't be found (git fails), read, or written.
pub async fn ignore(
    git: &Git,
    repository: &Repository,
    place: IgnoreIn,
    patterns: &[BString],
    cancel: &CancelToken,
) -> Result<Ignored, IgnoreError> {
    let file = match place {
        IgnoreIn::Shared => repository.work_tree().join(".gitignore"),
        IgnoreIn::Private => {
            let found = git
                .read("rev-parse")
                .args(["--path-format=absolute", "--git-path", "info/exclude"])
                .current_dir(repository.work_tree())
                .cancel_token(cancel)
                .output()
                .await?;
            found
                .stdout
                .trim_end_with(|c| c == '\n' || c == '\r')
                .to_path_lossy()
                .into_owned()
        }
    };
    add_patterns(git, &file, patterns).await
}

/// Adds `patterns` again, as they were before [`unignore`] took them back.
///
/// # Errors
///
/// When the file can't be read or written.
pub async fn ignore_again(git: &Git, ignored: &Ignored) -> Result<Ignored, IgnoreError> {
    let patterns: Vec<BString> = ignored
        .added
        .lines()
        .filter(|line| !line.is_empty())
        .map(BString::from)
        .collect();
    add_patterns(git, &ignored.file, &patterns).await
}

/// Takes back what [`ignore`] added: its end cut off (the file removed, when it was made for
/// it and holds nothing else).
///
/// # Errors
///
/// [`IgnoreError::Changed`] when the file no longer ends with it (nothing is changed), and
/// when it can't be read or written.
pub async fn unignore(git: &Git, ignored: &Ignored) -> Result<(), IgnoreError> {
    if ignored.added.is_empty() {
        return Ok(());
    }
    let file = &ignored.file;
    let io = |source| IgnoreError::Io {
        path: file.clone(),
        source,
    };
    let files = git.files();
    let content = match files.read(file).await {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(IgnoreError::Changed(file.clone()));
        }
        Err(error) => return Err(io(error)),
    };
    let Some(kept) = content.strip_suffix(ignored.added.as_slice()) else {
        return Err(IgnoreError::Changed(file.clone()));
    };
    if ignored.created && kept.is_empty() {
        return files.remove_file(file).await.map_err(io);
    }
    files.write(file, kept).await.map_err(io)
}

/// Appends those of `patterns` that `file` doesn't hold as a line already.
async fn add_patterns(
    git: &Git,
    file: &Path,
    patterns: &[BString],
) -> Result<Ignored, IgnoreError> {
    let io = |source| IgnoreError::Io {
        path: file.to_owned(),
        source,
    };
    let files = git.files();
    let (content, created) = match files.read(file).await {
        Ok(content) => (content, false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (Vec::new(), true),
        Err(error) => return Err(io(error)),
    };
    let ending: &[u8] = if content.contains_str("\r\n") {
        b"\r\n"
    } else {
        b"\n"
    };
    let mut added = BString::default();
    for pattern in patterns {
        let held = content
            .lines()
            .chain(added.lines())
            .any(|line| line == pattern.as_slice());
        if !held {
            added.extend_from_slice(pattern);
            added.extend_from_slice(ending);
        }
    }
    if added.is_empty() {
        return Ok(Ignored {
            file: file.to_owned(),
            added,
            created: false,
        });
    }
    if !content.is_empty() && !content.ends_with(b"\n") {
        added = [ending, added.as_slice()].concat().into();
    }
    if let Some(folder) = file.parent() {
        files.create_dir_all(folder).await.map_err(io)?;
    }
    files.append(file, &added).await.map_err(io)?;
    Ok(Ignored {
        file: file.to_owned(),
        added,
        created,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_name_exactly_what_was_asked_for() {
        let pattern = |path: &str| path_pattern(path.into(), false).unwrap();
        assert_eq!(pattern("src/build.log"), "/src/build.log");
        assert_eq!(pattern("a*b?c[d]e\\f"), "/a\\*b\\?c\\[d]e\\\\f");
        assert_eq!(pattern("#notes"), "/#notes");
        assert_eq!(pattern("!bang"), "/!bang");
        assert_eq!(pattern("two words  "), "/two words\\ \\ ");
        assert_eq!(path_pattern("build/".into(), true).unwrap(), "/build/");
        assert_eq!(extension_pattern("log".into()).unwrap(), "*.log");
        assert!(matches!(
            path_pattern("a\nb".into(), false),
            Err(IgnoreError::LineBreak)
        ));
    }
}
