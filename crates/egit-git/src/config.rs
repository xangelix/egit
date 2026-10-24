//! Git's configuration, as git itself resolves it, and changing it through git.
//!
//! A [`ConfigSnapshot`] is every entry in effect in one place (outside any repository, or inside
//! one, where conditional includes such as `includeIf "gitdir:…"` apply), each with the scope and
//! file it comes from, in the order git reads them: a later entry overrides an earlier one.
//! The output of `git config --list` is kept in one buffer that the entries point into.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use bstr::{BStr, ByteSlice as _};

use crate::error::GitError;
use crate::git::Git;
use crate::span::{Span, SpanSplitter, as_u32, span_bytes};

/// Which configuration a snapshot reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigPlace<'a> {
    /// The user's: system and global, outside any repository.
    User,
    /// What applies inside a repository: the user's, the repository's, and the work tree's.
    Repository(&'a Path),
}

/// Which configuration a change is written to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigTarget<'a> {
    /// The user's global configuration (`git config --global`).
    Global,
    /// One repository's own configuration (`git config --local`, run in its work tree).
    Repository(&'a Path),
}

/// The level a configuration entry comes from, from broadest to narrowest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ConfigScope {
    /// The system's configuration (`/etc/gitconfig`).
    System,
    /// The user's (`~/.gitconfig`, `$XDG_CONFIG_HOME/git/config`, and what they include).
    Global,
    /// The repository's (`.git/config`).
    Local,
    /// The work tree's (`config.worktree`).
    Worktree,
    /// Given on the command line or in the environment.
    Command,
    /// A scope git didn't name.
    Unknown,
}

impl ConfigScope {
    const fn from_name(name: &[u8]) -> Self {
        match name {
            b"system" => Self::System,
            b"global" => Self::Global,
            b"local" => Self::Local,
            b"worktree" => Self::Worktree,
            b"command" => Self::Command,
            _ => Self::Unknown,
        }
    }

    /// How git names it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Global => "global",
            Self::Local => "local",
            Self::Worktree => "worktree",
            Self::Command => "command",
            Self::Unknown => "unknown",
        }
    }
}

/// Every configuration entry in effect in one place.
#[derive(Clone, Debug)]
pub struct ConfigSnapshot {
    buffer: Box<[u8]>,
    entries: Box<[RawEntry]>,
    /// Where git ran: relative origins (`file:.git/config`) are relative to it.
    base: PathBuf,
}

#[derive(Clone, Copy, Debug)]
struct RawEntry {
    scope: ConfigScope,
    origin: Span,
    key: Span,
    value: Option<Span>,
}

/// One configuration entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConfigEntry<'a> {
    /// The level it comes from.
    pub scope: ConfigScope,
    /// Where git read it, as git names it (`file:/home/me/.gitconfig`, `blob:…`).
    pub origin: &'a BStr,
    /// The key, with section and name in lower case (`user.name`, `includeif.gitdir:~/w/.path`).
    pub key: &'a BStr,
    /// The value; `None` for a key written without `=`, which reads as boolean true.
    pub value: Option<&'a BStr>,
    base: &'a Path,
}

impl<'a> ConfigEntry<'a> {
    /// The file it was read from, when it was read from one.
    #[must_use]
    pub fn file(self) -> Option<PathBuf> {
        let path = self.origin.strip_prefix(b"file:")?.to_path_lossy();
        Some(self.base.join(path))
    }

    /// The value, with a key written without `=` reading as empty.
    #[must_use]
    pub fn text(self) -> &'a BStr {
        self.value.unwrap_or_else(|| BStr::new(b""))
    }

    /// The value as a boolean, read the way git reads one (`true`, `yes`, `on`, a non-zero
    /// number, or no value at all are true; `false`, `no`, `off`, `0`, and empty are false).
    #[must_use]
    pub fn bool(self) -> Option<bool> {
        self.value.map_or(Some(true), |value| parse_bool(value))
    }
}

impl ConfigSnapshot {
    /// Reads the configuration in effect at `place`.
    ///
    /// # Errors
    ///
    /// When git fails or prints something that isn't a configuration listing.
    pub async fn read(git: &Git, place: ConfigPlace<'_>) -> Result<Self, GitError> {
        let base = match place {
            // Anywhere but egit's own working directory, which might be inside a repository.
            ConfigPlace::User => crate::git::outside_repositories(),
            ConfigPlace::Repository(work_tree) => work_tree,
        };
        let listed = git
            .read("config")
            .args(["--list", "--show-origin", "--show-scope", "--null"])
            .current_dir(base)
            .output()
            .await?;
        Self::parse(listed.stdout, base.to_owned())
            .map_err(|detail| GitError::unexpected("config", detail))
    }

    /// Parses `git config --list --show-origin --show-scope --null`.
    ///
    /// Entries in the command scope are left out: egit's own `-c` settings show up there, and
    /// it removes any the user's environment would add.
    fn parse(output: Vec<u8>, base: PathBuf) -> Result<Self, &'static str> {
        if u32::try_from(output.len()).is_err() {
            return Err("the listing is larger than 4 GiB");
        }
        let mut entries = Vec::new();
        let mut fields = SpanSplitter::new(&output);
        while let Some(scope) = fields.next() {
            let origin = fields.next().ok_or("an entry without an origin")?;
            let key_value = fields.next().ok_or("an entry without a key")?;
            let scope = ConfigScope::from_name(span_bytes(&output, scope));
            if scope == ConfigScope::Command {
                continue;
            }
            let pair = span_bytes(&output, key_value);
            let (key, value) = pair.find_byte(b'\n').map_or((key_value, None), |newline| {
                let split = key_value.start.saturating_add(as_u32(newline));
                let key = Span {
                    start: key_value.start,
                    end: split,
                };
                let value = Span {
                    start: split.saturating_add(1),
                    end: key_value.end,
                };
                (key, Some(value))
            });
            entries.push(RawEntry {
                scope,
                origin,
                key,
                value,
            });
        }
        Ok(Self {
            buffer: output.into_boxed_slice(),
            entries: entries.into_boxed_slice(),
            base,
        })
    }

    /// How many entries it holds.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether it holds none.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The entry at `index` in the order git reads them.
    #[must_use]
    pub fn entry(&self, index: usize) -> Option<ConfigEntry<'_>> {
        self.entries.get(index).map(|raw| self.entry_at(raw))
    }

    /// Every entry, in the order git reads them.
    pub fn entries(&self) -> impl DoubleEndedIterator<Item = ConfigEntry<'_>> + ExactSizeIterator {
        self.entries.iter().map(|raw| self.entry_at(raw))
    }

    /// The entry in effect for `key` (the last one git reads), such as `user.name` or
    /// `branch.main.remote`. The section and name match in any case; the subsection exactly.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<ConfigEntry<'_>> {
        self.get_all(key).next_back()
    }

    /// Every entry for `key`, in order (for keys with several values).
    pub fn get_all<'s>(
        &'s self,
        key: &str,
    ) -> impl DoubleEndedIterator<Item = ConfigEntry<'s>> + use<'s> {
        let wanted = canonical_key(key);
        self.entries()
            .filter(move |entry| entry.key == wanted.as_bytes())
    }

    /// Whether the repository is a partial clone, its objects possibly missing and fetched on
    /// demand: a promisor remote, or a filter a clone was made with.
    #[must_use]
    pub fn is_partial_clone(&self) -> bool {
        self.get("extensions.partialClone").is_some()
            || self.entries().any(|entry| {
                let key = entry.key.as_bytes();
                key.starts_with(b"remote.")
                    && (key.ends_with(b".partialclonefilter")
                        || (key.ends_with(b".promisor") && entry.bool() == Some(true)))
            })
    }

    /// Whether every commit's trees are here: it isn't a partial clone, or its clones' filters
    /// each leave out only blobs (`blob:none`, `blob:limit=…`), so walking trees fetches
    /// nothing.
    #[must_use]
    pub fn has_every_tree(&self) -> bool {
        if !self.is_partial_clone() {
            return true;
        }
        let mut filters = self.entries().filter(|entry| {
            let key = entry.key.as_bytes();
            key.starts_with(b"remote.") && key.ends_with(b".partialclonefilter")
        });
        let mut any = false;
        let blobs_only = filters.all(|entry| {
            any = true;
            entry
                .value
                .is_some_and(|filter| filter.starts_with(b"blob:"))
        });
        any && blobs_only
    }

    fn entry_at<'s>(&'s self, raw: &RawEntry) -> ConfigEntry<'s> {
        ConfigEntry {
            scope: raw.scope,
            origin: BStr::new(span_bytes(&self.buffer, raw.origin)),
            key: BStr::new(span_bytes(&self.buffer, raw.key)),
            value: raw
                .value
                .map(|span| BStr::new(span_bytes(&self.buffer, span))),
            base: &self.base,
        }
    }
}

/// Sets `key` to `value` in `target`, replacing every value it had there.
///
/// # Errors
///
/// When git refuses the key or can't write the file.
pub async fn set_config(
    git: &Git,
    target: ConfigTarget<'_>,
    key: &str,
    value: impl AsRef<OsStr>,
) -> Result<(), GitError> {
    config_command(git, target)
        .args(["--replace-all", key])
        .arg(value)
        .output()
        .await
        .map(drop)
}

/// Adds `value` to `key`'s values in `target` (a key that may have several), unless it has it.
///
/// # Errors
///
/// When git refuses the key or can't write the file.
pub async fn add_config_value(
    git: &Git,
    target: ConfigTarget<'_>,
    key: &str,
    value: &str,
) -> Result<(), GitError> {
    let has = config_command(git, target)
        .args(["--fixed-value", "--get", key, value])
        .output_unchecked()
        .await?;
    if has.status.success() {
        return Ok(());
    }
    config_command(git, target)
        .args(["--add", key, value])
        .output()
        .await
        .map(drop)
}

/// Removes `value` (exactly) from `key`'s values in `target`, leaving its others. Removing a
/// value that isn't there succeeds.
///
/// # Errors
///
/// When git refuses the key or can't write the file.
pub async fn unset_config_value(
    git: &Git,
    target: ConfigTarget<'_>,
    key: &str,
    value: &str,
) -> Result<(), GitError> {
    let completed = config_command(git, target)
        .args(["--fixed-value", "--unset-all", key, value])
        .output_unchecked()
        .await?;
    // Exit code 5: there was nothing to remove.
    if completed.status.success() || completed.status.code() == Some(5) {
        Ok(())
    } else {
        Err(GitError::failed(
            "config",
            completed.status,
            completed.stderr,
        ))
    }
}

/// Removes every value of `key` from `target`. Removing a key that isn't there succeeds.
///
/// # Errors
///
/// When git refuses the key or can't write the file.
pub async fn unset_config(git: &Git, target: ConfigTarget<'_>, key: &str) -> Result<(), GitError> {
    let completed = config_command(git, target)
        .args(["--unset-all", key])
        .output_unchecked()
        .await?;
    // Exit code 5: there was nothing to remove.
    if completed.status.success() || completed.status.code() == Some(5) {
        Ok(())
    } else {
        Err(GitError::failed(
            "config",
            completed.status,
            completed.stderr,
        ))
    }
}

fn config_command(git: &Git, target: ConfigTarget<'_>) -> crate::GitCommand {
    match target {
        ConfigTarget::Global => git
            .write("config")
            .arg("--global")
            .current_dir(crate::git::outside_repositories()),
        ConfigTarget::Repository(work_tree) => {
            git.write("config").arg("--local").current_dir(work_tree)
        }
    }
}

/// Reads a boolean the way git does.
#[must_use]
pub(crate) fn parse_bool(value: &[u8]) -> Option<bool> {
    let lower = value.to_ascii_lowercase();
    match lower.as_slice() {
        b"true" | b"yes" | b"on" => Some(true),
        b"false" | b"no" | b"off" | b"" => Some(false),
        digits => {
            let number: i64 = std::str::from_utf8(digits).ok()?.parse().ok()?;
            Some(number != 0)
        }
    }
}

/// A key as `git config --list` prints it: section and name in lower case, the subsection
/// (between the first and last dots) as given.
fn canonical_key(key: &str) -> String {
    let (Some(first), Some(last)) = (key.find('.'), key.rfind('.')) else {
        return key.to_ascii_lowercase();
    };
    let (section, rest) = key.split_at(first);
    let (subsection, name) = rest.split_at(last.saturating_sub(first));
    let mut canonical = section.to_ascii_lowercase();
    canonical.push_str(subsection);
    canonical.push_str(&name.to_ascii_lowercase());
    canonical
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(listing: &[u8]) -> ConfigSnapshot {
        ConfigSnapshot::parse(listing.to_vec(), PathBuf::from("/work")).unwrap()
    }

    const LISTING: &[u8] = b"global\0file:/home/me/.gitconfig\0user.name\nMe Myself\0\
        global\0file:/home/me/.gitconfig\0egit.flag\0\
        global\0file:/home/me/.gitconfig\0credential.helper\nstore\0\
        local\0file:.git/config\0credential.helper\n\0\
        local\0file:.git/config\0Branch.Main.remote\norigin\0\
        local\0file:.git/config\0user.name\nRepo Me\0\
        command\0command line:\0color.ui\nnever\0";

    #[test]
    fn reads_every_entry_but_egits_own() {
        let snapshot = snapshot(LISTING);
        assert_eq!(snapshot.len(), 6);
        let first = snapshot.entries().next().unwrap();
        assert_eq!(first.scope, ConfigScope::Global);
        assert_eq!(first.origin, "file:/home/me/.gitconfig");
        assert_eq!(first.key, "user.name");
        assert_eq!(first.value, Some(BStr::new("Me Myself")));
        assert!(
            snapshot
                .entries()
                .all(|entry| entry.scope != ConfigScope::Command)
        );
    }

    #[test]
    fn entries_are_reachable_by_position() {
        let snapshot = snapshot(LISTING);
        assert_eq!(snapshot.entry(0), snapshot.entries().next());
        assert_eq!(snapshot.entry(5), snapshot.entries().last());
        assert_eq!(snapshot.entry(6), None);
    }

    #[test]
    fn the_last_value_wins() {
        let snapshot = snapshot(LISTING);
        let name = snapshot.get("user.name").unwrap();
        assert_eq!(name.value, Some(BStr::new("Repo Me")));
        assert_eq!(name.scope, ConfigScope::Local);
        assert_eq!(snapshot.get_all("user.name").count(), 2);
        assert_eq!(snapshot.get("user.signingkey"), None);
    }

    #[test]
    fn keys_match_like_gits() {
        let snapshot = snapshot(LISTING);
        assert!(snapshot.get("USER.Name").is_some());
        // The listing kept the subsection's case; only an exact subsection matches.
        assert!(snapshot.get("branch.Main.REMOTE").is_none());
        assert_eq!(canonical_key("Branch.Main.Remote"), "branch.Main.remote");
        assert_eq!(canonical_key("User.Name"), "user.name");
        assert_eq!(canonical_key("Core"), "core");
        assert_eq!(
            canonical_key("includeIf.gitdir:~/Work/.path"),
            "includeif.gitdir:~/Work/.path"
        );
    }

    #[test]
    fn values_without_equals_and_empty_values() {
        let snapshot = snapshot(LISTING);
        let flag = snapshot.get("egit.flag").unwrap();
        assert_eq!(flag.value, None);
        assert_eq!(flag.text(), "");
        assert_eq!(flag.bool(), Some(true));
        let reset = snapshot.get_all("credential.helper").nth(1).unwrap();
        assert_eq!(reset.value, Some(BStr::new("")));
        assert_eq!(reset.bool(), Some(false));
    }

    #[test]
    fn origins_name_files_relative_to_where_git_ran() {
        let snapshot = snapshot(LISTING);
        let global = snapshot.entries().next().unwrap();
        assert_eq!(global.file(), Some(PathBuf::from("/home/me/.gitconfig")));
        let local = snapshot.get("user.name").unwrap();
        assert_eq!(local.file(), Some(PathBuf::from("/work/.git/config")));
    }

    #[test]
    fn rejects_truncated_listings() {
        assert!(ConfigSnapshot::parse(b"global\0".to_vec(), PathBuf::new()).is_err());
        assert!(ConfigSnapshot::parse(b"global\0file:x\0".to_vec(), PathBuf::new()).is_err());
        assert!(
            ConfigSnapshot::parse(Vec::new(), PathBuf::new())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn booleans_read_like_gits() {
        for truthy in ["true", "TRUE", "yes", "On", "1", "-3"] {
            assert_eq!(parse_bool(truthy.as_bytes()), Some(true), "{truthy}");
        }
        for falsy in ["false", "No", "OFF", "0", ""] {
            assert_eq!(parse_bool(falsy.as_bytes()), Some(false), "{falsy}");
        }
        assert_eq!(parse_bool(b"maybe"), None);
    }
}
