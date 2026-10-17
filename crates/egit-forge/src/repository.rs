//! Which forge repository a remote's URL names: `https://github.com/owner/name.git`,
//! `git@gitlab.com:group/subgroup/project.git`, `ssh://git@ssh.github.com:443/owner/name`, a
//! listed host's, and an ssh host alias's (resolved as ssh resolves it).

use std::io::Read as _;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use egit_git::ObjectId;

use crate::host::{ForgeHost, ForgeHosts, ForgeKind, strip_prefix_ignore_case};

/// A repository on a forge's host: GitHub's `owner/name`, or GitLab's `group/…/project`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ForgeRepository {
    host: ForgeHost,
    path: String,
}

impl ForgeRepository {
    /// `path` on `host`: `owner/name` on GitHub; on GitLab, a project in its group and the
    /// groups around those. `None` unless each part is a name the forge could give.
    #[must_use]
    pub fn new(host: ForgeHost, path: &str) -> Option<Self> {
        let parts = path.split('/').count();
        let fits = match host.kind() {
            ForgeKind::GitHub => parts == 2,
            ForgeKind::GitLab => parts >= 2,
        };
        // GitLab's pages say `/-/` where a path ends.
        let named = path.split('/').all(|part| is_name(part) && part != "-");
        (fits && named).then(|| Self {
            host,
            path: path.to_owned(),
        })
    }

    /// Its host.
    #[must_use]
    pub const fn host(&self) -> &ForgeHost {
        &self.host
    }

    /// Its path: `owner/name`, or `group/subgroup/project`.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Its owner: a user, an organization, or the groups it is in.
    #[must_use]
    pub fn owner(&self) -> &str {
        self.path
            .rsplit_once('/')
            .map_or(self.path.as_str(), |(owner, _)| owner)
    }

    /// Its name, the path's last part.
    #[must_use]
    pub fn name(&self) -> &str {
        self.path
            .rsplit_once('/')
            .map_or(self.path.as_str(), |(_, name)| name)
    }

    /// Its path, for people.
    #[must_use]
    pub fn full_name(&self) -> String {
        self.path.clone()
    }

    /// Whether `other` is the same repository (forges' names ignore case).
    #[must_use]
    pub fn is(&self, other: &Self) -> bool {
        self.host == other.host && self.is_named(&other.path)
    }

    /// Whether its path is `path` (ignoring case).
    #[must_use]
    pub fn is_named(&self, path: &str) -> bool {
        self.path.eq_ignore_ascii_case(path)
    }

    /// Its web page.
    #[must_use]
    pub fn web_url(&self) -> String {
        self.host.web_url(&format!("/{}", self.path))
    }

    /// A pull request's (or merge request's) web page.
    #[must_use]
    pub fn pull_request_url(&self, number: u32) -> String {
        let path = &self.path;
        self.host.web_url(&match self.host.kind() {
            ForgeKind::GitHub => format!("/{path}/pull/{number}"),
            ForgeKind::GitLab => format!("/{path}/-/merge_requests/{number}"),
        })
    }

    /// A commit's web page.
    #[must_use]
    pub fn commit_url(&self, commit: ObjectId) -> String {
        let path = &self.path;
        self.host.web_url(&match self.host.kind() {
            ForgeKind::GitHub => format!("/{path}/commit/{commit}"),
            ForgeKind::GitLab => format!("/{path}/-/commit/{commit}"),
        })
    }

    /// A branch's web page (`branch`, its name there: whatever a ref may hold is escaped).
    #[must_use]
    pub fn branch_url(&self, branch: &str) -> String {
        let path = &self.path;
        let branch = escape_path(branch);
        self.host.web_url(&match self.host.kind() {
            ForgeKind::GitHub => format!("/{path}/tree/{branch}"),
            ForgeKind::GitLab => format!("/{path}/-/tree/{branch}"),
        })
    }
}

/// `text` as a URL's path: its slashes kept, anything but letters, digits, and `-._~`
/// percent-escaped.
fn escape_path(text: &str) -> String {
    use std::fmt::Write as _;
    let mut escaped = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte) {
            escaped.push(char::from(byte));
        } else {
            let _ = write!(escaped, "%{byte:02X}");
        }
    }
    escaped
}

/// The forge repository the remote URL `url` names on `hosts`, if any.
///
/// An ssh URL whose host isn't one of them may be an alias from the user's ssh configuration:
/// `alias` says which host it stands for ([`resolve_ssh_alias`]).
pub fn forge_repository(
    url: &str,
    hosts: &ForgeHosts,
    alias: impl FnOnce(&str) -> Option<String>,
) -> Option<ForgeRepository> {
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
    ForgeRepository::new(host, path)
}

/// The forge host an ssh URL's host name is, directly or as an alias of one.
fn ssh_host(
    hosts: &ForgeHosts,
    hostname: &str,
    alias: impl FnOnce(&str) -> Option<String>,
) -> Option<ForgeHost> {
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

/// Whether `name` could be a part of a forge's repository path: letters, digits, `-`, `_`,
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
        let hosts = ForgeHosts::with_listed(["ghe.example.com:8443"], ["git.example.org"]);
        forge_repository(url, &hosts, |alias| match alias {
            "github-work" => Some("github.com".to_owned()),
            "gitlab-work" => Some("gitlab.com".to_owned()),
            _ => None,
        })
        .map(|repository| format!("{} {}", repository.host().name(), repository.full_name()))
    }

    #[test]
    fn branches_have_web_pages_whatever_their_names() {
        let github = ForgeRepository::new(ForgeHost::github_com(), "ada/tool").unwrap();
        assert_eq!(
            github.branch_url("feat/keep-newest"),
            "https://github.com/ada/tool/tree/feat/keep-newest"
        );
        assert_eq!(
            github.branch_url("fix#12 100%"),
            "https://github.com/ada/tool/tree/fix%2312%20100%25"
        );
        let gitlab = ForgeRepository::new(ForgeHost::gitlab_com(), "team/sub/tool").unwrap();
        assert_eq!(
            gitlab.branch_url("main"),
            "https://gitlab.com/team/sub/tool/-/tree/main"
        );
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
    fn every_form_of_a_gitlab_url_names_its_project_in_its_groups() {
        for url in [
            "https://gitlab.com/gitlab-org/gitlab",
            "https://gitlab.com/gitlab-org/gitlab.git",
            "git@gitlab.com:gitlab-org/gitlab.git",
            "ssh://git@altssh.gitlab.com:443/gitlab-org/gitlab.git",
            "git@gitlab-work:gitlab-org/gitlab.git",
        ] {
            assert_eq!(
                named(url).as_deref(),
                Some("gitlab.com gitlab-org/gitlab"),
                "{url}"
            );
        }
        assert_eq!(
            named("https://gitlab.com/group/sub.group/deeper/project.git").as_deref(),
            Some("gitlab.com group/sub.group/deeper/project")
        );
        assert_eq!(
            named("git@git.example.org:team/tool.git").as_deref(),
            Some("git.example.org team/tool")
        );
        // A page's address, not a repository's.
        assert_eq!(named("https://gitlab.com/group/project/-/tree/main"), None);
        assert_eq!(named("https://gitlab.com/project"), None);
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
            "https://bitbucket.org/group/project.git",
            "git@bitbucket.org:group/project.git",
            "git@unknown-alias:owner/name.git",
            "https://github.com/group/sub/project",
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
        let repository = ForgeRepository::new(ForgeHost::github_com(), "rust-lang/rust").unwrap();
        assert_eq!(
            repository.pull_request_url(42),
            "https://github.com/rust-lang/rust/pull/42"
        );
        assert!(repository.is_named("Rust-Lang/RUST"));
        assert!(!repository.is_named("rust-lang/cargo"));
        assert_eq!(
            (repository.owner(), repository.name()),
            ("rust-lang", "rust")
        );
        let project = ForgeRepository::new(ForgeHost::gitlab_com(), "a/b/c").unwrap();
        assert_eq!(
            project.pull_request_url(7),
            "https://gitlab.com/a/b/c/-/merge_requests/7"
        );
        assert_eq!((project.owner(), project.name()), ("a/b", "c"));
    }

    #[test]
    fn aliases_that_could_be_options_are_never_passed_to_ssh() {
        assert_eq!(resolve_ssh_alias("-oProxyCommand=touch /tmp/x"), None);
        assert_eq!(resolve_ssh_alias("a b"), None);
    }
}
