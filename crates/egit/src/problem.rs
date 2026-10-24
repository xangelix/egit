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
    /// What would let it open, when something would.
    pub(crate) remedy: Option<Remedy>,
}

/// What lets a folder that didn't open open, on the computer it is on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Remedy {
    /// Add it to `safe.directory`: git refused a repository another user owns.
    Trust(crate::settings::Location),
    /// Make it a repository (`git init`): it isn't inside one.
    Create(crate::settings::Location),
}

impl OpenProblem {
    /// Why discovering a repository (on `host`, if another computer) failed.
    pub(crate) fn from_discovery(error: &DiscoverError, host: Option<&str>) -> Self {
        let title = "The repository couldn't be opened";
        let place = |path: &std::path::Path| crate::settings::Location {
            host: host.map(str::to_owned),
            path: path.to_path_buf(),
        };
        let (detail, remedy) = match error {
            DiscoverError::NotARepository { path } => (
                format!(
                    "{} isn't inside a git repository. Create one there to start tracking \
                     its files.",
                    path.display()
                ),
                Some(Remedy::Create(place(path))),
            ),
            DiscoverError::DubiousOwnership { path } => (
                format!(
                    "Another user owns {}, so git won't open it until you trust it \
                     (git's safe.directory setting).",
                    path.display()
                ),
                Some(Remedy::Trust(place(path))),
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
            remedy,
        }
    }

    pub(crate) const fn from_problem(problem: Problem) -> Self {
        Self {
            problem,
            remedy: None,
        }
    }
}

impl From<Problem> for OpenProblem {
    fn from(problem: Problem) -> Self {
        Self::from_problem(problem)
    }
}
