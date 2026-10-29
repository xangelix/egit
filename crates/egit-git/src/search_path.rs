//! Where egit looks for the programs it runs on this computer (git, Node), and the `PATH` they
//! get.
//!
//! An app macOS starts from the Finder or the Dock gets launchd's `PATH`
//! (`/usr/bin:/bin:/usr/sbin:/sbin`): no Homebrew, `MacPorts`, or Nix, whose git is the one a
//! person runs in a terminal (Apple's `/usr/bin/git` may be older than egit supports), nor
//! git-lfs, gpg, ssh's newer builds, or Node. Started so (no terminal: `TERM` unset), egit asks
//! the person's login shell for its `PATH`, as a terminal would see it (`$SHELL -l -i -c`, its
//! answer between markers whatever the profile prints, within [`SHELL_PATIENCE`], the shell's
//! whole group stopped past it), and looks there first, then in the inherited `PATH`. A shell
//! that doesn't answer leaves the well-known folders of a Mac's package managers
//! ([`WELL_KNOWN`]) ahead of the inherited `PATH`. Everywhere else, and on a Mac started from a
//! terminal, the inherited `PATH` is the person's own: [`search_path`] is `None`.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

/// How long a login shell may take to say its `PATH`.
pub const SHELL_PATIENCE: Duration = Duration::from_secs(3);

/// The folders a Mac's package managers put programs in, looked in first when the login shell
/// doesn't answer: Homebrew's (Apple Silicon, then Intel), `MacPorts`', and Nix's.
pub const WELL_KNOWN: [&str; 8] = [
    "/opt/homebrew/bin",
    "/opt/homebrew/sbin",
    "/usr/local/bin",
    "/usr/local/sbin",
    "/opt/local/bin",
    "/opt/local/sbin",
    "/nix/var/nix/profiles/default/bin",
    "/run/current-system/sw/bin",
];

/// What marks the start and end of the shell's answer: a record separator and a word no
/// profile prints.
#[cfg(unix)]
const MARK: &str = "\u{1e}EGIT-PATH\u{1e}";

/// The `PATH` egit looks for programs on, and gives those it runs here, where it isn't the one
/// egit inherited.
///
/// On a Mac started without a terminal, the login shell's (see the module); else `None`, the
/// inherited one as it is. Worked out once, on first use (it may run the login shell: never on
/// the interface's thread).
#[must_use]
pub fn search_path() -> Option<&'static OsStr> {
    static RESOLVED: OnceLock<Option<OsString>> = OnceLock::new();
    RESOLVED
        .get_or_init(|| {
            if !cfg!(target_os = "macos") || std::env::var_os("TERM").is_some_and(|t| !t.is_empty())
            {
                return None;
            }
            let inherited = std::env::var_os("PATH").unwrap_or_default();
            let shell = std::env::var_os("SHELL")
                .filter(|shell| !shell.is_empty())
                .unwrap_or_else(|| OsString::from("/bin/zsh"));
            let candidates: Vec<&Path> = WELL_KNOWN.iter().map(Path::new).collect();
            let resolved = ask_login_shell(&shell, SHELL_PATIENCE).map_or_else(
                || {
                    tracing::warn!(
                        shell = %Path::new(&shell).display(),
                        "the login shell didn't say its PATH: looking in the package managers' folders"
                    );
                    with_well_known(&inherited, &candidates)
                },
                |said| merged(&said, &inherited),
            );
            tracing::debug!(path = %Path::new(&resolved).display(), "programs are looked for on");
            (resolved != inherited).then_some(resolved)
        })
        .as_deref()
}

/// The first executable called `name` in a folder of [`search_path`], else of the inherited
/// `PATH`.
#[must_use]
pub fn find_program(name: &OsStr) -> Option<PathBuf> {
    let path =
        search_path().map_or_else(|| std::env::var_os("PATH"), |path| Some(path.to_owned()))?;
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

/// `said` (the login shell's `PATH`) first, then each folder of `inherited` it lacks.
fn merged(said: &OsStr, inherited: &OsStr) -> OsString {
    joined(std::env::split_paths(said).chain(std::env::split_paths(inherited)))
}

/// Each folder of `candidates` that exists and `inherited` lacks, then `inherited`.
fn with_well_known(inherited: &OsStr, candidates: &[&Path]) -> OsString {
    let present: Vec<PathBuf> = std::env::split_paths(inherited).collect();
    let added = candidates
        .iter()
        .filter(|dir| dir.is_dir() && !present.iter().any(|p| p == *dir))
        .map(|dir| dir.to_path_buf());
    joined(added.chain(present.iter().cloned()))
}

/// Folders joined into a `PATH`, each once, in order, none empty.
fn joined(folders: impl Iterator<Item = PathBuf>) -> OsString {
    let mut seen: Vec<PathBuf> = Vec::new();
    for folder in folders {
        if !folder.as_os_str().is_empty() && !seen.contains(&folder) {
            seen.push(folder);
        }
    }
    std::env::join_paths(seen).unwrap_or_default()
}

/// The `PATH` an interactive login shell `shell` sets, as it prints it between [`MARK`]s,
/// whatever its profile prints around it; `None` when it doesn't say within `patience` (its
/// group then stopped), fails, or says nothing.
#[cfg(unix)]
fn ask_login_shell(shell: &OsStr, patience: Duration) -> Option<OsString> {
    use std::io::Read as _;
    use std::os::unix::ffi::OsStringExt as _;
    use std::process::{Command, Stdio};

    let mut command = Command::new(shell);
    command
        .args(["-l", "-i", "-c"])
        .arg(format!(r#"printf '%s%s%s' '{MARK}' "$PATH" '{MARK}'"#))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut shell_process = crate::process::Detached::spawn(&mut command).ok()?;
    let stdout = shell_process.child_mut().stdout.take()?;
    let (said, heard) = std::sync::mpsc::channel();
    // Read on a thread of its own: a profile that never ends must not hold egit.
    std::thread::Builder::new()
        .name("login-shell".to_owned())
        .spawn(move || {
            let mut bytes = Vec::new();
            let _ = stdout.take(1 << 20).read_to_end(&mut bytes);
            let _ = said.send(bytes);
        })
        .ok()?;
    let bytes = heard.recv_timeout(patience).ok();
    shell_process.stop(Duration::from_millis(200));
    let bytes = bytes?;
    let text = bytes.as_slice();
    let mark = MARK.as_bytes();
    let start = find(text, mark)?.checked_add(mark.len())?;
    let rest = text.get(start..)?;
    let end = find(rest, mark)?;
    let path = rest.get(..end)?;
    (!path.is_empty()).then(|| OsString::from_vec(path.to_vec()))
}

/// Nowhere else asks a login shell.
#[cfg(not(unix))]
const fn ask_login_shell(_shell: &OsStr, _patience: Duration) -> Option<OsString> {
    None
}

/// Where `needle` first starts in `haystack`.
#[cfg(unix)]
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(folders: &[&str]) -> OsString {
        std::env::join_paths(folders).unwrap()
    }

    #[test]
    fn the_shell_s_folders_come_first_each_once() {
        assert_eq!(
            merged(
                &path(&["/opt/homebrew/bin", "/usr/bin", "/Users/a/.nvm/bin"]),
                &path(&["/usr/bin", "/bin", "/usr/sbin"])
            ),
            path(&[
                "/opt/homebrew/bin",
                "/usr/bin",
                "/Users/a/.nvm/bin",
                "/bin",
                "/usr/sbin"
            ])
        );
    }

    #[test]
    fn the_well_known_folders_that_exist_go_first() {
        let there = tempfile::tempdir().unwrap();
        let brew = there.path().join("brew/bin");
        std::fs::create_dir_all(&brew).unwrap();
        let missing = there.path().join("ports/bin");
        let inherited = path(&["/usr/bin", "/bin"]);
        assert_eq!(
            with_well_known(&inherited, &[&brew, &missing]),
            std::env::join_paths([brew.as_path(), Path::new("/usr/bin"), Path::new("/bin")])
                .unwrap()
        );
        // One the inherited PATH holds keeps its place.
        let holding = std::env::join_paths([Path::new("/usr/bin"), brew.as_path()]).unwrap();
        assert_eq!(with_well_known(&holding, &[&brew]), holding);
    }

    /// A stand-in login shell: runs the command it is given, its profile printing around it.
    #[cfg(unix)]
    fn shell(dir: &Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt as _;
        let script = dir.join("shell");
        std::fs::write(&script, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        script
    }

    #[cfg(unix)]
    #[test]
    fn the_login_shell_s_path_is_read_between_its_marks() {
        let dir = tempfile::tempdir().unwrap();
        // `-l -i -c <command>`: the command is the fourth argument.
        let chatty = shell(
            dir.path(),
            "echo 'Welcome!'; PATH=/opt/homebrew/bin:/usr/bin; export PATH; eval \"$4\"; echo bye",
        );
        assert_eq!(
            ask_login_shell(chatty.as_os_str(), SHELL_PATIENCE),
            Some(OsString::from("/opt/homebrew/bin:/usr/bin"))
        );
        // A shell that never answers is stopped.
        let hung = shell(dir.path(), "sleep 30");
        let started = std::time::Instant::now();
        assert_eq!(
            ask_login_shell(hung.as_os_str(), Duration::from_millis(300)),
            None
        );
        assert!(started.elapsed() < Duration::from_secs(5));
        // One that says nothing, or isn't there.
        let silent = shell(dir.path(), "exit 0");
        assert_eq!(ask_login_shell(silent.as_os_str(), SHELL_PATIENCE), None);
        assert_eq!(
            ask_login_shell(OsStr::new("/nonexistent/shell"), SHELL_PATIENCE),
            None
        );
    }
}
