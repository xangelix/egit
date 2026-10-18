//! Ignoring files git doesn't track, as a task: a pattern naming a file, every file of its
//! extension, or a folder, added to the top `.gitignore` (shared) or `info/exclude` (here
//! only), then checked by git (a later `!` pattern may bring it back), and taken back by Undo.

use bstr::{BStr, BString, ByteSlice as _};
use egit_git::{
    CancelToken, Git, IgnoreError, IgnoreIn, Repository, extension_pattern, ignore, ignored_paths,
    path_pattern,
};

use crate::problem::Problem;
use crate::tasks::Outcome;

/// What a pattern names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IgnoreWhat {
    /// The file alone.
    File,
    /// Every file with its extension, in any folder.
    Extension,
    /// A folder, and all it holds.
    Folder,
}

/// Something git doesn't track to ignore, and where the pattern goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Ignore {
    /// The file (or, for [`IgnoreWhat::Folder`], the folder), from the work tree's top.
    pub(crate) path: BString,
    pub(crate) what: IgnoreWhat,
    pub(crate) place: IgnoreIn,
}

impl Ignore {
    /// What is ignored, in words: "src/build.log", "every .log file", "the folder src/".
    pub(crate) fn words(&self) -> String {
        let path = self.path.to_str_lossy();
        match self.what {
            IgnoreWhat::File => path.into_owned(),
            IgnoreWhat::Extension => format!(
                "every .{} file",
                extension(self.path.as_bstr())
                    .unwrap_or_default()
                    .to_str_lossy()
            ),
            IgnoreWhat::Folder => format!("the folder {path}/"),
        }
    }

    /// The file it goes into, as people know it.
    pub(crate) const fn file_name(&self) -> &'static str {
        match self.place {
            IgnoreIn::Shared => ".gitignore",
            IgnoreIn::Private => "info/exclude",
        }
    }

    /// Its pattern.
    fn pattern(&self) -> Result<BString, IgnoreError> {
        match self.what {
            IgnoreWhat::File => path_pattern(self.path.as_bstr(), false),
            IgnoreWhat::Extension => {
                extension_pattern(extension(self.path.as_bstr()).unwrap_or_default())
            }
            IgnoreWhat::Folder => path_pattern(self.path.as_bstr(), true),
        }
    }

    /// The path git is asked about once it is ignored (a folder's ending with `/`, so a
    /// folder-only pattern matches it).
    fn checked(&self) -> BString {
        let mut path = self.path.clone();
        if self.what == IgnoreWhat::Folder {
            path.push(b'/');
        }
        path
    }
}

/// The extension a file's name ends with, if it has one (`.env` hasn't: its dot starts it).
pub(crate) fn extension(path: &BStr) -> Option<&BStr> {
    let name = path.rsplit_str("/").next().unwrap_or_default();
    let dot = name.rfind_byte(b'.').filter(|dot| *dot > 0)?;
    let extension = name.get(dot.saturating_add(1)..)?;
    (!extension.is_empty()).then(|| extension.as_bstr())
}

/// The folder holding a file, without its final `/` (none at the top).
pub(crate) fn folder_of(path: &BStr) -> Option<&BStr> {
    let slash = path.rfind_byte(b'/')?;
    path.get(..slash).map(bstr::ByteSlice::as_bstr)
}

/// Adds the pattern, then asks git whether the path is ignored now.
pub(crate) fn run(
    git: &Git,
    repository: &Repository,
    asked: &Ignore,
    cancel: &CancelToken,
) -> Result<Outcome, Problem> {
    let title = format!("{} couldn't be ignored", asked.words());
    let problem = |error: IgnoreError| match error {
        IgnoreError::Git(error) => Problem::git(title.clone(), &error),
        error => Problem::new(title.clone(), format!("{error}.")),
    };
    let pattern = asked.pattern().map_err(problem)?;
    let ignored = ignore(git, repository, asked.place, &[pattern], cancel).map_err(problem)?;
    let checked = asked.checked();
    // A check that fails says nothing either way: the pattern is written.
    let listed = ignored_paths(git, repository, &[checked.as_bstr()], cancel)
        .is_ok_and(|found| found.is_empty());
    Ok(Outcome::Ignored { ignored, listed })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_and_folders_are_read_from_paths() {
        let extension = |path: &str| extension(path.into()).map(ToString::to_string);
        assert_eq!(extension("src/build.log").as_deref(), Some("log"));
        assert_eq!(extension("archive.tar.gz").as_deref(), Some("gz"));
        assert_eq!(extension(".env"), None);
        assert_eq!(extension("src.d/Makefile"), None);
        assert_eq!(extension("trailing."), None);
        assert_eq!(
            folder_of("src/core/a.c".into()).map(ToString::to_string),
            Some("src/core".to_owned())
        );
        assert_eq!(folder_of("top.txt".into()), None);
    }
}
