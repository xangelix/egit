//! Following what changes in a repository on another computer, where egit's own watches can't
//! reach: a session of its own there says what changed, a line at a time.
//!
//! Where `inotifywait` (inotify-tools) is installed there, the session is told at once: it runs
//! it on the git directories, each on its own (`HEAD`, the index, an operation's state, the
//! configuration), on `refs` and every folder inside it, and on the user's configuration's
//! folders, and passes on each path changed, which [`classify`] sorts as a watch here does.
//! Elsewhere, or where those watches can't be set up (inotify's limits), a small `sh` loop looks
//! every few seconds at what a watch would see change (refs through their folders' times, which
//! a ref's lock file renamed over it changes; `packed-refs`; `HEAD`'s text; an operation's
//! files; the configuration's and the index's times) and says what did, a word a line.
//!
//! A work tree's folders are followed by a session of their own ([`look_at_folders_there`]):
//! those its owner names, each on its own, as here, through `inotifywait` only (without it, the
//! work tree's status is read every few seconds instead).
//!
//! Every session says `.` every few seconds, so one whose reader went away without a word (a
//! network gone) ends soon after; one whose reader closed it ends at once, as it stops its
//! watchers when its input ends.

use std::ffi::{OsStr, OsString};
use std::io::{self, BufRead as _, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, ChildStdout, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use bstr::{ByteSlice as _, ByteVec as _};

use crate::git::Git;
use crate::host::{self, GitStdout, Remote};
use crate::process::Detached;
use crate::repo::Repository;

/// The session following a repository: `$1` the git directory, `$2` the common one, `$3` the
/// seconds between looks, `$4` the `inotifywait` to be told by (empty: only look).
const REPOSITORY: &str = r#"
gitdir=$1 common=$2 every=$3 inotifywait=$4
config_home=${XDG_CONFIG_HOME:-$HOME/.config}
if [ -n "$inotifywait" ] && command -v "$inotifywait" >/dev/null 2>&1; then
  trap '' PIPE
  printf 'config %s\n' "$HOME/.gitconfig" "$config_home/git/config"
  set -- "$gitdir"
  [ "$common" = "$gitdir" ] || set -- "$@" "$common"
  [ -d "$config_home/git" ] && set -- "$@" "$config_home/git"
  [ -d "$HOME" ] && set -- "$@" "$HOME"
  changes=modify,close_write,create,delete,moved_to,moved_from
  "$inotifywait" -m --format 'e %w%f' -e "$changes" -- "$@" 2>&1 &
  flat=$!
  "$inotifywait" -m -r --format 'e %w%f' -e "$changes" -- "$common/refs" 2>&1 &
  deep=$!
  exec 3<&0
  { cat <&3 >/dev/null; kill $flat $deep $$ 2>/dev/null; } &
  guard=$!
  trap 'kill $flat $deep $guard 2>/dev/null' EXIT
  echo live
  while sleep 5; do
    kill -0 $flat $deep 2>/dev/null || break
    echo . || exit 0
  done
  kill $flat $deep $guard 2>/dev/null
  trap - EXIT
  trap - PIPE
  echo failed
fi
old_refs= old_state= old_config= old_index=
while :; do
  refs=$( { cat -- "$gitdir/HEAD"; find "$common/refs" "$gitdir/refs" -type d -exec stat -c '%n %y' {} +; stat -c '%y %s %i' -- "$common/packed-refs"; } 2>/dev/null | cksum)
  state=$(for name in MERGE_HEAD CHERRY_PICK_HEAD REVERT_HEAD BISECT_LOG rebase-merge rebase-apply sequencer; do [ -e "$gitdir/$name" ] && echo "$name"; done | cksum)
  config=$(stat -c '%n %y %s' -- "$common/config" "$gitdir/config.worktree" "$HOME/.gitconfig" "$config_home/git/config" 2>/dev/null | cksum)
  index=$(stat -c '%y %s %i' -- "$gitdir/index" 2>/dev/null | cksum)
  [ "$refs" = "$old_refs" ] || echo refs
  [ "$state" = "$old_state" ] || echo state
  [ "$config" = "$old_config" ] || echo config
  [ "$index" = "$old_index" ] || echo index
  echo . || exit 0
  old_refs=$refs old_state=$state old_config=$config old_index=$index
  sleep "$every"
done
"#;

/// The session following a work tree's folders: `$1` the `inotifywait`, `$2` how many bytes of
/// its input name the folders (each ended by a NUL); the rest of its input stays open until its
/// reader goes.
const FOLDERS: &str = r#"
inotifywait=$1 size=$2
command -v "$inotifywait" >/dev/null 2>&1 || { echo missing; exit 0; }
list=$(mktemp "${TMPDIR:-/tmp}/egit-folders.XXXXXXXX") || exit 1
trap 'rm -f "$list"' EXIT
head -c "$size" | xargs -0 sh -c 'for folder do [ -d "$folder" ] && printf "%s\n" "$folder"; done' sh > "$list"
trap '' PIPE
"$inotifywait" -m --format 'e %e %w%f' -e modify,close_write,create,delete,moved_to,moved_from,delete_self --fromfile "$list" 2>&1 &
watcher=$!
exec 3<&0
{ cat <&3 >/dev/null; kill $watcher 2>/dev/null; rm -f "$list"; kill $$ 2>/dev/null; } &
guard=$!
trap 'kill $watcher $guard 2>/dev/null; rm -f "$list"' EXIT
while sleep 5; do
  rm -f "$list"
  kill -0 $watcher 2>/dev/null || { echo failed; exit 0; }
  echo . || exit 0
done
"#;

/// The program telling a session there at once what changed, by its name on `PATH` there.
pub const INOTIFYWAIT: &str = "inotifywait";

/// What changed there: some of `HEAD` and the refs, an operation's state, configuration, and
/// the index.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Looked(u8);

impl Looked {
    /// Nothing.
    pub const NONE: Self = Self(0);
    /// `HEAD`, a branch, a tag, or a remote-tracking branch.
    pub const REFS: Self = Self(1);
    /// An operation's state (a merge or rebase starting or ending).
    pub const STATE: Self = Self(1 << 1);
    /// Configuration.
    pub const CONFIG: Self = Self(1 << 2);
    /// The index (what is staged).
    pub const INDEX: Self = Self(1 << 3);

    /// These and `other`.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Whether any of `other` is among these.
    #[must_use]
    pub const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    /// Whether nothing changed.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// What a change to `path` in a repository's git directories means (`git_dir` the work tree's
/// own, `common_dir` the one every work tree shares).
///
/// As git writes them: a lock file is a write under way (its rename into place is what
/// counts), and objects, logs, and `FETCH_HEAD` change nothing egit shows.
#[must_use]
pub fn classify(path: &Path, git_dir: &Path, common_dir: &Path) -> Looked {
    if path
        .extension()
        .is_some_and(|extension| extension == "lock")
    {
        return Looked::NONE;
    }
    // A linked work tree's git directory is inside the common one: look at it first.
    if let Ok(relative) = path.strip_prefix(git_dir) {
        let found = classify_git_dir(relative);
        if !found.is_empty() || git_dir != common_dir {
            return found;
        }
    }
    path.strip_prefix(common_dir)
        .map_or(Looked::NONE, classify_common_dir)
}

/// A path relative to a work tree's own git directory.
fn classify_git_dir(relative: &Path) -> Looked {
    let Some(first) = relative.components().next() else {
        return Looked::NONE;
    };
    match first.as_os_str().to_str() {
        Some("HEAD") => Looked::REFS.union(Looked::STATE),
        Some("index") => Looked::INDEX,
        Some("config.worktree") => Looked::CONFIG,
        Some(
            "MERGE_HEAD" | "CHERRY_PICK_HEAD" | "REVERT_HEAD" | "BISECT_LOG" | "rebase-merge"
            | "rebase-apply" | "sequencer",
        ) => Looked::STATE,
        _ => Looked::NONE,
    }
}

/// A path relative to the directory every work tree shares.
fn classify_common_dir(relative: &Path) -> Looked {
    let Some(first) = relative.components().next() else {
        return Looked::NONE;
    };
    match first.as_os_str().to_str() {
        Some("refs" | "packed-refs") => Looked::REFS,
        Some("config") => Looked::CONFIG,
        _ => Looked::NONE,
    }
}

/// How a session there follows a repository.
#[derive(Clone, Debug)]
pub struct Follow {
    /// How often it looks, while nothing tells it at once.
    pub every: Duration,
    /// The `inotifywait` telling it at once, where it is installed there (a name on `PATH`
    /// there, or a path); `None` only looks.
    pub inotifywait: Option<OsString>,
}

impl Follow {
    /// Told at once where `inotifywait` is installed there, else looking every `every`.
    #[must_use]
    pub fn at_once(every: Duration) -> Self {
        Self {
            every,
            inotifywait: Some(OsString::from(INOTIFYWAIT)),
        }
    }
}

/// A session following a repository, or a work tree's folders, there; stopped when dropped.
#[derive(Debug)]
pub struct Looking {
    process: Detached,
    /// Its input, kept open: closed, the session stops its watchers there at once.
    _input: Option<ChildStdin>,
    liveness: Liveness,
}

impl Looking {
    /// Whether it stopped (the connection was lost, or it couldn't run).
    #[must_use]
    pub fn has_ended(&self) -> bool {
        self.liveness.has_ended()
    }

    /// Whether it is told at once what changes (`inotifywait` there), rather than looking every
    /// few seconds (or, for folders, not at all).
    #[must_use]
    pub fn is_live(&self) -> bool {
        self.liveness.is_live()
    }

    /// How it stands, to ask without holding it (cheap to clone: it is shared).
    #[must_use]
    pub fn liveness(&self) -> Liveness {
        self.liveness.clone()
    }
}

/// How a [`Looking`] stands, as its reader keeps it.
#[derive(Clone, Debug, Default)]
pub struct Liveness {
    ended: Arc<AtomicBool>,
    live: Arc<AtomicBool>,
}

impl Liveness {
    /// See [`Looking::has_ended`].
    #[must_use]
    pub fn has_ended(&self) -> bool {
        self.ended.load(Ordering::Acquire)
    }

    /// See [`Looking::is_live`].
    #[must_use]
    pub fn is_live(&self) -> bool {
        self.live.load(Ordering::Acquire) && !self.has_ended()
    }
}

impl Drop for Looking {
    fn drop(&mut self) {
        // The session closes; there, its input ends, which stops its watchers.
        self.process.stop(Duration::ZERO);
    }
}

/// Follows `repository` there as `follow` says, handing what changed to `changed` on a thread
/// of its own (nothing for a first look, which only notes how things are).
///
/// # Errors
///
/// When the session can't be started, or `git` runs here (where a watch does better).
pub fn look_there(
    git: &Git,
    repository: &Repository,
    follow: &Follow,
    mut changed: impl FnMut(Looked) + Send + 'static,
) -> io::Result<Looking> {
    let seconds = follow.every.as_secs().max(1).to_string();
    let inotifywait = follow.inotifywait.clone().unwrap_or_default();
    let args = [
        repository.git_dir().as_os_str(),
        repository.common_dir().as_os_str(),
        OsStr::new(&seconds),
        &inotifywait,
    ];
    let (process, stdout, input) = start(git, REPOSITORY, &args)?;
    let (git_dir, common_dir) = (
        repository.git_dir().to_owned(),
        repository.common_dir().to_owned(),
    );
    let mut config_files: Vec<PathBuf> = Vec::new();
    // Until the first look is done (or the session says it is told), what is said is how
    // things were, not a change.
    let mut first = true;
    // Told at once once both of its watchers have set up their watches.
    let mut established = 0_u8;
    read_lines(git, (stdout, process), (input, &[]), move |line, live| {
        let looked = match line {
            // A look done, or told from now on.
            b"." | b"live" => {
                first = false;
                return;
            }
            b"Watches established." => {
                established = established.saturating_add(1);
                if established >= 2 {
                    live.store(true, Ordering::Release);
                }
                return;
            }
            // Looking from now on: its first look notes how things are.
            b"failed" => {
                live.store(false, Ordering::Release);
                first = true;
                return;
            }
            b"refs" => Looked::REFS,
            b"state" => Looked::STATE,
            b"config" => Looked::CONFIG,
            b"index" => Looked::INDEX,
            _ => {
                if let Some(file) = line.strip_prefix(b"config ") {
                    config_files.push(file.to_path_lossy().into_owned());
                } else if let Some(path) = line.strip_prefix(b"e ") {
                    let path = path.to_path_lossy();
                    let looked = if config_files.iter().any(|file| *file == *path) {
                        Looked::CONFIG
                    } else {
                        classify(&path, &git_dir, &common_dir)
                    };
                    if !looked.is_empty() {
                        changed(looked);
                    }
                } else {
                    tracing::debug!(said = %line.as_bstr(), "looking there");
                }
                return;
            }
        };
        if !first {
            changed(looked);
        }
    })
}

/// What a session following a work tree's folders there saw.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Seen {
    /// Its watches are set up: from now on, it is told at once.
    Ready,
    /// Something in one of the folders changed.
    Changed,
    /// A folder was made (or moved) into one of them (something changed, too).
    Created(PathBuf),
    /// `inotifywait` isn't installed there.
    Missing,
    /// Its watches couldn't be set up, or stopped (inotify's limits, most likely): why, in its
    /// words, when it said.
    Failed(String),
}

/// Follows `folders` of `repository`'s work tree there, each on its own (never what is inside
/// them), through `inotifywait` (`program`), handing what it saw to `seen` on a thread of its
/// own.
///
/// A folder no longer there is left out; one whose path holds a line break can't be named to
/// `inotifywait`, and isn't followed.
///
/// # Errors
///
/// When the session can't be started, or `git` runs here.
pub fn look_at_folders_there<'p>(
    git: &Git,
    repository: &Repository,
    folders: impl IntoIterator<Item = &'p Path>,
    program: &OsStr,
    mut seen: impl FnMut(Seen) + Send + 'static,
) -> io::Result<Looking> {
    let mut list = Vec::new();
    for folder in folders {
        let bytes = Vec::from_path_lossy(folder);
        if folder.is_absolute() && !bytes.contains(&b'\n') && !bytes.contains(&0) {
            list.extend_from_slice(&bytes);
            list.push(0);
        }
    }
    let size = list.len().to_string();
    let (process, stdout, input) = start(git, FOLDERS, &[program, OsStr::new(&size)])?;
    let git_dir = repository.git_dir().to_owned();
    let mut complaint = String::new();
    let mut handle = move |line: &[u8], live: &AtomicBool| {
        if let Some(event) = line.strip_prefix(b"e ") {
            let Some((events, path)) = event.split_once_str(" ") else {
                return;
            };
            let path = path.to_path_lossy();
            if path.starts_with(&git_dir) {
                return;
            }
            let folder = events.contains_str("ISDIR")
                && (events.contains_str("CREATE") || events.contains_str("MOVED_TO"));
            seen(if folder {
                Seen::Created(path.into_owned())
            } else {
                Seen::Changed
            });
            return;
        }
        match line {
            b"." | b"Setting up watches." => {}
            b"Watches established." => {
                live.store(true, Ordering::Release);
                seen(Seen::Ready);
            }
            b"missing" => seen(Seen::Missing),
            b"failed" => {
                live.store(false, Ordering::Release);
                seen(Seen::Failed(std::mem::take(&mut complaint)));
            }
            // What `inotifywait` said: why it couldn't watch, kept for when it gives up.
            said => said.to_str_lossy().trim().clone_into(&mut complaint),
        }
    };
    read_lines(git, (stdout, process), (input, &list), move |line, live| {
        handle(line, live);
    })
}

/// Starts `script` (with `args` as `$1` on) there in a session of its own: the session, its
/// output (from the script's marker on), and its input.
fn start(
    git: &Git,
    script: &str,
    args: &[&OsStr],
) -> io::Result<(Detached, ChildStdout, Option<ChildStdin>)> {
    let Some(ssh) = git.host().ssh() else {
        return Err(io::Error::other("the repository is on this computer"));
    };
    let environment = git.remote_environment();
    let mut words = vec![OsStr::new("-c"), OsStr::new(script), OsStr::new("sh")];
    words.extend_from_slice(args);
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
    git.environment().apply(&mut command);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut process = Detached::spawn(&mut command)?;
    let stdout = process
        .child_mut()
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("ssh's output wasn't set up"))?;
    let input = process.child_mut().stdin.take();
    Ok((process, stdout, input))
}

/// Reads the session's lines on a thread of its own, each trimmed, handing them to `handle`
/// with the flag saying whether it is told at once; marks it ended when its output ends. Its
/// input is given `written` once its reader is up (so neither side waits on the other's pipe),
/// then kept open.
fn read_lines(
    git: &Git,
    (stdout, process): (ChildStdout, Detached),
    (mut input, written): (Option<ChildStdin>, &[u8]),
    mut handle: impl FnMut(&[u8], &AtomicBool) + Send + 'static,
) -> io::Result<Looking> {
    let liveness = Liveness::default();
    let (marking, telling) = (Arc::clone(&liveness.ended), Arc::clone(&liveness.live));
    let host = git.host().clone();
    std::thread::Builder::new()
        .name("looking there".to_owned())
        .spawn(move || {
            let mut lines = BufReader::new(GitStdout::new(stdout, &host));
            let mut line = Vec::new();
            loop {
                line.clear();
                match lines.read_until(b'\n', &mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => handle(line.trim_ascii_end(), &telling),
                }
            }
            telling.store(false, Ordering::Release);
            marking.store(true, Ordering::Release);
        })?;
    if !written.is_empty() {
        let input = input
            .as_mut()
            .ok_or_else(|| io::Error::other("ssh's input wasn't set up"))?;
        input.write_all(written)?;
        input.flush()?;
    }
    Ok(Looking {
        process,
        _input: input,
        liveness,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn main_worktree(path: &str) -> Looked {
        classify(Path::new(path), Path::new("/r/.git"), Path::new("/r/.git"))
    }

    fn linked_worktree(path: &str) -> Looked {
        classify(
            Path::new(path),
            Path::new("/r/.git/worktrees/w"),
            Path::new("/r/.git"),
        )
    }

    #[test]
    fn sorts_a_main_work_trees_changes() {
        assert_eq!(
            main_worktree("/r/.git/HEAD"),
            Looked::REFS.union(Looked::STATE)
        );
        assert_eq!(main_worktree("/r/.git/refs/heads/feature/x"), Looked::REFS);
        assert_eq!(main_worktree("/r/.git/packed-refs"), Looked::REFS);
        assert_eq!(main_worktree("/r/.git/config"), Looked::CONFIG);
        assert_eq!(main_worktree("/r/.git/index"), Looked::INDEX);
        assert_eq!(main_worktree("/r/.git/MERGE_HEAD"), Looked::STATE);
        assert_eq!(main_worktree("/r/.git/rebase-merge/done"), Looked::STATE);
    }

    #[test]
    fn ignores_what_isnt_state() {
        assert_eq!(main_worktree("/r/.git/index.lock"), Looked::NONE);
        assert_eq!(main_worktree("/r/.git/refs/heads/main.lock"), Looked::NONE);
        assert_eq!(main_worktree("/r/.git/objects/ab/cdef"), Looked::NONE);
        assert_eq!(main_worktree("/r/.git/logs/HEAD"), Looked::NONE);
        assert_eq!(main_worktree("/r/.git/FETCH_HEAD"), Looked::NONE);
        assert_eq!(main_worktree("/elsewhere/HEAD"), Looked::NONE);
    }

    #[test]
    fn a_linked_work_tree_has_its_own_head_and_shares_refs() {
        assert_eq!(
            linked_worktree("/r/.git/worktrees/w/HEAD"),
            Looked::REFS.union(Looked::STATE)
        );
        assert_eq!(linked_worktree("/r/.git/worktrees/w/index"), Looked::INDEX);
        assert_eq!(linked_worktree("/r/.git/refs/heads/main"), Looked::REFS);
        assert_eq!(linked_worktree("/r/.git/config"), Looked::CONFIG);
        // The main work tree's own HEAD and index aren't this one's.
        assert_eq!(linked_worktree("/r/.git/HEAD"), Looked::NONE);
        assert_eq!(linked_worktree("/r/.git/index"), Looked::NONE);
    }
}
