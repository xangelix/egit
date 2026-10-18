//! The git binary egit drives.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use crate::command::{Access, GitCommand};
use crate::env::Environment;
use crate::error::GitError;
use crate::transcript::Transcript;
use crate::version::GitVersion;

/// How to find and run git.
#[derive(Clone, Debug, Default)]
pub struct GitOptions {
    /// The executable. `None` finds `git` on `PATH`.
    pub program: Option<PathBuf>,
    /// Environment variables for every git process, set after (and so over) egit's own.
    pub env: Vec<(OsString, OsString)>,
    /// Environment variables no git process gets, whatever egit or [`Self::env`] would set.
    pub unset: Vec<OsString>,
}

/// The git binary egit drives: where it is, its version, and the environment it runs in; and,
/// given one ([`Self::with_transcript`]), the transcript its writing commands print into.
///
/// Cheap to clone: clones share one description.
#[derive(Clone)]
pub struct Git {
    inner: Arc<Inner>,
    transcript: Option<Transcript>,
}

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
            None => find_on_path(OsStr::new(GIT)).ok_or(GitError::NotFound)?,
        };
        let mut environment = Environment::plan(std::env::vars_os());
        for (key, value) in options.env {
            environment.push(key, value);
        }
        for key in options.unset {
            environment.unset(key);
        }
        let mut inner = Inner {
            program,
            // Replaced below, once git has said which it is.
            version: GitVersion::MINIMUM,
            environment,
        };

        let unchecked = Self {
            inner: Arc::new(inner.clone()),
            transcript: None,
        };
        let reported = unchecked.read("version").output()?;
        let version =
            GitVersion::parse(&reported.stdout).map_err(|_| GitError::UnknownVersion {
                program: inner.program.clone(),
            })?;
        if !version.is_supported() {
            return Err(GitError::Unsupported { found: version });
        }
        inner.version = version;
        Ok(Self {
            inner: Arc::new(inner),
            transcript: None,
        })
    }

    /// A command that only reads (see [`Access::Read`]).
    pub fn read(&self, subcommand: &'static str) -> GitCommand {
        GitCommand::new(self.clone(), Access::Read, subcommand)
    }

    /// A command that may write (see [`Access::Write`]).
    pub fn write(&self, subcommand: &'static str) -> GitCommand {
        GitCommand::new(self.clone(), Access::Write, subcommand)
    }

    /// The same git, its every command that may write printing into `transcript` as it runs:
    /// its command line, what it prints (hooks' output among it), and how it ended.
    #[must_use]
    pub fn with_transcript(&self, transcript: &Transcript) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            transcript: Some(transcript.clone()),
        }
    }

    /// The transcript its writing commands print into, if any.
    #[must_use]
    pub const fn transcript(&self) -> Option<&Transcript> {
        self.transcript.as_ref()
    }

    /// The executable.
    #[must_use]
    pub fn program(&self) -> &Path {
        &self.inner.program
    }

    /// Its version.
    #[must_use]
    pub fn version(&self) -> GitVersion {
        self.inner.version
    }

    pub(crate) fn environment(&self) -> &Environment {
        &self.inner.environment
    }
}

impl fmt::Debug for Git {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Git")
            .field("program", &self.inner.program)
            .field("version", &self.inner.version)
            .finish_non_exhaustive()
    }
}

/// The executable's name on `PATH`.
const GIT: &str = if cfg!(windows) { "git.exe" } else { "git" };

/// A directory no repository contains, where a command that must not see one runs (the
/// user's global configuration, a clone): `/`, or on Windows the system drive's root.
pub(crate) fn outside_repositories() -> &'static Path {
    static OUTSIDE: OnceLock<PathBuf> = OnceLock::new();
    OUTSIDE.get_or_init(|| {
        if cfg!(windows) {
            let drive = std::env::var_os("SystemDrive").unwrap_or_else(|| "C:".into());
            let mut root = PathBuf::from(drive);
            root.push("\\");
            root
        } else {
            PathBuf::from("/")
        }
    })
}

/// The first executable called `name` in a directory on `PATH`.
fn find_on_path(name: &OsStr) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable(candidate))
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    path.metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

/// On Windows a file is run by its name (`.exe`), not a permission.
#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}
