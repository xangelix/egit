//! Which GitHub repository a remote's URL names: `https://github.com/owner/name.git`,
//! `git@github.com:owner/name.git`, `ssh://git@ssh.github.com:443/owner/name`, an Enterprise
//! host's, and an ssh host alias's (resolved as ssh resolves it).

use std::io::Read as _;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use egit_git::ObjectId;

use crate::host::{GitHubHost, GitHubHosts, strip_prefix_ignore_case};

/// A repository on a GitHub host.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct GitHubRepository {
    host: GitHubHost,
    owner: String,
    name: String,
}

impl GitHubRepository {
    /// `owner/name` on `host`; `None` unless both are names GitHub could give.
    #[must_use]
    pub fn new(host: GitHubHost, owner: &str, name: &str) -> Option<Self> {
        (is_name(owner) && is_name(name)).then(|| Self {
            host,
            owner: owner.to_owned(),
            name: name.to_owned(),
        })
    }

    /// Its host.
    #[must_use]
    pub const fn host(&self) -> &GitHubHost {
        &self.host
    }

    /// Its owner (a user or an organization).
    #[must_use]
    pub fn owner(&self) -> &str {
        &self.owner
    }

    /// Its name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// `owner/name`.
    #[must_use]
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }

    /// Whether `other` is the same repository (GitHub's names ignore case).
    #[must_use]
    pub fn is(&self, other: &Self) -> bool {
        self.host == other.host
            && self.owner.eq_ignore_ascii_case(&other.owner)
            && self.name.eq_ignore_ascii_case(&other.name)
    }

    /// Whether it is `owner/name` on the same host (ignoring case).
    #[must_use]
    pub fn is_named(&self, full_name: &str) -> bool {
        full_name.split_once('/').is_some_and(|(owner, name)| {
            self.owner.eq_ignore_ascii_case(owner) && self.name.eq_ignore_ascii_case(name)
        })
    }

    /// Its web page.
    #[must_use]
    pub fn web_url(&self) -> String {
        self.host.web_url(&format!("/{}/{}", self.owner, self.name))
    }

    /// A pull request's web page.
    #[must_use]
    pub fn pull_request_url(&self, number: u32) -> String {
        self.host
            .web_url(&format!("/{}/{}/pull/{number}", self.owner, self.name))
    }

    /// A commit's web page.
    #[must_use]
    pub fn commit_url(&self, commit: ObjectId) -> String {
        self.host
            .web_url(&format!("/{}/{}/commit/{commit}", self.owner, self.name))
    }
}

/// The GitHub repository the remote URL `url` names on `hosts`, if any.
///
/// An ssh URL whose host isn't one of them may be an alias from the user's ssh configuration:
/// `alias` says which host it stands for ([`resolve_ssh_alias`]).
pub fn github_repository(
    url: &str,
    hosts: &GitHubHosts,
    alias: impl FnOnce(&str) -> Option<String>,
) -> Option<GitHubRepository> {
    let url = url.trim();
    let (host, path) = if let Some((scheme, rest)) = url.split_once("://") {
        let (authority, path) = rest.split_once('/')?;
        let address = authority
            .rsplit_once('@')
            .map_or(authority, |(_, address)| address);
        match scheme.to_ascii_lowercase().as_str() {
            "https" => (hosts.for_web(address, true)?.clone(), path),
            "http" => (hosts.for_web(address, false)?.clone(), path),
            "ssh" | "git+ssh" | "ssh+git" | "git" => {
                (ssh_host(hosts, ssh_hostname(address), alias)?, path)
            }
            _ => return None,
        }
    } else {
        // scp-like: `[user@]host:path`, where what comes before the colon has no slash (else
        // it is a local path).
        let (authority, path) = url.split_once(':')?;
        if authority.is_empty() || authority.contains('/') {
            return None;
        }
        let hostname = authority
            .rsplit_once('@')
            .map_or(authority, |(_, host)| host);
        (ssh_host(hosts, hostname, alias)?, path)
    };
    let path = path.trim_matches('/');
    let path = path
        .strip_suffix(".git")
        .or_else(|| strip_suffix_ignore_case(path, ".git"))
        .unwrap_or(path);
    let (owner, name) = path.split_once('/')?;
    GitHubRepository::new(host, owner, name)
}

/// The GitHub host an ssh URL's host name is, directly or as an alias of one.
fn ssh_host(
    hosts: &GitHubHosts,
    hostname: &str,
    alias: impl FnOnce(&str) -> Option<String>,
) -> Option<GitHubHost> {
    if let Some(host) = hosts.for_ssh(hostname) {
        return Some(host.clone());
    }
    let resolved = alias(hostname)?;
    hosts.for_ssh(&resolved).cloned()
}

/// An ssh URL's `host[:port]`'s host.
fn ssh_hostname(address: &str) -> &str {
    if address.starts_with('[') {
        return address
            .find(']')
            .and_then(|end| address.get(1..end))
            .unwrap_or(address);
    }
    address.split(':').next().unwrap_or(address)
}

/// `text` without `suffix`, compared ignoring ASCII case.
fn strip_suffix_ignore_case<'a>(text: &'a str, suffix: &str) -> Option<&'a str> {
    let start = text.len().checked_sub(suffix.len())?;
    text.get(start..)?
        .eq_ignore_ascii_case(suffix)
        .then(|| text.get(..start))
        .flatten()
}

/// Whether `name` could be a GitHub owner's or repository's name: letters, digits, `-`, `_`,
/// `.`, not starting with a dot (so never `.` or `..`).
fn is_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
}

/// How long `ssh -G` may take: it reads configuration and connects to nothing.
const SSH_TIMEOUT: Duration = Duration::from_secs(2);

/// The host an ssh host alias stands for, as ssh resolves it (`ssh -G`'s `hostname`): `None`
/// when ssh isn't there, fails, or the alias isn't one (ssh then names the alias itself).
#[must_use]
pub fn resolve_ssh_alias(alias: &str) -> Option<String> {
    // Never an option, whatever a URL says.
    if alias.starts_with('-') || !is_name(alias) {
        return None;
    }
    let mut child = Command::new("ssh")
        .args(["-G", "--", alias])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < SSH_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(5));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    let mut printed = String::new();
    child.stdout.take()?.read_to_string(&mut printed).ok()?;
    if !status.success() {
        return None;
    }
    let hostname = printed
        .lines()
        .find_map(|line| strip_prefix_ignore_case(line, "hostname "))?
        .trim();
    (hostname != alias).then(|| hostname.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(url: &str) -> Option<String> {
        let hosts = GitHubHosts::with_enterprise(["ghe.example.com:8443"]);
        github_repository(url, &hosts, |alias| {
            (alias == "github-work").then(|| "github.com".to_owned())
        })
        .map(|repository| format!("{} {}", repository.host().name(), repository.full_name()))
    }

    #[test]
    fn every_form_of_a_github_url_names_its_repository() {
        for url in [
            "https://github.com/rust-lang/rust",
            "https://github.com/rust-lang/rust.git",
            "https://github.com/rust-lang/rust/",
            "https://user@github.com/rust-lang/rust.git",
            "HTTPS://www.GitHub.com/rust-lang/rust.GIT",
            "git@github.com:rust-lang/rust.git",
            "github.com:rust-lang/rust",
            "ssh://git@github.com/rust-lang/rust.git",
            "ssh://git@ssh.github.com:443/rust-lang/rust.git",
            "git+ssh://git@github.com/rust-lang/rust",
            "git://github.com/rust-lang/rust.git",
            "git@github-work:rust-lang/rust.git",
        ] {
            assert_eq!(
                named(url).as_deref(),
                Some("github.com rust-lang/rust"),
                "{url}"
            );
        }
    }

    #[test]
    fn enterprise_urls_name_their_host() {
        assert_eq!(
            named("https://ghe.example.com:8443/team/tool.git").as_deref(),
            Some("ghe.example.com:8443 team/tool")
        );
        assert_eq!(
            named("git@ghe.example.com:team/tool.git").as_deref(),
            Some("ghe.example.com:8443 team/tool")
        );
        // The web's port matters for a web URL.
        assert_eq!(named("https://ghe.example.com/team/tool.git"), None);
    }

    #[test]
    fn other_urls_name_nothing() {
        for url in [
            "/srv/git/repo.git",
            "./relative/repo",
            "file:///srv/git/repo.git",
            "https://gitlab.com/group/project.git",
            "git@gitlab.com:group/project.git",
            "git@unknown-alias:owner/name.git",
            "https://github.com/rust-lang",
            "https://github.com/rust-lang/rust/tree/master",
            "https://github.com/../etc",
            "https://github.com/owner/..",
            "ftp://github.com/owner/name",
            "",
        ] {
            assert_eq!(named(url), None, "{url}");
        }
    }

    #[test]
    fn web_pages_are_on_the_host() {
        let repository =
            GitHubRepository::new(GitHubHost::github_com(), "rust-lang", "rust").unwrap();
        assert_eq!(
            repository.pull_request_url(42),
            "https://github.com/rust-lang/rust/pull/42"
        );
        assert!(repository.is_named("Rust-Lang/RUST"));
        assert!(!repository.is_named("rust-lang/cargo"));
    }

    #[test]
    fn aliases_that_could_be_options_are_never_passed_to_ssh() {
        assert_eq!(resolve_ssh_alias("-oProxyCommand=touch /tmp/x"), None);
        assert_eq!(resolve_ssh_alias("a b"), None);
    }
}
