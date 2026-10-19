//! Following what changes in a repository on another computer, where egit can't watch its
//! files: a session of its own there runs a small `sh` loop that looks every few seconds at
//! what a watch here would see change (refs, an operation's state, configuration, the index)
//! and says what did, a word a line (and a `.` after each look, so it ends soon after nobody
//! reads it any more).
//!
//! Refs are looked at through their folders' times (git writes a ref by renaming a lock file
//! over it, which changes its folder), `packed-refs`, and `HEAD`'s text, so thousands of refs
//! cost a few folders; nothing is read from the work tree (its status is read every few
//! seconds instead).

use std::ffi::OsStr;
use std::io::{self, BufRead as _, BufReader};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::git::Git;
use crate::host::{self, GitStdout, Remote};
use crate::process::Detached;
use crate::repo::Repository;

/// The loop there: `$1` the git directory, `$2` the common one, `$3` the seconds between looks.
const LOOP: &str = r#"
gitdir=$1 common=$2 every=$3
config_home=${XDG_CONFIG_HOME:-$HOME/.config}
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

/// What changed there, as the loop says it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Looked {
    /// `HEAD`, a branch, a tag, or a remote-tracking branch.
    Refs,
    /// An operation's state (a merge or rebase starting or ending).
    State,
    /// Configuration.
    Config,
    /// The index.
    Index,
}

/// A loop looking at a repository there, stopped when dropped.
#[derive(Debug)]
pub struct Looking {
    process: Detached,
    ended: Arc<AtomicBool>,
}

impl Looking {
    /// Whether it stopped looking (the connection was lost, or the loop couldn't run).
    #[must_use]
    pub fn has_ended(&self) -> bool {
        self.ended.load(Ordering::Acquire)
    }
}

impl Drop for Looking {
    fn drop(&mut self) {
        // The session closes; the loop there ends at its next look.
        self.process.stop(Duration::ZERO);
    }
}

/// Starts looking at `repository` there every `every`, handing what changed to `changed` on a
/// thread of its own (nothing for the first look, which only notes how things are).
///
/// # Errors
///
/// When the session can't be started, or `git` runs here (where a watch does better).
pub fn look_there(
    git: &Git,
    repository: &Repository,
    every: Duration,
    mut changed: impl FnMut(Looked) + Send + 'static,
) -> io::Result<Looking> {
    let Some(ssh) = git.host().ssh() else {
        return Err(io::Error::other("the repository is on this computer"));
    };
    let seconds = every.as_secs().max(1).to_string();
    let environment = git.remote_environment();
    let script = host::script(&Remote {
        program: OsStr::new("sh"),
        args: vec![
            OsStr::new("-c"),
            OsStr::new(LOOP),
            OsStr::new("sh"),
            repository.git_dir().as_os_str(),
            repository.common_dir().as_os_str(),
            OsStr::new(&seconds),
        ],
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
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut process = Detached::spawn(&mut command)?;
    let stdout = process
        .child_mut()
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("ssh's output wasn't set up"))?;
    let ended = Arc::new(AtomicBool::new(false));
    let marking = Arc::clone(&ended);
    let host = git.host().clone();
    std::thread::Builder::new()
        .name("looking there".to_owned())
        .spawn(move || {
            let mut lines = BufReader::new(GitStdout::new(stdout, &host));
            let mut line = Vec::new();
            let mut first = true;
            loop {
                line.clear();
                match lines.read_until(b'\n', &mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                let looked = match line.trim_ascii() {
                    b"." => {
                        first = false;
                        continue;
                    }
                    b"refs" => Looked::Refs,
                    b"state" => Looked::State,
                    b"config" => Looked::Config,
                    b"index" => Looked::Index,
                    _ => continue,
                };
                if !first {
                    changed(looked);
                }
            }
            marking.store(true, Ordering::Release);
        })?;
    Ok(Looking { process, ended })
}
