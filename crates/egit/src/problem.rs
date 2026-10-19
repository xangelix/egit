//! Problems as the user reads them: what went wrong, and git's own words for it.

use std::fmt;

use egit_git::{DiscoverError, GitError};

/// Something that went wrong, for the user to read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Problem {
    /// One sentence saying what failed ("The repository couldn't be opened").
    pub(crate) title: String,
    /// What git (or the system) said about it.
    pub(crate) detail: String,
}

impl Problem {
    pub(crate) fn new(title: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            detail: detail.into(),
        }
    }

    /// A git failure: git's own message when it gave one, or the error.
    pub(crate) fn git(title: impl Into<String>, error: &GitError) -> Self {
        let detail = error.failure().map_or_else(
            || error.to_string(),
            |failure| {
                let message = failure.stderr.message();
                if message.is_empty() {
                    error.to_string()
                } else {
                    message
                }
            },
        );
        Self::new(title, detail)
    }

    /// A background job that ended without an answer (it panicked, or the runtime stopped).
    pub(crate) fn interrupted() -> Self {
        Self::new(
            "Something egit was doing stopped unexpectedly",
            "The work was interrupted; try again.",
        )
    }
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.title, self.detail)
    }
}

/// Why a folder couldn't be opened as a repository, and what can be done about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OpenProblem {
    /// What went wrong.
    pub(crate) problem: Problem,
    /// Where to add to `safe.directory` when git refused a repository another user owns (on
    /// the computer it is on).
    pub(crate) trust: Option<crate::settings::Location>,
}

impl OpenProblem {
    /// Why discovering a repository (on `host`, if another computer) failed.
    pub(crate) fn from_discovery(error: &DiscoverError, host: Option<&str>) -> Self {
        let title = "The repository couldn't be opened";
        let (detail, trust) = match error {
            DiscoverError::NotARepository { path } => (
                format!("{} isn't inside a git repository.", path.display()),
                None,
            ),
            DiscoverError::DubiousOwnership { path } => (
                format!(
                    "Another user owns {}, so git won't open it until you trust it \
                     (git's safe.directory setting).",
                    path.display()
                ),
                Some(path.clone()),
            ),
            DiscoverError::Bare { git_dir } => (
                format!(
                    "{} is a bare repository: it has no files to work on. Clone it to work \
                     with it.",
                    git_dir.display()
                ),
                None,
            ),
            DiscoverError::InsideGitDir { path } => (
                format!(
                    "{} is inside a repository's .git folder. Open the folder that \
                     contains .git instead.",
                    path.display()
                ),
                None,
            ),
            DiscoverError::Git(error) => return Self::from_problem(Problem::git(title, error)),
            other => (other.to_string(), None),
        };
        Self {
            problem: Problem::new(title, detail),
            trust: trust.map(|path| crate::settings::Location {
                host: host.map(str::to_owned),
                path,
            }),
        }
    }

    pub(crate) const fn from_problem(problem: Problem) -> Self {
        Self {
            problem,
            trust: None,
        }
    }
}

impl From<Problem> for OpenProblem {
    fn from(problem: Problem) -> Self {
        Self::from_problem(problem)
    }
}
