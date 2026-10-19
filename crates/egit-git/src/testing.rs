//! A sandboxed git for tests (the `testing` feature).
//!
//! A sandbox is a temporary directory with a home of its own and a known global configuration.
//! The system's configuration is ignored, and so is the user's, and so are identities and dates
//! the environment would give commits (`GIT_AUTHOR_NAME`, say), so tests behave the same on
//! every machine. Repository discovery stops at the sandbox (`GIT_CEILING_DIRECTORIES`).
//!
//! Set `EGIT_TEST_GIT` to a git executable to run the tests against it (an older release, say)
//! instead of the one on `PATH`.

use std::error::Error;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use crate::{Completed, Git, GitOptions};

/// The global configuration every sandbox starts with.
const GLOBAL_CONFIG: &str = "\
[user]
\tname = egit tests
\temail = tests@egit.invalid
[init]
\tdefaultBranch = main
[commit]
\tgpgSign = false
[tag]
\tgpgSign = false
[core]
\tautocrlf = false
";

/// Variables that would give commits another identity or date than the sandbox's.
const IDENTITY_VARIABLES: [&str; 7] = [
    "GIT_AUTHOR_NAME",
    "GIT_AUTHOR_EMAIL",
    "GIT_AUTHOR_DATE",
    "GIT_COMMITTER_NAME",
    "GIT_COMMITTER_EMAIL",
    "GIT_COMMITTER_DATE",
    "EMAIL",
];

/// Variables naming the ssh command git runs: a test names its own, or none.
const SSH_VARIABLES: [&str; 3] = ["GIT_SSH_COMMAND", "GIT_SSH", "GIT_SSH_VARIANT"];

/// What can go wrong setting up a sandbox.
pub type SandboxError = Box<dyn Error + Send + Sync>;

/// A temporary home with its own git configuration.
#[derive(Debug)]
pub struct Sandbox {
    root: TempDir,
    home: PathBuf,
    env: Vec<(OsString, OsString)>,
    git: Git,
}

/// Sets `path`'s permission bits to `mode` where files have them (Unix); elsewhere does
/// nothing: Git for Windows runs a script by its `#!` line, and keeps no permission bits.
///
/// # Errors
///
/// When the permissions can't be set.
#[cfg(unix)]
pub fn set_mode(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

/// Sets `path`'s permission bits to `mode` where files have them (Unix); elsewhere does
/// nothing: Git for Windows runs a script by its `#!` line, and keeps no permission bits.
///
/// # Errors
///
/// None here.
#[cfg(not(unix))]
pub const fn set_mode(_path: &Path, _mode: u32) -> std::io::Result<()> {
    Ok(())
}

impl Sandbox {
    /// A new sandbox around the git named by `EGIT_TEST_GIT`, or the one on `PATH`.
    ///
    /// # Errors
    ///
    /// When the directory can't be created or git can't be started.
    pub fn new() -> Result<Self, SandboxError> {
        let root = tempfile::Builder::new().prefix("egit-sandbox-").tempdir()?;
        let home = root.path().join("home");
        fs::create_dir(&home)?;
        let global_config = home.join(".gitconfig");
        fs::write(&global_config, GLOBAL_CONFIG)?;

        let env: Vec<(OsString, OsString)> = [
            ("HOME", home.clone().into_os_string()),
            ("XDG_CONFIG_HOME", home.join(".config").into_os_string()),
            ("GIT_CONFIG_GLOBAL", global_config.into_os_string()),
            ("GIT_CONFIG_NOSYSTEM", OsString::from("1")),
            (
                "GIT_CEILING_DIRECTORIES",
                root.path().as_os_str().to_owned(),
            ),
            // An editor's terminal may export its own askpass helper; nothing in a test may
            // prompt anyone.
            ("GIT_ASKPASS", OsString::new()),
            ("SSH_ASKPASS", OsString::new()),
        ]
        .into_iter()
        .map(|(key, value)| (OsString::from(key), value))
        .collect();
        let git = Self::git_from(env.clone())?;
        Ok(Self {
            root,
            home,
            env,
            git,
        })
    }

    fn git_from(env: Vec<(OsString, OsString)>) -> Result<Git, SandboxError> {
        Ok(Git::new(GitOptions {
            program: std::env::var_os("EGIT_TEST_GIT").map(PathBuf::from),
            // The ssh command's, unless a test names its own.
            unset: IDENTITY_VARIABLES
                .iter()
                .chain(
                    SSH_VARIABLES
                        .iter()
                        .filter(|name| !env.iter().any(|(key, _)| key == **name)),
                )
                .map(OsString::from)
                .collect(),
            env,
        })?)
    }

    /// The sandbox's git with more environment variables, such as git's own test switches
    /// (`GIT_TEST_ASSUME_DIFFERENT_OWNER`) or an askpass server's.
    ///
    /// # Errors
    ///
    /// When git can't be started.
    pub fn git_with<K, V>(
        &self,
        extra: impl IntoIterator<Item = (K, V)>,
    ) -> Result<Git, SandboxError>
    where
        K: Into<OsString>,
        V: Into<OsString>,
    {
        let mut env = self.env.clone();
        env.extend(
            extra
                .into_iter()
                .map(|(key, value)| (key.into(), value.into())),
        );
        Self::git_from(env)
    }

    /// Git, configured for the sandbox.
    #[must_use]
    pub const fn git(&self) -> &Git {
        &self.git
    }

    /// The sandbox's home directory (it holds the global `.gitconfig`).
    #[must_use]
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// A path inside the sandbox.
    #[must_use]
    pub fn path(&self, name: impl AsRef<Path>) -> PathBuf {
        self.root.path().join(name)
    }

    /// A new, empty repository called `name`, on branch `main`.
    ///
    /// # Errors
    ///
    /// When the directory can't be created or `git init` fails.
    pub fn init(&self, name: &str) -> Result<PathBuf, SandboxError> {
        let repo = self.path(name);
        fs::create_dir_all(&repo)?;
        self.git
            .write("init")
            .arg("--quiet")
            .current_dir(&repo)
            .output()?;
        Ok(repo)
    }

    /// Runs `git <subcommand> <args>` in `dir`, which must succeed.
    ///
    /// # Errors
    ///
    /// When git fails.
    pub fn run(
        &self,
        dir: &Path,
        subcommand: &'static str,
        args: &[&str],
    ) -> Result<Completed, SandboxError> {
        Ok(self
            .git
            .write(subcommand)
            .args(args)
            .current_dir(dir)
            .output()?)
    }

    /// Writes `contents` to `file` in `repo` and commits it with `message`.
    ///
    /// # Errors
    ///
    /// When the file can't be written or git fails.
    pub fn commit_file(
        &self,
        repo: &Path,
        file: &str,
        contents: &str,
        message: &str,
    ) -> Result<(), SandboxError> {
        let path = repo.join(file);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, contents)?;
        self.run(repo, "add", &["--", file])?;
        self.run(repo, "commit", &["--quiet", "--message", message])?;
        Ok(())
    }
}
