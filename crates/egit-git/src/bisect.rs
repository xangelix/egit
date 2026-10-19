//! Bisecting: finding the commit that brought a change in, by marking commits on either side of
//! it while git checks out ones between (`git bisect`).
//!
//! [`bisect_mark`] marks a commit (starting a bisection first, when none is under way);
//! [`bisection`] reads where one stands from git's own records: its terms (`git bisect terms`)
//! and the refs it keeps (`refs/bisect/`), and from them how many commits could still be the
//! first bad one, or which it is. Git's messages, which name the terms and change between
//! versions, aren't read. A bisection ends with `abort_operation`'s `git bisect reset`.

use bstr::{BString, ByteSlice as _};

use crate::cancel::CancelToken;
use crate::error::GitError;
use crate::git::Git;
use crate::object_id::ObjectId;
use crate::repo::{Operation, Repository};

/// The most candidates listed one by one, to tell whether the first bad commit is found: more
/// are only counted.
const LISTED: u64 = 1024;

/// What a commit is marked as.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BisectMark {
    /// It has the change looked for (git's `bad`, or the bisection's own word for it).
    Bad,
    /// It hasn't (`good`, or the bisection's word).
    Good,
    /// It can't be tested: another near it is checked out instead.
    Skip,
}

/// The words a bisection uses for its two sides: `bad` and `good`, or its own (`new` and
/// `old`, say, when looking for a fix).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BisectTerms {
    /// The side with the change.
    pub bad: String,
    /// The side without it.
    pub good: String,
}

impl Default for BisectTerms {
    fn default() -> Self {
        Self {
            bad: "bad".to_owned(),
            good: "good".to_owned(),
        }
    }
}

/// Where a bisection stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bisection {
    /// Its words for the two sides.
    pub terms: BisectTerms,
    /// The commit marked bad (the newest that is: git keeps one).
    pub bad: Option<ObjectId>,
    /// The commits marked good.
    pub good: Vec<ObjectId>,
    /// The commits skipped.
    pub skipped: Vec<ObjectId>,
    /// What is left.
    pub left: BisectLeft,
}

/// What is left of a bisection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BisectLeft {
    /// A commit to mark bad.
    NeedsBad,
    /// A commit to mark good.
    NeedsGood,
    /// So many commits could still be the first bad one (the one checked out is among them, to
    /// test next).
    Testing {
        /// How many (skipped ones too).
        candidates: u64,
    },
    /// The first bad commit, found.
    Found(ObjectId),
    /// The first bad commit is one of these: the others near it were skipped.
    Among(Vec<ObjectId>),
}

impl BisectLeft {
    /// About how many more commits to test: each halves the candidates (the bad one among
    /// them is known already).
    #[must_use]
    pub const fn steps(&self) -> u32 {
        match self {
            Self::Testing { candidates } => match candidates.checked_ilog2() {
                Some(steps) => steps,
                None => 0,
            },
            _ => 0,
        }
    }
}

/// Where the bisection under way stands, or `None` when there is none.
///
/// # Errors
///
/// When git fails or prints what isn't a ref, or the repository's state can't be read.
pub fn bisection(git: &Git, repository: &Repository) -> Result<Option<Bisection>, GitError> {
    let operations = repository
        .operations(git)
        .map_err(|error| GitError::unexpected("bisect", error.to_string()))?;
    if !operations.contains(&Operation::Bisect) {
        return Ok(None);
    }
    let terms = terms(git, repository)?;
    let listed = git
        .read("for-each-ref")
        .args(["--format=%(refname)%00%(objectname)", "refs/bisect/"])
        .current_dir(repository.work_tree())
        .output()?;
    let (mut bad, mut good, mut skipped) = (None, Vec::new(), Vec::new());
    let bad_ref = format!("refs/bisect/{}", terms.bad);
    let good_prefix = format!("refs/bisect/{}-", terms.good);
    for line in listed.stdout.lines().filter(|line| !line.is_empty()) {
        let malformed = || GitError::unexpected("for-each-ref", "a ref");
        let (name, id) = line.split_once_str(b"\0").ok_or_else(malformed)?;
        let id = ObjectId::from_hex(id).ok_or_else(malformed)?;
        if name == bad_ref.as_bytes() {
            bad = Some(id);
        } else if name.starts_with(good_prefix.as_bytes()) {
            good.push(id);
        } else if name.starts_with(b"refs/bisect/skip-") {
            skipped.push(id);
        }
    }
    let left = match (bad, good.is_empty()) {
        (None, _) => BisectLeft::NeedsBad,
        (Some(_), true) => BisectLeft::NeedsGood,
        (Some(bad), false) => left(git, repository, bad, &good, &skipped)?,
    };
    Ok(Some(Bisection {
        terms,
        bad,
        good,
        skipped,
        left,
    }))
}

/// The bisection's words for its two sides (`git bisect terms`).
fn terms(git: &Git, repository: &Repository) -> Result<BisectTerms, GitError> {
    let term = |which: &str| -> Result<String, GitError> {
        let said = git
            .read("bisect")
            .args(["terms", which])
            .current_dir(repository.work_tree())
            .output()?;
        let term = said.stdout.trim_ascii().to_str_lossy().into_owned();
        if term.is_empty() {
            return Err(GitError::unexpected("bisect", "a term"));
        }
        Ok(term)
    };
    Ok(BisectTerms {
        bad: term("--term-bad")?,
        good: term("--term-good")?,
    })
}

/// What is left between `bad` and the `good` commits: the commits `bad` reaches that none of
/// them does, `skipped` ones aside.
fn left(
    git: &Git,
    repository: &Repository,
    bad: ObjectId,
    good: &[ObjectId],
    skipped: &[ObjectId],
) -> Result<BisectLeft, GitError> {
    // The good commits on standard input, however many were marked.
    let mut input = format!("{bad}\n");
    for commit in good {
        input.push('^');
        input.push_str(&commit.to_string());
        input.push('\n');
    }
    let listing = |count: bool| {
        let mut command = git.read("rev-list").arg("--stdin");
        if count {
            command = command.arg("--count");
        }
        command
            .stdin(input.clone().into_bytes())
            .current_dir(repository.work_tree())
            .output()
    };
    let counted = listing(true)?;
    let candidates: u64 = counted
        .stdout
        .trim_ascii()
        .to_str()
        .ok()
        .and_then(|count| count.parse().ok())
        .ok_or_else(|| GitError::unexpected("rev-list", "a count"))?;
    if candidates > LISTED {
        return Ok(BisectLeft::Testing { candidates });
    }
    let listed = listing(false)?;
    let all: Vec<ObjectId> = listed
        .stdout
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            ObjectId::from_hex(line).ok_or_else(|| GitError::unexpected("rev-list", "a commit"))
        })
        .collect::<Result<_, _>>()?;
    let untested: Vec<&ObjectId> = all.iter().filter(|id| !skipped.contains(id)).collect();
    if untested.as_slice() != [&bad] {
        return Ok(BisectLeft::Testing { candidates });
    }
    let among: Vec<ObjectId> = all
        .iter()
        .filter(|id| **id == bad || skipped.contains(id))
        .copied()
        .collect();
    Ok(if among.len() > 1 {
        BisectLeft::Among(among)
    } else {
        BisectLeft::Found(bad)
    })
}

/// Marks `commit` as `mark` says, in the bisection under way's terms.
///
/// Starts one first when none is (`git bisect start`). Once a bad and a good commit are
/// marked, git checks out one between them to test, until the first bad one is found.
///
/// # Errors
///
/// When git refuses (changes in the work tree in the way of the commit to test, say) or is
/// cancelled.
pub fn bisect_mark(
    git: &Git,
    repository: &Repository,
    mark: BisectMark,
    commit: ObjectId,
    cancel: &CancelToken,
) -> Result<(), GitError> {
    let bisecting = repository
        .operations(git)
        .map_err(|error| GitError::unexpected("bisect", error.to_string()))?
        .contains(&Operation::Bisect);
    let terms = if bisecting {
        terms(git, repository)?
    } else {
        git.write("bisect")
            .arg("start")
            .current_dir(repository.work_tree())
            .cancel_token(cancel)
            .output()?;
        BisectTerms::default()
    };
    let term: BString = match mark {
        BisectMark::Bad => terms.bad.into(),
        BisectMark::Good => terms.good.into(),
        BisectMark::Skip => "skip".into(),
    };
    let done = git
        .write("bisect")
        .arg(term.to_os_str_lossy())
        .arg(commit.to_string())
        .current_dir(repository.work_tree())
        .cancel_token(cancel)
        .output_unchecked()?;
    // Status 2: only skipped commits are left, one of which is the first bad commit
    // (`bisection` says which).
    if done.status.success() || done.status.code() == Some(2) {
        Ok(())
    } else {
        Err(GitError::failed("bisect", done.status, done.stderr))
    }
}
