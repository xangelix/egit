//! The git binary egit drives.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use crate::command::{Access, GitCommand};
use crate::env::Environment;
use crate::error::GitError;
use crate::files::Files;
use crate::host::Host;
use crate::transcript::Transcript;
use crate::version::GitVersion;

/// How to find and run git.
#[derive(Clone, Debug, Default)]
pub struct GitOptions {
    /// The executable. `None` finds `git` on `PATH` (on another computer, runs `git` there).
    pub program: Option<PathBuf>,
    /// Environment variables for every git process, set after (and so over) egit's own. On
    /// another computer, git gets them but this computer's helpers (the askpass, the ssh
    /// wrapper), which the ssh reaching it gets.
    pub env: Vec<(OsString, OsString)>,
    /// Environment variables no git process gets, whatever egit or [`Self::env`] would set.
    pub unset: Vec<OsString>,
    /// Where git runs: here, or on another computer over SSH.
    pub host: Host,
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
    /// The environment of what runs here: git, or the ssh reaching it.
    environment: Environment,
    host: Host,
    /// Git's environment on another computer.
    remote: Option<Environment>,
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
        let (program, remote) = match &options.host {
            Host::Local => match options.program {
                Some(program) => (program, None),
                None => (
                    find_on_path(OsStr::new(GIT)).ok_or(GitError::NotFound)?,
                    None,
                ),
            },
            Host::Ssh(_) => (
                options.program.unwrap_or_else(|| PathBuf::from("git")),
                Some(Environment::remote(&options.env, &options.unset)),
            ),
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
            host: options.host,
            remote,
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

    /// Where it runs.
    #[must_use]
    pub fn host(&self) -> &Host {
        &self.inner.host
    }

    /// The files of the computer it runs on, as egit reads and writes them.
    #[must_use]
    pub const fn files(&self) -> Files<'_> {
        Files::of(self)
    }

    pub(crate) fn environment(&self) -> &Environment {
        &self.inner.environment
    }

    /// The ssh command running `program` with `args` on the other computer this git runs on
    /// (in `dir`, with git's environment there and `env`, but this computer's helpers, which
    /// the ssh gets); `None` when it runs here. Its standard output starts with what a login
    /// script there printed: read it through [`GitStdout::of`](crate::GitStdout::of); its
    /// standard error starts with the script's process id: through
    /// [`PidReader`](crate::PidReader).
    #[must_use]
    pub fn command_there(
        &self,
        program: &OsStr,
        args: &[&OsStr],
        dir: Option<&Path>,
        env: &[(OsString, OsString)],
    ) -> Option<std::process::Command> {
        let ssh = self.inner.host.ssh()?;
        let environment = self.inner.remote.as_ref();
        let (here, there): (Vec<_>, Vec<_>) = env
            .iter()
            .map(|(key, value)| (key.as_os_str(), value.as_os_str()))
            .partition(|(key, _)| crate::env::is_local_only(key));
        let mut set: Vec<(&OsStr, &OsStr)> = environment
            .map(|environment| environment.settings().collect())
            .unwrap_or_default();
        set.extend(there);
        let script = crate::host::script(&crate::host::Remote {
            program,
            args: args.to_vec(),
            dir,
            unset: environment
                .map(|environment| environment.removals().collect())
                .unwrap_or_default(),
            set,
        });
        let mut command = ssh.command(&script);
        self.inner.environment.apply(&mut command);
        command.envs(here);
        Some(command)
    }

    /// Git's environment on another computer (none here).
    pub(crate) fn remote_environment(&self) -> Option<&Environment> {
        self.inner.remote.as_ref()
    }
}

impl fmt::Debug for Git {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Git")
            .field("program", &self.inner.program)
            .field("version", &self.inner.version)
            .field("host", &self.inner.host)
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
