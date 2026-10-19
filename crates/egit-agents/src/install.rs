//! Setting Claude up on this computer: Node.js and npm found, and the SDK installed.
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

use std::ffi::OsStr;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use egit_git::{CancelToken, Detached};

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

/// Node.js and npm, as found on `PATH`.
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
    let node = on_path(node_name).ok_or(AgentError::NodeMissing)?;
    let npm = on_path(npm_name).ok_or(AgentError::NodeMissing)?;
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

/// The first file called `name` in a folder on `PATH`.
fn on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// Where the bridge and the SDK are (or go), for this version of what egit carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Install {
    dir: PathBuf,
}

impl Install {
    /// The installation under `cache` (egit's cache folder).
    #[must_use]
    pub fn at(cache: &Path) -> Self {
        Self {
            dir: cache.join("agents").join("claude").join(folder_name()),
        }
    }

    /// Its folder.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Whether it is installed and whole.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.dir.join(READY).is_file() && self.bridge().is_file() && self.claude().is_some()
    }

    /// The bridge script.
    #[must_use]
    pub fn bridge(&self) -> PathBuf {
        self.dir.join("bridge.mjs")
    }

    /// Claude Code's binary for this platform, as the SDK installed it.
    #[must_use]
    pub fn claude(&self) -> Option<PathBuf> {
        let packages = self.dir.join("node_modules").join("@anthropic-ai");
        let binary = if cfg!(windows) {
            "claude.exe"
        } else {
            "claude"
        };
        platforms()
            .iter()
            .map(|platform| {
                packages
                    .join(format!("claude-agent-sdk-{platform}"))
                    .join(binary)
            })
            .find(|path| path.is_file())
    }

    /// Installs it with `node`'s npm (`npm ci`), unless it is ready already; removes other
    /// installations after. Blocks: run it in a job.
    ///
    /// # Errors
    ///
    /// When the files can't be written, npm fails (its words said), Claude Code's binary isn't
    /// there after, or `cancel` stopped it.
    pub fn install(&self, node: &Node, cancel: &CancelToken) -> Result<(), AgentError> {
        if self.is_ready() {
            return Ok(());
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
        let made = write_and_install(&making, node, cancel);
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

/// Writes the package into `dir` and runs `npm ci` there.
fn write_and_install(dir: &Path, node: &Node, cancel: &CancelToken) -> Result<(), AgentError> {
    std::fs::write(dir.join("bridge.mjs"), BRIDGE)?;
    std::fs::write(dir.join("package.json"), PACKAGE)?;
    std::fs::write(dir.join("package-lock.json"), LOCKFILE)?;
    let mut command = Command::new(&node.npm);
    command
        .args([
            "ci",
            "--ignore-scripts",
            "--legacy-peer-deps",
            "--no-audit",
            "--no-fund",
            "--no-update-notifier",
        ])
        .current_dir(dir)
        // npm's own `node` the one found, whatever `npm`'s first line names.
        .env("PATH", path_with(node))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut npm = Detached::spawn(&mut command)?;
    let stderr = npm.child_mut().stderr.take();
    let reading = std::thread::spawn(move || {
        let mut said = Vec::new();
        if let Some(mut stderr) = stderr {
            // Only its end is kept for a failure's words.
            let _read = stderr.read_to_end(&mut said);
        }
        String::from_utf8_lossy(said.get(said.len().saturating_sub(4096)..).unwrap_or(&said))
            .into_owned()
    });
    let status = loop {
        if cancel.is_cancelled() {
            npm.stop(Duration::ZERO);
            return Err(AgentError::Cancelled);
        }
        if let Some(status) = npm.try_wait()? {
            break status;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let said = reading.join().unwrap_or_default();
    if !status.success() {
        return Err(AgentError::Install(format!(
            "npm ci failed ({status}): {}",
            said.trim()
        )));
    }
    let install = Install {
        dir: dir.to_owned(),
    };
    if install.claude().is_none() {
        return Err(AgentError::Install(
            "npm installed no Claude Code binary for this platform".to_owned(),
        ));
    }
    std::fs::write(dir.join(READY), SDK_VERSION)?;
    Ok(())
}

/// `PATH` with Node's folder first.
fn path_with(node: &Node) -> std::ffi::OsString {
    let mut folders: Vec<PathBuf> = node.node.parent().map(Path::to_owned).into_iter().collect();
    if let Some(path) = std::env::var_os("PATH") {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_installation_is_named_for_what_it_holds() {
        let install = Install::at(Path::new("/cache"));
        let name = install.dir().file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with("0.3.296-"), "{name}");
        assert_eq!(name.len(), "0.3.296-".len() + 8);
        assert!(!install.is_ready());
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
