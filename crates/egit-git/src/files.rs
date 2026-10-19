//! The files of the computer git runs on, as egit reads and writes them directly (an
//! operation's state files, `.gitignore`, a rebase's todo): through `std::fs` here; on another
//! computer through a command there each (a few at once where it can), `sh` running a small
//! script with the paths as its arguments.

use std::ffi::{OsStr, OsString};
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};

use crate::git::Git;
use crate::host::{self, GitStdout, Host, PidReader, Remote, RemotePid, SshHost};

/// How a script there says the path asked about isn't there.
const MISSING: i32 = 3;

/// The files of the computer a [`Git`] runs on.
#[derive(Clone, Copy, Debug)]
pub struct Files<'g> {
    git: &'g Git,
}

impl<'g> Files<'g> {
    pub(crate) const fn of(git: &'g Git) -> Self {
        Self { git }
    }

    /// `path`'s bytes.
    ///
    /// # Errors
    ///
    /// [`io::ErrorKind::NotFound`] when it isn't there, and when it can't be read.
    pub fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        match self.git.host() {
            Host::Local => std::fs::read(path),
            Host::Ssh(ssh) => {
                let ran = self.run(
                    ssh,
                    "[ -e \"$1\" ] || exit 3; exec cat -- \"$1\"",
                    &[path.as_os_str()],
                    None,
                )?;
                ran.into_result(path)
            }
        }
    }

    /// Each of `paths`' bytes, or `None` where it isn't there: one command there for all.
    ///
    /// # Errors
    ///
    /// When one there can't be read.
    pub fn read_all(&self, paths: &[&Path]) -> io::Result<Vec<Option<Vec<u8>>>> {
        match self.git.host() {
            Host::Local => paths
                .iter()
                .map(|path| match std::fs::read(path) {
                    Ok(bytes) => Ok(Some(bytes)),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
                    Err(error) => Err(error),
                })
                .collect(),
            Host::Ssh(ssh) => {
                let args: Vec<&OsStr> = paths.iter().map(|path| path.as_os_str()).collect();
                // Each its size on a line of its own ("-" for none), then its bytes.
                let ran = self.run(
                    ssh,
                    "for path in \"$@\"; do \
                     if [ -f \"$path\" ]; then wc -c < \"$path\" && cat -- \"$path\" || exit 1; \
                     else echo -; fi; done",
                    &args,
                    None,
                )?;
                split_sized(&ran.into_result(Path::new(""))?, paths.len())
            }
        }
    }

    /// Whether git would run each of `paths` as a program: a file, executable where files have
    /// the bit. One command there for all.
    ///
    /// # Errors
    ///
    /// When they can't be looked at there.
    pub fn runnable(&self, paths: &[&Path]) -> io::Result<Vec<bool>> {
        match self.git.host() {
            Host::Local => Ok(paths.iter().map(|path| runnable_here(path)).collect()),
            Host::Ssh(ssh) => {
                let args: Vec<&OsStr> = paths.iter().map(|path| path.as_os_str()).collect();
                let ran = self.run(
                    ssh,
                    "for path in \"$@\"; do \
                     if [ -f \"$path\" ] && [ -x \"$path\" ]; then printf 1; else printf 0; fi; \
                     done",
                    &args,
                    None,
                )?;
                flags(&ran.into_result(Path::new(""))?, paths.len())
            }
        }
    }

    /// What is at each of `paths` (a link not followed): one command there for all.
    ///
    /// # Errors
    ///
    /// When one can't be looked at.
    pub fn look(&self, paths: &[&Path]) -> io::Result<Vec<Entry>> {
        match self.git.host() {
            Host::Local => paths.iter().map(|path| look_here(path)).collect(),
            Host::Ssh(ssh) => {
                let args: Vec<&OsStr> = paths.iter().map(|path| path.as_os_str()).collect();
                let ran = self.run(
                    ssh,
                    "for path in \"$@\"; do \
                     if [ -L \"$path\" ]; then printf L; readlink -- \"$path\" | wc -c; \
                     readlink -- \"$path\"; \
                     elif [ -d \"$path\" ]; then printf D; \
                     elif [ -e \"$path\" ]; then printf F; \
                     if [ -x \"$path\" ]; then printf 1; else printf 0; fi; wc -c < \"$path\"; \
                     else printf N; fi || exit 1; done",
                    &args,
                    None,
                )?;
                read_entries(&ran.into_result(Path::new(""))?, paths.len())
            }
        }
    }

    /// Puts `bytes` at `path` as `put` says, never seen half written: a file written beside it
    /// and renamed over it (its permissions those of the file it replaces, executable as
    /// asked); a link where links are made. Makes the folders it is in.
    ///
    /// # Errors
    ///
    /// When it can't be written.
    pub fn put(&self, path: &Path, bytes: &[u8], put: Put) -> io::Result<()> {
        match self.git.host() {
            Host::Local => put_here(path, bytes, put),
            Host::Ssh(ssh) => {
                let ran = match put {
                    Put::Link => self.run(
                        ssh,
                        "mkdir -p -- \"$(dirname -- \"$1\")\" && rm -f -- \"$1\" && \
                         exec ln -s -- \"$(cat)\" \"$1\"",
                        &[path.as_os_str()],
                        Some(bytes),
                    )?,
                    Put::File { executable } => self.run(
                        ssh,
                        "folder=$(dirname -- \"$1\") && mkdir -p -- \"$folder\" && \
                         new=$(mktemp \"$folder/.egit-XXXXXXXX.tmp\") && cat > \"$new\" || exit 1; \
                         bits=644; if [ -f \"$1\" ] && [ ! -L \"$1\" ]; then \
                         bits=$(stat -c %a -- \"$1\") || bits=644; fi; bits=$((0$bits)); \
                         if [ \"$2\" = 1 ]; then bits=$((bits | ((bits & 0444) >> 2))); \
                         else bits=$((bits & ~0111)); fi; \
                         chmod \"$(printf %o \"$bits\")\" \"$new\" && exec mv -f -- \"$new\" \"$1\"",
                        &[
                            path.as_os_str(),
                            OsStr::new(if executable { "1" } else { "0" }),
                        ],
                        Some(bytes),
                    )?,
                };
                ran.into_result(path).map(drop)
            }
        }
    }

    /// Removes the file (or link) at `path`, if there is one.
    ///
    /// # Errors
    ///
    /// When it can't be removed.
    pub fn remove_if_there(&self, path: &Path) -> io::Result<()> {
        match self.git.host() {
            Host::Local => remove_here(path),
            Host::Ssh(ssh) => self
                .run(ssh, "exec rm -f -- \"$1\"", &[path.as_os_str()], None)?
                .into_result(path)
                .map(drop),
        }
    }

    /// Whether each of `paths` is there (a link to nothing counts): one command there for all.
    ///
    /// # Errors
    ///
    /// When they can't be looked at.
    pub fn exist(&self, paths: &[&Path]) -> io::Result<Vec<bool>> {
        match self.git.host() {
            Host::Local => paths
                .iter()
                .map(|path| Ok(std::fs::exists(path)? || path.is_symlink()))
                .collect(),
            Host::Ssh(ssh) => {
                let args: Vec<&OsStr> = paths.iter().map(|path| path.as_os_str()).collect();
                let ran = self.run(
                    ssh,
                    "for path in \"$@\"; do \
                     if [ -e \"$path\" ] || [ -L \"$path\" ]; then printf 1; else printf 0; fi; \
                     done",
                    &args,
                    None,
                )?;
                flags(&ran.into_result(Path::new(""))?, paths.len())
            }
        }
    }

    /// Whether `path` is there.
    ///
    /// # Errors
    ///
    /// When it can't be looked at.
    pub fn exists(&self, path: &Path) -> io::Result<bool> {
        Ok(self.exist(&[path])?.first() == Some(&true))
    }

    /// Whether `path` is a file (following links); not when it can't be looked at.
    #[must_use]
    pub fn is_file(&self, path: &Path) -> bool {
        match self.git.host() {
            Host::Local => path.is_file(),
            Host::Ssh(ssh) => self
                .run(ssh, "[ -f \"$1\" ]", &[path.as_os_str()], None)
                .is_ok_and(|ran| ran.status.success()),
        }
    }

    /// Writes `bytes` to `path`, replacing what it held.
    ///
    /// # Errors
    ///
    /// When it can't be written.
    pub fn write(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        match self.git.host() {
            Host::Local => std::fs::write(path, bytes),
            Host::Ssh(ssh) => self
                .run(ssh, "exec cat > \"$1\"", &[path.as_os_str()], Some(bytes))?
                .into_result(path)
                .map(drop),
        }
    }

    /// Adds `bytes` at `path`'s end (making it, if it isn't there).
    ///
    /// # Errors
    ///
    /// When it can't be written.
    pub fn append(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        match self.git.host() {
            Host::Local => std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)?
                .write_all(bytes),
            Host::Ssh(ssh) => self
                .run(ssh, "exec cat >> \"$1\"", &[path.as_os_str()], Some(bytes))?
                .into_result(path)
                .map(drop),
        }
    }

    /// Removes the file `path`.
    ///
    /// # Errors
    ///
    /// [`io::ErrorKind::NotFound`] when it isn't there, and when it can't be removed.
    pub fn remove_file(&self, path: &Path) -> io::Result<()> {
        match self.git.host() {
            Host::Local => std::fs::remove_file(path),
            Host::Ssh(ssh) => self
                .run(
                    ssh,
                    "[ -e \"$1\" ] || [ -L \"$1\" ] || exit 3; exec rm -f -- \"$1\"",
                    &[path.as_os_str()],
                    None,
                )?
                .into_result(path)
                .map(drop),
        }
    }

    /// Makes the folder `path` and those it is in, where they aren't.
    ///
    /// # Errors
    ///
    /// When it can't be made.
    pub fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        match self.git.host() {
            Host::Local => std::fs::create_dir_all(path),
            Host::Ssh(ssh) => self
                .run(ssh, "exec mkdir -p -- \"$1\"", &[path.as_os_str()], None)?
                .into_result(path)
                .map(drop),
        }
    }

    /// A new, empty folder of egit's own (its name starting `prefix`), removed with what it
    /// holds when dropped.
    ///
    /// # Errors
    ///
    /// When it can't be made.
    pub fn temp_dir(&self, prefix: &str) -> io::Result<TempDir> {
        match self.git.host() {
            Host::Local => Ok(TempDir::Here(
                tempfile::Builder::new().prefix(prefix).tempdir()?,
            )),
            Host::Ssh(ssh) => {
                let template = OsString::from(format!("{prefix}XXXXXXXX"));
                let made = self
                    .run(
                        ssh,
                        "exec mktemp -d \"${TMPDIR:-/tmp}/$1\"",
                        &[&template],
                        None,
                    )?
                    .into_result(Path::new(""))?;
                let path = bstr::ByteSlice::to_path_lossy(made.trim_ascii_end()).into_owned();
                Ok(TempDir::There {
                    path,
                    git: self.git.clone(),
                })
            }
        }
    }

    /// Runs `script` with `sh` there, `args` its arguments (`$1` on), `input` its standard
    /// input; returns how it ended and what it printed.
    fn run(
        self,
        ssh: &SshHost,
        script: &str,
        args: &[&OsStr],
        input: Option<&[u8]>,
    ) -> io::Result<Ran> {
        let mut words = vec![OsStr::new("-c"), OsStr::new(script), OsStr::new("sh")];
        words.extend_from_slice(args);
        let environment = self.git.remote_environment();
        let script = host::script(&Remote {
            program: OsStr::new("sh"),
            args: words,
            dir: None,
            unset: environment
                .map(|environment| environment.removals().collect())
                .unwrap_or_default(),
            set: environment
                .map(|environment| environment.settings().collect())
                .unwrap_or_default(),
        });
        let mut command = ssh.command(&script);
        self.git.environment().apply(&mut command);
        command
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = crate::process::Detached::spawn(&mut command)?;
        let process = child.child_mut();
        let stdin = process.stdin.take();
        let stdout = process.stdout.take();
        let stderr = process.stderr.take();
        let (Some(stdout), Some(mut stderr)) = (stdout, stderr) else {
            return Err(io::Error::other("ssh's pipes weren't set up"));
        };
        let feeding = match (stdin, input) {
            (Some(mut stdin), Some(input)) => {
                let input = input.to_vec();
                Some(std::thread::spawn(move || stdin.write_all(&input)))
            }
            _ => None,
        };
        let draining = std::thread::spawn(move || {
            let mut said = Vec::new();
            stderr.read_to_end(&mut said).map(|_| said)
        });
        let mut stdout = GitStdout::new(stdout, self.git.host());
        let mut printed = Vec::new();
        stdout.read_to_end(&mut printed)?;
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        };
        if let Some(feeding) = feeding {
            feeding
                .join()
                .map_err(|_| io::Error::other("feeding ssh panicked"))??;
        }
        let said = draining
            .join()
            .map_err(|_| io::Error::other("reading ssh panicked"))??;
        let mut reader = PidReader::new(RemotePid::default());
        let mut stderr = reader
            .feed(&said)
            .map(std::borrow::Cow::into_owned)
            .unwrap_or_default();
        stderr.extend(reader.finish());
        Ok(Ran {
            status,
            stdout: printed,
            stderr,
        })
    }
}

/// Whether git would run the file at `path` here: a file, executable where files have the bit.
fn runnable_here(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        meta.is_file()
    }
}

/// A script's answers about `count` paths, a `1` or `0` each.
fn flags(said: &[u8], count: usize) -> io::Result<Vec<bool>> {
    if said.len() != count {
        return Err(io::Error::other("the files there weren't all looked at"));
    }
    Ok(said.iter().map(|byte| *byte == b'1').collect())
}

/// The files [`Files::read_all`]'s script printed: each a size on a line ("-" for none), then
/// that many bytes.
fn split_sized(mut printed: &[u8], count: usize) -> io::Result<Vec<Option<Vec<u8>>>> {
    let garbled = || io::Error::other("the files there came back garbled");
    let mut files = Vec::with_capacity(count);
    for _ in 0..count {
        let end = printed
            .iter()
            .position(|byte| *byte == b'\n')
            .ok_or_else(garbled)?;
        let (line, rest) = printed.split_at(end);
        let rest = rest.get(1..).unwrap_or_default();
        let line = line.trim_ascii();
        if line == b"-" {
            files.push(None);
            printed = rest;
            continue;
        }
        let size: usize = std::str::from_utf8(line)
            .ok()
            .and_then(|size| size.parse().ok())
            .ok_or_else(garbled)?;
        let bytes = rest.get(..size).ok_or_else(garbled)?;
        files.push(Some(bytes.to_vec()));
        printed = rest.get(size..).unwrap_or_default();
    }
    Ok(files)
}

/// What is at a path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    /// Nothing.
    Nothing,
    /// A symbolic link, and its target.
    Link(Vec<u8>),
    /// A file: its size, and whether it is executable.
    File {
        /// Its size, in bytes.
        size: u64,
        /// Whether it is executable (never, where files have no such bit).
        executable: bool,
    },
    /// A folder.
    Folder,
}

/// What [`Files::put`] puts at a path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Put {
    /// A file, executable or not.
    File {
        /// Whether it is to be executable (where files have the bit).
        executable: bool,
    },
    /// A symbolic link (its bytes the target): where links can't be made, a file holding its
    /// target, as git checks a link out without `core.symlinks`.
    Link,
}

/// What [`Files::look`] finds here.
fn look_here(path: &Path) -> io::Result<Entry> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Entry::Nothing),
        Err(error) => return Err(error),
    };
    if metadata.is_symlink() {
        let target = std::fs::read_link(path)?;
        return Ok(Entry::Link(
            <Vec<u8> as bstr::ByteVec>::from_path_lossy(&target).into_owned(),
        ));
    }
    if metadata.is_dir() {
        return Ok(Entry::Folder);
    }
    Ok(Entry::File {
        size: metadata.len(),
        executable: is_executable(&metadata),
    })
}

/// What [`Files::look`]'s script printed: a letter a path (`N`othing, `D`irectory), a file's
/// `F`, its executable bit, and its size on a line, a link's `L` and its target's size (with
/// `readlink`'s newline) on a line, then the target.
fn read_entries(mut printed: &[u8], count: usize) -> io::Result<Vec<Entry>> {
    let garbled = || io::Error::other("the files there came back garbled");
    let line = |printed: &mut &[u8]| -> io::Result<u64> {
        let end = printed
            .iter()
            .position(|byte| *byte == b'\n')
            .ok_or_else(garbled)?;
        let (number, rest) = printed.split_at(end);
        *printed = rest.get(1..).unwrap_or_default();
        std::str::from_utf8(number.trim_ascii())
            .ok()
            .and_then(|number| number.parse().ok())
            .ok_or_else(garbled)
    };
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let (&kind, rest) = printed.split_first().ok_or_else(garbled)?;
        printed = rest;
        entries.push(match kind {
            b'N' => Entry::Nothing,
            b'D' => Entry::Folder,
            b'F' => {
                let (&bit, rest) = printed.split_first().ok_or_else(garbled)?;
                printed = rest;
                Entry::File {
                    executable: bit == b'1',
                    size: line(&mut printed)?,
                }
            }
            b'L' => {
                let size = usize::try_from(line(&mut printed)?).map_err(|_| garbled())?;
                let target = printed.get(..size).ok_or_else(garbled)?;
                printed = printed.get(size..).unwrap_or_default();
                Entry::Link(target.strip_suffix(b"\n").unwrap_or(target).to_vec())
            }
            _ => return Err(garbled()),
        });
    }
    Ok(entries)
}

/// Puts `bytes` at `path` here as `put` says: a file written beside it and renamed over it, so
/// it is never seen half written (its permissions those of the file it replaces, its executable
/// bit as asked); a link where links are made.
fn put_here(path: &Path, bytes: &[u8], put: Put) -> io::Result<()> {
    let folder = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(folder)?;
    #[cfg(unix)]
    {
        if put == Put::Link {
            remove_here(path)?;
            return std::os::unix::fs::symlink(bstr::ByteSlice::to_path_lossy(bytes), path);
        }
    }
    let mut file = tempfile::Builder::new()
        .prefix(".egit-")
        .suffix(".tmp")
        .tempfile_in(folder)?;
    file.write_all(bytes)?;
    let replaced = std::fs::symlink_metadata(path)
        .ok()
        .filter(std::fs::Metadata::is_file);
    let executable = matches!(put, Put::File { executable: true });
    std::fs::set_permissions(
        file.path(),
        permissions(replaced.as_ref(), &file.as_file().metadata()?, executable),
    )?;
    file.persist(path).map_err(|error| error.error)?;
    Ok(())
}

/// Removes the file (or link) at `path` here, if there is one.
fn remove_here(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

#[cfg(unix)]
fn is_executable(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    metadata.permissions().mode() & 0o111 != 0
}

/// No executable bit here: git records files as it finds them in the index.
#[cfg(not(unix))]
const fn is_executable(_metadata: &std::fs::Metadata) -> bool {
    false
}

/// A file's permissions: those of the file it replaces (else as git makes a file), executable
/// where it can be read when asked, not executable otherwise.
#[cfg(unix)]
fn permissions(
    replaced: Option<&std::fs::Metadata>,
    _new: &std::fs::Metadata,
    executable: bool,
) -> std::fs::Permissions {
    use std::os::unix::fs::PermissionsExt as _;
    let bits = replaced.map_or(0o644, |metadata| metadata.permissions().mode() & 0o7777);
    std::fs::Permissions::from_mode(with_executable(bits, executable))
}

/// A file's permissions: read-only only if the file it replaces was.
#[cfg(not(unix))]
fn permissions(
    replaced: Option<&std::fs::Metadata>,
    new: &std::fs::Metadata,
    _executable: bool,
) -> std::fs::Permissions {
    replaced.map_or_else(|| new.permissions(), std::fs::Metadata::permissions)
}

/// `bits` executable (by whoever may read it) or not.
#[cfg_attr(not(unix), expect(dead_code, reason = "files have no such bits there"))]
const fn with_executable(bits: u32, executable: bool) -> u32 {
    if executable {
        bits | ((bits & 0o444) >> 2)
    } else {
        bits & !0o111
    }
}

/// What a script there did.
struct Ran {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl Ran {
    /// What it printed, when it succeeded; that `path` isn't there, or what it said, when not.
    fn into_result(self, path: &Path) -> io::Result<Vec<u8>> {
        if self.status.success() {
            return Ok(self.stdout);
        }
        if self.status.code() == Some(MISSING) {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{} isn't there", path.display()),
            ));
        }
        let said = String::from_utf8_lossy(self.stderr.trim_ascii()).into_owned();
        Err(io::Error::other(if said.is_empty() {
            format!("a command on the other computer ended: {}", self.status)
        } else {
            said
        }))
    }
}

/// A folder of egit's own, removed with what it holds when dropped: here, or on another
/// computer.
#[derive(Debug)]
pub enum TempDir {
    /// On this computer.
    Here(tempfile::TempDir),
    /// On another computer, through its git.
    There {
        /// Where.
        path: PathBuf,
        /// The git it was made through.
        git: Git,
    },
}

impl TempDir {
    /// Where it is.
    #[must_use]
    pub fn path(&self) -> &Path {
        match self {
            Self::Here(dir) => dir.path(),
            Self::There { path, .. } => path,
        }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        if let Self::There { path, git } = self
            && let Some(ssh) = git.host().ssh()
        {
            let removed = git
                .files()
                .run(ssh, "exec rm -rf -- \"$1\"", &[path.as_os_str()], None);
            if let Err(error) = removed {
                tracing::debug!(%error, path = %path.display(), "a folder there wasn't removed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_read_as_the_script_prints_them() {
        let printed = b"NDF1      12\nL4\na\nb\nF0 0\n";
        assert_eq!(
            read_entries(printed, 5).unwrap(),
            [
                Entry::Nothing,
                Entry::Folder,
                Entry::File {
                    size: 12,
                    executable: true
                },
                Entry::Link(b"a\nb".to_vec()),
                Entry::File {
                    size: 0,
                    executable: false
                },
            ]
        );
        assert_eq!(with_executable(0o640, true), 0o750);
        assert_eq!(with_executable(0o755, false), 0o644);
        assert_eq!(with_executable(0o600, false), 0o600);
    }

    #[test]
    fn files_read_together_are_split_by_their_sizes() {
        let printed = b"3\nabc-\n0\n      2\n\n\n";
        assert_eq!(
            split_sized(printed, 4).unwrap(),
            [
                Some(b"abc".to_vec()),
                None,
                Some(Vec::new()),
                Some(b"\n\n".to_vec())
            ]
        );
        assert!(split_sized(b"9\nshort", 1).is_err());
    }
}
