//! A sandboxed git for tests (the `testing` feature).
//!
//! A sandbox is a temporary directory with a home of its own and a known global configuration.
//! The system's configuration is ignored, and so is the user's, so tests behave the same on
//! every machine. Repository discovery stops at the sandbox (`GIT_CEILING_DIRECTORIES`).
//!
//! Set `EGIT_TEST_GIT` to a git executable to run the tests against it (an older release, say)
//! instead of the one on `PATH`.

use std::error::Error;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use crate::{Git, GitOptions};

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

/// What can go wrong setting up a sandbox.
pub type SandboxError = Box<dyn Error + Send + Sync>;

/// A temporary home with its own git configuration.
#[derive(Debug)]
pub struct Sandbox {
    root: TempDir,
    home: PathBuf,
    git: Git,
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

        let env = [
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
        let git = Git::new(GitOptions {
            program: std::env::var_os("EGIT_TEST_GIT").map(PathBuf::from),
            env,
        })?;
        Ok(Self { root, home, git })
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
        self.git
            .write("add")
            .arg("--")
            .arg(file)
            .current_dir(repo)
            .output()?;
        self.git
            .write("commit")
            .args(["--quiet", "--message", message])
            .current_dir(repo)
            .output()?;
        Ok(())
    }
}
