//! A repository's submodules: each one `.gitmodules` names and the index records (its name,
//! path, address, the commit recorded, and whether `submodule init` set it up here), and
//! submodules brought to the commits recorded (`submodule update --init --recursive`).

use bstr::{BStr, BString, ByteSlice as _};

use crate::cancel::CancelToken;
use crate::error::GitError;
use crate::git::Git;
use crate::object_id::ObjectId;
use crate::progress::Progress;
use crate::repo::Repository;

/// A submodule of the repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Submodule {
    /// Its name in `.gitmodules` (often its path).
    pub name: BString,
    /// Where it is, from the work tree's top.
    pub path: BString,
    /// Where it comes from, as `.gitmodules` says.
    pub url: Option<BString>,
    /// The commit the index records for it.
    pub recorded: ObjectId,
    /// Whether it is set up here (`submodule.<name>.url` in the repository's configuration, as
    /// `submodule init` sets it).
    pub initialized: bool,
}

/// The submodules `.gitmodules` names that the index records, by path.
///
/// # Errors
///
/// When git fails or prints something unexpected.
pub fn submodules(git: &Git, repository: &Repository) -> Result<Vec<Submodule>, GitError> {
    let declared = config_entries(git, repository, &["--file", ".gitmodules"], "^submodule\\.")?;
    // Each name's path and address, in the order `.gitmodules` gives them.
    let mut named: Vec<Declared> = Vec::new();
    for (key, value) in declared {
        let Some((name, variable)) = key
            .strip_prefix(b"submodule.")
            .and_then(|rest| rest.rsplit_once_str(b"."))
        else {
            continue;
        };
        let entry = if let Some(at) = named.iter().position(|known| known.name == name) {
            named.get_mut(at)
        } else {
            named.push(Declared {
                name: BString::from(name),
                path: None,
                url: None,
            });
            named.last_mut()
        };
        let Some(entry) = entry else {
            continue;
        };
        match variable {
            b"path" => entry.path = Some(value),
            b"url" => entry.url = Some(value),
            _ => {}
        }
    }
    let paths: Vec<&BString> = named
        .iter()
        .filter_map(|known| known.path.as_ref())
        .collect();
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let recorded = gitlinks(git, repository, &paths)?;
    let initialized: Vec<BString> =
        config_entries(git, repository, &["--local"], "^submodule\\..*\\.url$")?
            .into_iter()
            .filter_map(|(key, _)| {
                key.strip_prefix(b"submodule.")
                    .and_then(|rest| rest.strip_suffix(b".url"))
                    .map(BString::from)
            })
            .collect();
    let mut found: Vec<Submodule> = named
        .into_iter()
        .filter_map(|Declared { name, path, url }| {
            let path = path?;
            let (_, commit) = recorded.iter().find(|(known, _)| *known == path)?;
            Some(Submodule {
                initialized: initialized.contains(&name),
                recorded: *commit,
                name,
                path,
                url,
            })
        })
        .collect();
    found.sort_unstable_by(|a, b| a.path.cmp(&b.path));
    Ok(found)
}

/// A submodule as `.gitmodules` declares it.
struct Declared {
    name: BString,
    path: Option<BString>,
    url: Option<BString>,
}

/// The keys matching `pattern` and their values in the configuration `place` names (`-z
/// --get-regexp`: `key\nvalue\0` each); none when nothing matches or the file is missing.
fn config_entries(
    git: &Git,
    repository: &Repository,
    place: &[&str],
    pattern: &str,
) -> Result<Vec<(BString, BString)>, GitError> {
    let listed = git
        .read("config")
        .args(place)
        .args(["-z", "--get-regexp", pattern])
        .current_dir(repository.work_tree())
        .output_unchecked()?;
    match listed.status.code() {
        Some(0) => {}
        // Nothing matched, or no such file.
        Some(1) => return Ok(Vec::new()),
        _ => return Err(GitError::failed("config", listed.status, listed.stderr)),
    }
    Ok(listed
        .stdout
        .split_str(b"\0")
        .filter(|record| !record.is_empty())
        .map(|record| {
            let (key, value) = record.split_once_str(b"\n").unwrap_or((record, b""));
            (BString::from(key), BString::from(value))
        })
        .collect())
}

/// The commits the index records for those of `paths` that are submodules (`ls-files -s -z`,
/// literal paths), the first stage of each when in conflict.
fn gitlinks(
    git: &Git,
    repository: &Repository,
    paths: &[&BString],
) -> Result<Vec<(BString, ObjectId)>, GitError> {
    let listed = git
        .read("ls-files")
        .args(["-s", "-z", "--"])
        .args(paths.iter().map(|path| path.to_os_str_lossy()))
        .env("GIT_LITERAL_PATHSPECS", "1")
        .current_dir(repository.work_tree())
        .output()?;
    let mut found: Vec<(BString, ObjectId)> = Vec::new();
    for record in listed
        .stdout
        .split_str(b"\0")
        .filter(|record| !record.is_empty())
    {
        let malformed = || GitError::unexpected("ls-files", "an index entry");
        let (meta, path) = record.split_once_str(b"\t").ok_or_else(malformed)?;
        let mut fields = meta.split_str(b" ");
        let (Some(mode), Some(id)) = (fields.next(), fields.next()) else {
            return Err(malformed());
        };
        if mode != b"160000" || found.iter().any(|(known, _)| known == path) {
            continue;
        }
        found.push((
            BString::from(path),
            ObjectId::from_hex(id).ok_or_else(malformed)?,
        ));
    }
    Ok(found)
}

/// Brings the submodules at `paths` (all of them when empty) to the commits recorded, cloning
/// and setting up those not here yet, and theirs in turn (`submodule update --init
/// --recursive`).
///
/// # Errors
///
/// When git fails (a submodule can't be fetched, has changes in the way) or is cancelled.
pub fn update_submodules(
    git: &Git,
    repository: &Repository,
    paths: &[&BStr],
    progress: &Progress,
    cancel: &CancelToken,
) -> Result<(), GitError> {
    git.write("submodule")
        .args(["update", "--init", "--recursive", "--progress", "--"])
        .args(paths.iter().map(|path| path.to_os_str_lossy()))
        .env("GIT_LITERAL_PATHSPECS", "1")
        .prompt_context("Updating submodules")
        .current_dir(repository.work_tree())
        .progress(progress)
        .cancel_token(cancel)
        .output()
        .map(drop)
}
