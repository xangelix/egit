//! Blame: which commit each line of a file came from, streamed from `git blame --incremental`
//! as git finds it, and every commit that changed a file, following renames, streamed from
//! `git log --follow`.
//!
//! Blame's incremental format gives a run of lines (`<commit> <line there> <line here>
//! <count>`), then the commit's details the first time it appears (author, time, summary,
//! `boundary`, `previous`), then `filename`; [`blame`] hands each run over with its commit's
//! place in a table that grows as commits appear. Uncommitted lines (blaming the work tree)
//! come from a commit of zeros.

use std::ffi::OsStr;
use std::ops::ControlFlow;
use std::os::unix::ffi::OsStrExt as _;

use bstr::{BStr, BString, ByteSlice as _};

use crate::cancel::CancelToken;
use crate::error::GitError;
use crate::git::Git;
use crate::lines::Lines;
use crate::object_id::ObjectId;
use crate::repo::Repository;

/// A commit lines came from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BlameCommit {
    /// Its name; `None` for lines not committed yet (blaming the work tree).
    pub id: Option<ObjectId>,
    /// Its author's name.
    pub author: String,
    /// When it was authored (seconds since the Unix epoch).
    pub time: i64,
    /// Its subject.
    pub summary: String,
    /// Whether the blame stopped at it (the root, say): its lines may be older still.
    pub boundary: bool,
    /// The commit before it, and the file's path there: where to blame what was before.
    pub previous: Option<(ObjectId, BString)>,
}

/// A run of lines from one commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlameRun {
    /// The commit's place in the table [`blame`] hands over.
    pub commit: u32,
    /// Its first line in the file blamed (from 0).
    pub line: u32,
    /// How many lines.
    pub count: u32,
    /// Its first line in the commit's version of the file (from 0).
    pub original: u32,
}

/// Whose a line moved within the file is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Moves {
    /// The commit that moved it (git's default).
    #[default]
    Moving,
    /// The commit that wrote it, wherever it was then (`-M`). Far slower on a large file: git
    /// looks for each line's earlier place in every older version of the whole file (100,000
    /// lines rewritten by 100 commits: 80 s, against 0.9 s).
    Writing,
}

/// Blames the file at `path` as `at` has it (`None`: as the work tree has it).
///
/// Each run of lines is handed over as git finds it, with the commits found so far
/// (`commits[run.commit]` is its commit). `moves` says whose lines moved within the file are.
///
/// # Errors
///
/// When git fails (no such file there, say), is cancelled, or prints something that isn't
/// blame.
pub fn blame(
    git: &Git,
    repository: &Repository,
    at: Option<ObjectId>,
    path: &BStr,
    moves: Moves,
    cancel: &CancelToken,
    mut found: impl FnMut(&[BlameCommit], BlameRun),
) -> Result<(), GitError> {
    let mut command = git
        .read("blame")
        .arg("--incremental")
        .current_dir(repository.work_tree())
        .cancel_token(cancel);
    if moves == Moves::Writing {
        command = command.arg("-M");
    }
    if let Some(at) = at {
        command = command.arg(at.to_string());
    }
    let mut parser = BlameParser::default();
    let mut lines = Lines::default();
    let mut malformed = false;
    let mut handle = |line: &[u8]| match parser.line(line) {
        Ok(Some(run)) => {
            found(&parser.commits, run);
            ControlFlow::Continue(())
        }
        Ok(None) => ControlFlow::Continue(()),
        Err(()) => {
            malformed = true;
            ControlFlow::Break(())
        }
    };
    command
        .arg("--")
        .arg(OsStr::from_bytes(path))
        .stream(|chunk| lines.feed(chunk, &mut handle))?;
    if malformed {
        return Err(GitError::unexpected("blame", "incremental blame"));
    }
    Ok(())
}

/// Reads `blame --incremental`, a line at a time.
#[derive(Debug, Default)]
struct BlameParser {
    commits: Vec<BlameCommit>,
    /// The commits' names, sorted, with their places (to find a commit seen before).
    known: Vec<(ObjectId, u32)>,
    /// The run being read, until its `filename` line.
    open: Option<BlameRun>,
    /// Commits of zeros (uncommitted) share one place.
    uncommitted: Option<u32>,
}

impl BlameParser {
    /// Takes in a line; a run once its last line (`filename`) is read.
    fn line(&mut self, line: &[u8]) -> Result<Option<BlameRun>, ()> {
        let Some(run) = self.open else {
            self.open = Some(self.header(line)?);
            return Ok(None);
        };
        let (key, value) = line.split_once_str(b" ").unwrap_or((line, b""));
        let commit = self
            .commits
            .get_mut(usize::try_from(run.commit).map_err(drop)?)
            .ok_or(())?;
        let text = || value.to_str_lossy().into_owned();
        match key {
            b"filename" => {
                self.open = None;
                return Ok(Some(run));
            }
            b"author" => commit.author = text(),
            b"author-time" => {
                commit.time = value
                    .to_str()
                    .ok()
                    .and_then(|time| time.parse().ok())
                    .ok_or(())?;
            }
            b"summary" => commit.summary = text(),
            b"boundary" => commit.boundary = true,
            // The first commit before it that git names is where its lines' past is.
            b"previous" if commit.previous.is_none() => {
                let (id, path) = value.split_once_str(b" ").ok_or(())?;
                let id = ObjectId::from_hex(id).ok_or(())?;
                commit.previous = Some((id, BString::from(path)));
            }
            // The rest (mails, time zones, the committer) aren't shown.
            _ => {}
        }
        Ok(None)
    }

    /// A run's first line: `<commit> <line there> <line here> <count>` (from 1).
    fn header(&mut self, line: &[u8]) -> Result<BlameRun, ()> {
        let mut fields = line.split_str(b" ");
        let id = fields.next().and_then(ObjectId::from_hex).ok_or(())?;
        let mut number = || {
            fields
                .next()
                .and_then(|field| field.to_str().ok()?.parse::<u32>().ok())
                .ok_or(())
        };
        let (original, line, count) = (number()?, number()?, number()?);
        let commit = self.place(id)?;
        Ok(BlameRun {
            commit,
            line: line.checked_sub(1).ok_or(())?,
            count,
            original: original.checked_sub(1).ok_or(())?,
        })
    }

    /// The place of commit `id` in the table, added if it is new.
    fn place(&mut self, id: ObjectId) -> Result<u32, ()> {
        let uncommitted = id.as_bytes().iter().all(|&byte| byte == 0);
        if uncommitted && let Some(place) = self.uncommitted {
            return Ok(place);
        }
        let at = self.known.binary_search_by(|(known, _)| known.cmp(&id));
        if let Ok(found) = at {
            return self.known.get(found).map(|&(_, place)| place).ok_or(());
        }
        let place = u32::try_from(self.commits.len()).map_err(drop)?;
        self.commits.push(BlameCommit {
            id: (!uncommitted).then_some(id),
            ..BlameCommit::default()
        });
        if uncommitted {
            self.uncommitted = Some(place);
        } else if let Err(insert) = at {
            self.known.insert(insert, (id, place));
        }
        Ok(place)
    }
}

/// A commit that changed a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileCommit {
    /// Its name.
    pub id: ObjectId,
    /// Its first parent, if any.
    pub parent: Option<ObjectId>,
    /// Its author's name.
    pub author: String,
    /// When it was authored (seconds since the Unix epoch).
    pub time: i64,
    /// Its subject.
    pub subject: String,
    /// How it changed the file: `A`, `M`, `D`, `R` (renamed), `C`, `T` (git's letter).
    pub status: u8,
    /// The file's path in it.
    pub path: BString,
    /// Its path before, when the commit renamed or copied it.
    pub from: Option<BString>,
}

/// Every commit that changed the file at `path`, newest first, following renames (`git log
/// --follow`), each handed over as git finds it.
///
/// # Errors
///
/// When git fails, is cancelled, or prints something that isn't a commit.
pub fn file_log(
    git: &Git,
    repository: &Repository,
    path: &BStr,
    cancel: &CancelToken,
    mut found: impl FnMut(FileCommit),
) -> Result<(), GitError> {
    let mut record = Vec::new();
    let mut malformed = false;
    let mut take = |record: &[u8]| match parse_file_commit(record) {
        Some(commit) => {
            found(commit);
            true
        }
        None if record.trim_ascii().is_empty() => true,
        None => {
            malformed = true;
            false
        }
    };
    git.read("log")
        .config("log.showSignature", "false")
        .args([
            "--follow",
            "-M",
            "--name-status",
            "-z",
            "--format=%x1e%H%x1f%P%x1f%an%x1f%at%x1f%s",
        ])
        .arg("--")
        .arg(OsStr::from_bytes(path))
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .stream(|chunk| {
            // Records start with a record separator; one ends where the next starts.
            let mut rest = chunk;
            while let Some(at) = rest.find_byte(0x1e) {
                record.extend_from_slice(rest.get(..at).unwrap_or_default());
                if !take(&record) {
                    return ControlFlow::Break(());
                }
                record.clear();
                rest = rest.get(at.saturating_add(1)..).unwrap_or_default();
            }
            record.extend_from_slice(rest);
            ControlFlow::Continue(())
        })?;
    if !take(&record) || malformed {
        return Err(GitError::unexpected(
            "log",
            "a commit and how it changed the file",
        ));
    }
    Ok(())
}

/// A record of `log --name-status -z --format=%H%x1f%P%x1f%an%x1f%at%x1f%s`: the header, a
/// NUL, then the status, the path (and a rename's new path), each NUL-ended.
fn parse_file_commit(record: &[u8]) -> Option<FileCommit> {
    let (header, changes) = record.split_once_str(b"\0")?;
    let mut fields = header.splitn_str(5, b"\x1f");
    let id = ObjectId::from_hex(fields.next()?)?;
    let parent = fields
        .next()?
        .split_str(b" ")
        .next()
        .and_then(ObjectId::from_hex);
    let author = fields.next()?.to_str_lossy().into_owned();
    let time = fields.next()?.to_str().ok()?.parse().ok()?;
    let subject = fields.next()?.to_str_lossy().into_owned();
    let mut changes = changes
        .trim_start_with(|c| c == '\n')
        .split_str(b"\0")
        .filter(|field| !field.is_empty());
    let status = *changes.next()?.first()?;
    let first = BString::from(changes.next()?);
    let (path, from) = match (status, changes.next()) {
        (b'R' | b'C', Some(new)) => (BString::from(new), Some(first)),
        _ => (first, None),
    };
    Some(FileCommit {
        id,
        parent,
        author,
        time,
        subject,
        status,
        path,
        from,
    })
}

#[cfg(test)]
// The relaxations `clippy.toml` gives `#[test]` functions, for this module's helpers.
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn runs(output: &str) -> (Vec<BlameCommit>, Vec<BlameRun>) {
        let mut parser = BlameParser::default();
        let mut runs = Vec::new();
        for line in output.lines() {
            if let Some(run) = parser.line(line.as_bytes()).unwrap() {
                runs.push(run);
            }
        }
        (parser.commits, runs)
    }

    #[test]
    fn reads_runs_and_their_commits_once() {
        let a = "a".repeat(40);
        let p = "b".repeat(40);
        let zero = "0".repeat(40);
        let output = format!(
            "{a} 2 2 1\nauthor Ada\nauthor-mail <a@x>\nauthor-time 100\nauthor-tz +0000\n\
             summary Two\nprevious {p} old/f\nfilename f\n\
             {a} 4 5 2\nprevious {p} old/f\nfilename f\n\
             {zero} 6 7 1\nauthor Not Committed Yet\nauthor-time 200\nsummary Version\nfilename f\n\
             {p} 1 1 1\nauthor Bea\nauthor-time 50\nsummary One\nboundary\nfilename f\n"
        );
        let (commits, runs) = runs(&output);
        assert_eq!(commits.len(), 3);
        assert_eq!(commits[0].author, "Ada");
        assert_eq!(commits[0].time, 100);
        assert_eq!(commits[0].summary, "Two");
        assert_eq!(
            commits[0].previous,
            Some((ObjectId::from_hex(p.as_bytes()).unwrap(), "old/f".into()))
        );
        assert_eq!(commits[1].id, None);
        assert!(commits[2].boundary);
        assert_eq!(
            runs,
            [
                BlameRun {
                    commit: 0,
                    line: 1,
                    count: 1,
                    original: 1
                },
                BlameRun {
                    commit: 0,
                    line: 4,
                    count: 2,
                    original: 3
                },
                BlameRun {
                    commit: 1,
                    line: 6,
                    count: 1,
                    original: 5
                },
                BlameRun {
                    commit: 2,
                    line: 0,
                    count: 1,
                    original: 0
                },
            ]
        );
    }

    #[test]
    fn reads_a_commit_and_how_it_changed_the_file() {
        let id = "c".repeat(40);
        let parent = "d".repeat(40);
        let renamed = format!("{id}\x1f{parent}\x1fAda\x1f300\x1fMove it\0\nR100\0old\0new\0");
        let commit = parse_file_commit(renamed.as_bytes()).unwrap();
        assert_eq!(commit.subject, "Move it");
        assert_eq!(commit.parent, ObjectId::from_hex(parent.as_bytes()));
        assert_eq!((commit.status, commit.path.as_slice()), (b'R', &b"new"[..]));
        assert_eq!(commit.from, Some("old".into()));
        let added = format!("{id}\x1f\x1fAda\x1f300\x1fAdd it\0\nA\0f\0");
        let commit = parse_file_commit(added.as_bytes()).unwrap();
        assert_eq!((commit.parent, commit.status), (None, b'A'));
        assert_eq!(parse_file_commit(b"not a commit"), None);
    }
}
