//! The git binary egit drives.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::command::{Access, GitCommand};
use crate::env::Environment;
use crate::error::GitError;
use crate::version::GitVersion;

/// How to find and run git.
#[derive(Clone, Debug, Default)]
pub struct GitOptions {
    /// The executable. `None` finds `git` on `PATH`.
    pub program: Option<PathBuf>,
    /// Environment variables for every git process, set after (and so over) egit's own.
    pub env: Vec<(OsString, OsString)>,
}

/// The git binary egit drives: where it is, its version, and the environment it runs in.
///
/// Cheap to clone: clones share one description.
#[derive(Clone)]
pub struct Git(Arc<Inner>);

#[derive(Clone)]
struct Inner {
    program: PathBuf,
    version: GitVersion,
    environment: Environment,
}

impl Git {
    /// Finds git, checks that egit supports its version, and prepares its environment from the
    /// one egit was started with.
    ///
    /// # Errors
    ///
    /// [`GitError::NotFound`] without a `git` on `PATH`, [`GitError::Launch`] when it can't be
    /// started, [`GitError::UnknownVersion`] when it doesn't report a version, and
    /// [`GitError::Unsupported`] when it is older than [`GitVersion::MINIMUM`].
    pub fn new(options: GitOptions) -> Result<Self, GitError> {
        let program = match options.program {
            Some(program) => program,
            None => find_on_path(OsStr::new("git")).ok_or(GitError::NotFound)?,
        };
        let mut environment = Environment::plan(std::env::vars_os());
        for (key, value) in options.env {
            environment.push(key, value);
        }
        let mut inner = Inner {
            program,
            // Replaced below, once git has said which it is.
            version: GitVersion::MINIMUM,
            environment,
        };

        let unchecked = Self(Arc::new(inner.clone()));
        let reported = unchecked.read("version").output()?;
        let version =
            GitVersion::parse(&reported.stdout).map_err(|_| GitError::UnknownVersion {
                program: inner.program.clone(),
            })?;
        if !version.is_supported() {
            return Err(GitError::Unsupported { found: version });
        }
        inner.version = version;
        Ok(Self(Arc::new(inner)))
    }

    /// A command that only reads (see [`Access::Read`]).
    pub fn read(&self, subcommand: &'static str) -> GitCommand {
        GitCommand::new(self.clone(), Access::Read, subcommand)
    }

    /// A command that may write (see [`Access::Write`]).
    pub fn write(&self, subcommand: &'static str) -> GitCommand {
        GitCommand::new(self.clone(), Access::Write, subcommand)
    }

    /// The executable.
    #[must_use]
    pub fn program(&self) -> &Path {
        &self.0.program
    }

    /// Its version.
    #[must_use]
    pub fn version(&self) -> GitVersion {
        self.0.version
    }

    pub(crate) fn environment(&self) -> &Environment {
        &self.0.environment
    }
}

impl fmt::Debug for Git {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Git")
            .field("program", &self.0.program)
            .field("version", &self.0.version)
            .finish_non_exhaustive()
    }
}

/// The first executable called `name` in a directory on `PATH`.
fn find_on_path(name: &OsStr) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    path.metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}
