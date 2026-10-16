//! What a git command printed.

use std::process::ExitStatus;

use bstr::{BStr, ByteSlice as _};

/// A finished git command: how it exited and what it printed.
#[derive(Debug)]
pub struct Completed {
    /// How git exited.
    pub status: ExitStatus,
    /// Everything git printed to standard output.
    pub stdout: Vec<u8>,
    /// The end of what git printed to standard error.
    pub stderr: Stderr,
}

/// The end of what a git command printed to standard error: at most [`Stderr::LIMIT`] bytes.
///
/// Git's diagnostics come last, so the tail is what matters; keeping only it bounds the memory
/// a chatty command can take.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stderr {
    bytes: Vec<u8>,
    truncated: bool,
}

impl Stderr {
    /// How much of standard error is kept.
    pub const LIMIT: usize = 64 * 1024;

    /// The bytes kept, as git printed them.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Whether git printed more than [`Self::LIMIT`] bytes, of which only the last are kept.
    #[must_use]
    pub const fn was_truncated(&self) -> bool {
        self.truncated
    }

    /// What a person should read: git's `fatal:` and `error:` lines without their prefixes, or
    /// else the last line it printed.
    #[must_use]
    pub fn message(&self) -> String {
        let mut message = String::new();
        for line in self.bytes.lines() {
            let reason = line
                .strip_prefix(b"fatal: ")
                .or_else(|| line.strip_prefix(b"error: "));
            if let Some(reason) = reason {
                if !message.is_empty() {
                    message.push('\n');
                }
                message.push_str(&reason.trim_ascii_end().to_str_lossy());
            }
        }
        if message.is_empty()
            && let Some(last) = self
                .bytes
                .lines()
                .rev()
                .find(|line| !line.trim_ascii().is_empty())
        {
            message.push_str(&last.trim_ascii().to_str_lossy());
        }
        message
    }

    /// The lock file git couldn't create because another git process holds it, as in
    /// `Unable to create '/repo/.git/index.lock': File exists.`
    #[must_use]
    pub fn held_lock(&self) -> Option<&BStr> {
        const BEFORE: &[u8] = b"Unable to create '";
        const AFTER: &[u8] = b".lock': File exists";
        let start = self.bytes.find(BEFORE)?.checked_add(BEFORE.len())?;
        let rest = self.bytes.get(start..)?;
        let end = rest.find(AFTER)?.checked_add(b".lock".len())?;
        rest.get(..end).map(BStr::new)
    }
}

/// Collects the tail of a stream into a [`Stderr`].
#[derive(Debug, Default)]
pub(crate) struct TailBuffer {
    bytes: Vec<u8>,
    truncated: bool,
}

impl TailBuffer {
    /// Appends `chunk`, letting the oldest bytes go once there are well over the limit (so the
    /// buffer is compacted rarely rather than on every chunk).
    pub(crate) fn push(&mut self, chunk: &[u8]) {
        self.bytes.extend_from_slice(chunk);
        if self.bytes.len() > Stderr::LIMIT.saturating_mul(2) {
            self.keep_limit();
        }
    }

    /// The tail as kept.
    pub(crate) fn finish(mut self) -> Stderr {
        self.keep_limit();
        Stderr {
            bytes: self.bytes,
            truncated: self.truncated,
        }
    }

    fn keep_limit(&mut self) {
        let excess = self.bytes.len().saturating_sub(Stderr::LIMIT);
        if excess > 0 {
            self.bytes.drain(..excess);
            self.truncated = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stderr(text: &str) -> Stderr {
        let mut tail = TailBuffer::default();
        tail.push(text.as_bytes());
        tail.finish()
    }

    #[test]
    fn the_message_is_gits_fatal_and_error_lines() {
        let stderr = stderr(
            "hint: something helpful\nerror: first problem\nfatal: second problem  \nhint: more\n",
        );
        assert_eq!(stderr.message(), "first problem\nsecond problem");
    }

    #[test]
    fn without_a_fatal_line_the_message_is_the_last_line() {
        assert_eq!(
            stderr("warning: one\nsomething else\n\n").message(),
            "something else"
        );
        assert_eq!(stderr("").message(), "");
    }

    #[test]
    fn invalid_utf8_reads_lossily() {
        let mut tail = TailBuffer::default();
        tail.push(b"fatal: bad \xff path\n");
        assert_eq!(tail.finish().message(), "bad \u{fffd} path");
    }

    #[test]
    fn finds_the_lock_another_git_holds() {
        let index = stderr(
            "fatal: Unable to create '/repo/.git/index.lock': File exists.\n\n\
             Another git process seems to be running in this repository\n",
        );
        assert_eq!(index.held_lock(), Some(BStr::new("/repo/.git/index.lock")));

        let reference = stderr(
            "error: cannot lock ref 'refs/heads/main': Unable to create \
             '/repo/.git/refs/heads/main.lock': File exists.\n",
        );
        assert_eq!(
            reference.held_lock(),
            Some(BStr::new("/repo/.git/refs/heads/main.lock"))
        );
    }

    #[test]
    fn other_failures_hold_no_lock() {
        assert_eq!(Stderr::default().held_lock(), None);
        let denied = stderr("fatal: Unable to create '/repo/x.lock': Permission denied\n");
        assert_eq!(denied.held_lock(), None);
    }

    #[test]
    fn keeps_only_the_tail() {
        let mut tail = TailBuffer::default();
        let chunk = vec![b'a'; Stderr::LIMIT];
        tail.push(&chunk);
        tail.push(&chunk);
        tail.push(&chunk);
        tail.push(b"fatal: the end\n");
        let stderr = tail.finish();
        assert!(stderr.was_truncated());
        assert_eq!(stderr.as_bytes().len(), Stderr::LIMIT);
        assert!(stderr.as_bytes().ends_with(b"fatal: the end\n"));
    }

    #[test]
    fn short_output_is_kept_whole() {
        let stderr = stderr("fatal: short\n");
        assert!(!stderr.was_truncated());
        assert_eq!(stderr.as_bytes(), b"fatal: short\n");
    }
}
