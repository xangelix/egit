//! Setting Claude up where a repository is (this computer, or another reached over SSH): Node.js
//! and npm found, and the SDK installed.
//!
//! Node and npm are found on `PATH`; the bridge, with Anthropic's Claude Agent SDK, is
//! installed into egit's cache by `npm ci`, from the lockfile egit
//! carries (exact versions with their integrity hashes; install scripts off; the SDK's peers
//! left out, as its main entry carries them). Two packages arrive, both Anthropic's: the SDK and
//! the Claude Code binary for this platform.
//!
//! An installation is a folder named for the SDK's version and a hash of the files egit
//! carries, made beside its final place and renamed into it once `npm ci` succeeded (a marker
//! file says so), so a half-made one is never used; others are removed then.
//!
//! On another computer the same is done there, through its git ([`Git::run_there`],
//! [`Git::files`]): Node and npm found on its `PATH` (`find_node_there`), the installation under
//! its cache folder (`${XDG_CACHE_HOME:-$HOME/.cache}/egit`, `Install::there`).

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use egit_git::{CancelToken, Git, Running};

use crate::AgentError;

/// The Claude Agent SDK's version, as the lockfile pins it.
pub const SDK_VERSION: &str = "0.3.296";

/// The oldest Node.js the SDK runs on.
pub const NODE_MINIMUM: u32 = 18;

/// About how much an installation takes on disk, in megabytes (mostly Claude Code's binary).
pub const INSTALLED_MEGABYTES: u32 = 250;

/// The bridge, and the package it is installed as.
const BRIDGE: &str = include_str!("../bridge/claude/bridge.mjs");
const PACKAGE: &str = include_str!("../bridge/claude/package.json");
const LOCKFILE: &str = include_str!("../bridge/claude/package-lock.json");

/// Written last into a finished installation.
const READY: &str = ".egit-ready";

/// Node.js and npm, as found on `PATH` (there, by their names: its `PATH` finds them).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    /// `node`.
    pub node: PathBuf,
    /// `npm`.
    pub npm: PathBuf,
    /// Node's major version.
    pub major: u32,
    /// Its version as it says it (`v22.1.0`).
    pub version: String,
}

/// Finds Node.js (18 or newer) and npm on `PATH`.
///
/// # Errors
///
/// [`AgentError::NodeMissing`] without them, [`AgentError::NodeTooOld`] for an older Node.
pub fn find_node() -> Result<Node, AgentError> {
    let (node_name, npm_name) = if cfg!(windows) {
        ("node.exe", "npm.cmd")
    } else {
        ("node", "npm")
    };
    let node = egit_git::find_program(OsStr::new(node_name)).ok_or(AgentError::NodeMissing)?;
    let npm = egit_git::find_program(OsStr::new(npm_name)).ok_or(AgentError::NodeMissing)?;
    let out = Command::new(&node)
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .map_err(|_| AgentError::NodeMissing)?;
    let version = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    let major = version
        .trim_start_matches('v')
        .split('.')
        .next()
        .and_then(|major| major.parse().ok())
        .unwrap_or(0);
    if major < NODE_MINIMUM {
        return Err(AgentError::NodeTooOld(version));
    }
    Ok(Node {
        node,
        npm,
        major,
        version,
    })
}

/// Finds Node.js (18 or newer) and npm on the `PATH` of the other computer `git` runs on.
/// Blocks: run it in a job.
///
/// # Errors
///
/// [`AgentError::NodeMissing`] without them there, [`AgentError::NodeTooOld`] for an older
/// Node, and when the other computer can't be reached.
pub async fn find_node_there(git: &Git) -> Result<Node, AgentError> {
    let said = sh_there(
        git,
        "command -v node >/dev/null && command -v npm >/dev/null || { echo missing; exit 0; }; \
         node --version",
        &[],
    )
    .await?;
    let version = String::from_utf8_lossy(&said).trim().to_owned();
    if version == "missing" {
        return Err(AgentError::NodeMissing);
    }
    let major = version
        .trim_start_matches('v')
        .split('.')
        .next()
        .and_then(|major| major.parse().ok())
        .unwrap_or(0);
    if major < NODE_MINIMUM {
        return Err(AgentError::NodeTooOld(version));
    }
    Ok(Node {
        node: PathBuf::from("node"),
        npm: PathBuf::from("npm"),
        major,
        version,
    })
}

/// Where the bridge and the SDK are (or go), for this version of what egit carries: on this
/// computer, or on another (`there`: its git).
#[derive(Clone, Debug)]
pub struct Install {
    dir: PathBuf,
    there: Option<Git>,
}

impl PartialEq for Install {
    fn eq(&self, other: &Self) -> bool {
        self.dir == other.dir
            && self.there.as_ref().map(Git::host) == other.there.as_ref().map(Git::host)
    }
}

impl Eq for Install {}

impl Install {
    /// The installation under `cache` (egit's cache folder).
    #[must_use]
    pub fn at(cache: &Path) -> Self {
        Self {
            dir: cache.join("agents").join("claude").join(folder_name()),
            there: None,
        }
    }

    /// The installation on the other computer `git` runs on, under its cache folder there.
    /// Blocks: run it in a job.
    ///
    /// # Errors
    ///
    /// When the other computer can't be reached.
    pub async fn there(git: &Git) -> Result<Self, AgentError> {
        let cache = sh_there(
            git,
            "printf %s \"${XDG_CACHE_HOME:-$HOME/.cache}/egit\"",
            &[],
        )
        .await?;
        let cache = PathBuf::from(String::from_utf8_lossy(&cache).into_owned());
        Ok(Self {
            there: Some(git.clone()),
            ..Self::at(&cache)
        })
    }

    /// Its folder.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The other computer's git, when it is there.
    #[must_use]
    pub const fn elsewhere(&self) -> Option<&Git> {
        self.there.as_ref()
    }

    /// Whether it is installed and whole (there, asked of the other computer: in a job).
    #[must_use]
    pub async fn is_ready(&self) -> bool {
        let Some(git) = &self.there else {
            return self.dir.join(READY).is_file()
                && self.bridge().is_file()
                && self.claude().await.is_some();
        };
        let (ready, bridge) = (self.dir.join(READY), self.bridge());
        git.files()
            .exist(&[&ready, &bridge])
            .await
            .is_ok_and(|there| there.iter().all(|there| *there))
            && self.claude().await.is_some()
    }

    /// The bridge script.
    #[must_use]
    pub fn bridge(&self) -> PathBuf {
        self.dir.join("bridge.mjs")
    }

    /// Claude Code's binary for its platform, as the SDK installed it (there, whichever
    /// platform's npm chose: asked of the other computer, in a job).
    #[must_use]
    pub async fn claude(&self) -> Option<PathBuf> {
        let packages = self.dir.join("node_modules").join("@anthropic-ai");
        let binary = if cfg!(windows) && self.there.is_none() {
            "claude.exe"
        } else {
            "claude"
        };
        let candidate = |platform: &&str| {
            packages
                .join(format!("claude-agent-sdk-{platform}"))
                .join(binary)
        };
        let Some(git) = &self.there else {
            return platforms()
                .iter()
                .map(candidate)
                .find(|path| path.is_file());
        };
        let candidates: Vec<PathBuf> = OTHER_PLATFORMS.iter().map(candidate).collect();
        let there = git
            .files()
            .exist(&candidates.iter().map(PathBuf::as_path).collect::<Vec<_>>())
            .await
            .ok()?;
        candidates
            .into_iter()
            .zip(there)
            .find_map(|(path, there)| there.then_some(path))
    }

    /// Installs it with `node`'s npm (`npm ci`), unless it is ready already; removes other
    /// installations after. Blocks: run it in a job.
    ///
    /// # Errors
    ///
    /// When the files can't be written, npm fails (its words said), Claude Code's binary isn't
    /// there after, or `cancel` stopped it.
    pub async fn install(&self, node: &Node, cancel: &CancelToken) -> Result<(), AgentError> {
        if self.is_ready().await {
            return Ok(());
        }
        if let Some(git) = &self.there {
            return self.install_there(git, cancel).await;
        }
        let parent = self
            .dir
            .parent()
            .ok_or_else(|| AgentError::Install("no folder to install into".to_owned()))?;
        std::fs::create_dir_all(parent)?;
        let making = parent.join(format!(".installing-{}", std::process::id()));
        if making.exists() {
            std::fs::remove_dir_all(&making)?;
        }
        std::fs::create_dir_all(&making)?;
        let made = write_and_install(&making, node, cancel).await;
        if let Err(error) = made {
            remove_quietly(&making);
            return Err(error);
        }
        if self.dir.exists() {
            // Made meanwhile by another egit, or half-made before: this one replaces it.
            std::fs::remove_dir_all(&self.dir)?;
        }
        std::fs::rename(&making, &self.dir)?;
        for entry in std::fs::read_dir(parent)?.flatten() {
            let path = entry.path();
            let older = path != self.dir
                && entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| !name.starts_with('.'));
            if older {
                remove_quietly(&path);
            }
        }
        Ok(())
    }
}

impl Install {
    /// [`Self::install`] on the other computer `git` runs on: the package written into a
    /// folder beside its place there, `npm ci` run there (cancelled, its session ends), then
    /// renamed into place, older ones removed.
    async fn install_there(&self, git: &Git, cancel: &CancelToken) -> Result<(), AgentError> {
        let parent = self
            .dir
            .parent()
            .ok_or_else(|| AgentError::Install("no folder to install into".to_owned()))?;
        let making = parent.join(format!(".installing-{}", std::process::id()));
        sh_there(
            git,
            "rm -rf -- \"$1\" && mkdir -p -- \"$1\"",
            &[making.as_os_str()],
        )
        .await?;
        let files = git.files();
        for (name, text) in [
            ("bridge.mjs", BRIDGE),
            ("package.json", PACKAGE),
            ("package-lock.json", LOCKFILE),
        ] {
            files.write(&making.join(name), text.as_bytes()).await?;
        }
        let args = NPM_CI.map(OsStr::new);
        let npm = git.run_there(OsStr::new("npm"), &args, Some(&making), (&[], &[]), false)?;
        let ran = run_npm(npm, cancel).await;
        let made = Self {
            dir: making.clone(),
            there: Some(git.clone()),
        };
        let checked = match ran {
            Err(error) => Err(error),
            Ok(()) if made.claude().await.is_none() => Err(AgentError::Install(
                "npm installed no Claude Code binary for that computer".to_owned(),
            )),
            Ok(()) => files
                .write(&making.join(READY), SDK_VERSION.as_bytes())
                .await
                .map_err(AgentError::from),
        };
        if let Err(error) = checked {
            let _removed = sh_there(git, "rm -rf -- \"$1\"", &[making.as_os_str()]).await;
            return Err(error);
        }
        sh_there(
            git,
            "rm -rf -- \"$2\" && mv -- \"$1\" \"$2\" && \
             for old in \"$3\"/*; do [ \"$old\" = \"$2\" ] || rm -rf -- \"$old\"; done",
            &[making.as_os_str(), self.dir.as_os_str(), parent.as_os_str()],
        )
        .await?;
        Ok(())
    }
}

/// `npm ci`'s arguments: exactly the lockfile, no scripts, nothing else said.
const NPM_CI: [&str; 6] = [
    "ci",
    "--ignore-scripts",
    "--legacy-peer-deps",
    "--no-audit",
    "--no-fund",
    "--no-update-notifier",
];

/// Waits for npm (`npm`, running) to end, or stops it when `cancel` asks; fails with the end of
/// what it said.
async fn run_npm(mut npm: Running, cancel: &CancelToken) -> Result<(), AgentError> {
    // What it prints goes nowhere: only its words on standard error tell how it failed.
    let mut printed = vec![0; 64 * 1024];
    while npm.read(&mut printed).await? > 0 {}
    let Some(status) = npm.wait_unless(cancel).await? else {
        return Err(AgentError::Cancelled);
    };
    if status.success() {
        Ok(())
    } else {
        let said = npm.said();
        let tail = said
            .char_indices()
            .rev()
            .nth(4096)
            .map_or(said.as_str(), |(at, _)| said.get(at..).unwrap_or_default());
        Err(AgentError::Install(format!(
            "npm ci failed ({status}): {}",
            tail.trim()
        )))
    }
}

/// Writes the package into `dir` and runs `npm ci` there.
async fn write_and_install(
    dir: &Path,
    node: &Node,
    cancel: &CancelToken,
) -> Result<(), AgentError> {
    std::fs::write(dir.join("bridge.mjs"), BRIDGE)?;
    std::fs::write(dir.join("package.json"), PACKAGE)?;
    std::fs::write(dir.join("package-lock.json"), LOCKFILE)?;
    let mut command = Command::new(&node.npm);
    command
        .args(NPM_CI)
        .current_dir(dir)
        // npm's own `node` the one found, whatever `npm`'s first line names.
        .env("PATH", path_with(node));
    run_npm(Running::here(command, false)?, cancel).await?;
    let install = Install {
        dir: dir.to_owned(),
        there: None,
    };
    if install.claude().await.is_none() {
        return Err(AgentError::Install(
            "npm installed no Claude Code binary for this platform".to_owned(),
        ));
    }
    std::fs::write(dir.join(READY), SDK_VERSION)?;
    Ok(())
}

/// `PATH` with Node's folder first, then the folders egit looks for programs in.
fn path_with(node: &Node) -> std::ffi::OsString {
    let mut folders: Vec<PathBuf> = node.node.parent().map(Path::to_owned).into_iter().collect();
    let path = egit_git::search_path()
        .map_or_else(|| std::env::var_os("PATH"), |path| Some(path.to_owned()));
    if let Some(path) = path {
        folders.extend(std::env::split_paths(&path));
    }
    std::env::join_paths(folders).unwrap_or_else(|_| OsStr::new("").to_owned())
}

fn remove_quietly(path: &Path) {
    if let Err(error) = std::fs::remove_dir_all(path) {
        tracing::debug!(%error, path = %path.display(), "couldn't remove an installation");
    }
}

/// The installation's folder: the SDK's version and a hash of what egit carries, so a changed
/// bridge installs afresh.
fn folder_name() -> String {
    // FNV-1a: no secret to keep, only changes to notice.
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in [BRIDGE, PACKAGE, LOCKFILE]
        .iter()
        .flat_map(|text| text.bytes())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{SDK_VERSION}-{:08x}", hash >> 32)
}

/// The SDK's names for another computer's platforms (Linux's and macOS's), likeliest first.
const OTHER_PLATFORMS: [&str; 6] = [
    "linux-x64",
    "linux-arm64",
    "linux-x64-musl",
    "linux-arm64-musl",
    "darwin-arm64",
    "darwin-x64",
];

/// The SDK's names for this platform's package, likeliest first.
const fn platforms() -> &'static [&'static str] {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        &["linux-x64", "linux-x64-musl"]
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        &["linux-arm64", "linux-arm64-musl"]
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        &["darwin-arm64"]
    } else if cfg!(target_os = "macos") {
        &["darwin-x64"]
    } else if cfg!(all(windows, target_arch = "aarch64")) {
        &["win32-arm64"]
    } else if cfg!(windows) {
        &["win32-x64"]
    } else {
        &[]
    }
}

/// Runs the `sh` script `snippet` with `args` ($1 on) on the computer `git` runs on; returns what
/// it printed, or why it failed (its words).
async fn sh_there(git: &Git, snippet: &str, args: &[&OsStr]) -> Result<Vec<u8>, AgentError> {
    let mut words = vec![OsStr::new("-c"), OsStr::new(snippet), OsStr::new("sh")];
    words.extend_from_slice(args);
    let mut sh = git.run_there(OsStr::new("sh"), &words, None, (&[], &[]), false)?;
    let printed = sh.read_to_end(ANSWER_LIMIT).await?;
    let status = sh.wait().await?;
    if status.success() {
        Ok(printed)
    } else {
        let said = sh.said();
        Err(AgentError::Install(if said.trim().is_empty() {
            format!("a command there ended: {status}")
        } else {
            said.trim().to_owned()
        }))
    }
}

/// The most a script there may print.
const ANSWER_LIMIT: usize = 1 << 20;

#[cfg(test)]
mod tests {
    use egit_git::Wait as _;

    use super::*;

    #[test]
    fn an_installation_is_named_for_what_it_holds() {
        let install = Install::at(Path::new("/cache"));
        let name = install.dir().file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with("0.3.296-"), "{name}");
        assert_eq!(name.len(), "0.3.296-".len() + 8);
        assert!(!install.is_ready().wait());
        // The lockfile pins the version the SDK's constant says, and only Anthropic's packages.
        let lock: serde_json::Value = serde_json::from_str(LOCKFILE).unwrap();
        let packages = lock["packages"].as_object().unwrap();
        assert_eq!(
            packages["node_modules/@anthropic-ai/claude-agent-sdk"]["version"],
            SDK_VERSION
        );
        for (name, package) in packages {
            assert!(
                name.is_empty() || name.starts_with("node_modules/@anthropic-ai/"),
                "{name}"
            );
            assert!(
                name.is_empty() || package["integrity"].is_string(),
                "{name}"
            );
        }
    }
}
