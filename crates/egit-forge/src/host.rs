//! The forges' hosts: github.com and gitlab.com, and the GitHub Enterprise and self-managed
//! GitLab hosts the user lists. Each knows its forge and where its APIs and web pages are.

/// Which forge a host runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ForgeKind {
    /// GitHub (github.com, or GitHub Enterprise Server).
    GitHub,
    /// GitLab (gitlab.com, or a self-managed instance).
    GitLab,
}

impl ForgeKind {
    /// Its name, for people ("GitHub").
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::GitHub => "GitHub",
            Self::GitLab => "GitLab",
        }
    }
}

/// A forge's host: github.com, gitlab.com, or one the user lists.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ForgeHost {
    kind: ForgeKind,
    /// The host, lowercase, with its port when it isn't the scheme's default.
    name: String,
    /// HTTPS; plain HTTP only for the loopback address (a test server).
    secure: bool,
}

/// github.com's and gitlab.com's names.
const GITHUB_COM: &str = "github.com";
const GITLAB_COM: &str = "gitlab.com";

impl ForgeHost {
    /// github.com.
    #[must_use]
    pub fn github_com() -> Self {
        Self {
            kind: ForgeKind::GitHub,
            name: GITHUB_COM.to_owned(),
            secure: true,
        }
    }

    /// gitlab.com.
    #[must_use]
    pub fn gitlab_com() -> Self {
        Self {
            kind: ForgeKind::GitLab,
            name: GITLAB_COM.to_owned(),
            secure: true,
        }
    }

    /// A host of `kind` as the user lists it: `host`, `host:port`, or either after `https://`
    /// (or `http://`, taken only for the loopback address). `None` when it isn't usable as a
    /// host.
    #[must_use]
    pub fn parse(kind: ForgeKind, entry: &str) -> Option<Self> {
        let entry = entry.trim().trim_end_matches('/');
        let (secure, address) = strip_prefix_ignore_case(entry, "https://")
            .map(|rest| (true, rest))
            .or_else(|| strip_prefix_ignore_case(entry, "http://").map(|rest| (false, rest)))
            .unwrap_or((true, entry));
        let name = normalized_address(address, secure)?;
        if !secure && !is_loopback(hostname_of(&name)) {
            return None;
        }
        Some(Self { kind, name, secure })
    }

    /// Which forge it runs.
    #[must_use]
    pub const fn kind(&self) -> ForgeKind {
        self.kind
    }

    /// The host as the user would write it (`github.com`, `ghe.example.com:8443`).
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether this is github.com or gitlab.com, which everyone's lists hold.
    #[must_use]
    pub fn is_public(&self) -> bool {
        matches!(
            (self.kind, self.name.as_str()),
            (ForgeKind::GitHub, GITHUB_COM) | (ForgeKind::GitLab, GITLAB_COM)
        )
    }

    /// The protocol credentials are kept under for it (`https`).
    #[must_use]
    pub const fn protocol(&self) -> &'static str {
        if self.secure { "https" } else { "http" }
    }

    /// Its GraphQL endpoint.
    pub(crate) fn graphql_url(&self) -> String {
        if self.kind == ForgeKind::GitHub && self.name == GITHUB_COM {
            "https://api.github.com/graphql".to_owned()
        } else {
            format!("{}://{}/api/graphql", self.protocol(), self.name)
        }
    }

    /// Its REST endpoint for `path` (starting with `/`).
    pub(crate) fn rest_url(&self, path: &str) -> String {
        match self.kind {
            ForgeKind::GitHub if self.name == GITHUB_COM => format!("https://api.github.com{path}"),
            ForgeKind::GitHub => format!("{}://{}/api/v3{path}", self.protocol(), self.name),
            ForgeKind::GitLab => format!("{}://{}/api/v4{path}", self.protocol(), self.name),
        }
    }

    /// Its web page at `path` (starting with `/`).
    #[must_use]
    pub fn web_url(&self, path: &str) -> String {
        format!("{}://{}{path}", self.protocol(), self.name)
    }

    /// Where a personal access token is made on it, with what egit needs ticked.
    #[must_use]
    pub fn new_token_url(&self) -> String {
        match self.kind {
            ForgeKind::GitHub => self.web_url("/settings/tokens/new?scopes=repo&description=egit"),
            ForgeKind::GitLab => {
                self.web_url("/-/user_settings/personal_access_tokens?name=egit&scopes=api")
            }
        }
    }

    /// Whether a credential may be sent to it: over HTTPS, or to the loopback address.
    pub(crate) fn is_safe_for_credentials(&self) -> bool {
        self.secure || is_loopback(hostname_of(&self.name))
    }

    /// The name without its port: what an ssh URL to it names.
    fn hostname(&self) -> &str {
        hostname_of(&self.name)
    }
}

/// The forges' hosts known: github.com, gitlab.com, then those listed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForgeHosts {
    hosts: Vec<ForgeHost>,
}

impl Default for ForgeHosts {
    fn default() -> Self {
        Self {
            hosts: vec![ForgeHost::github_com(), ForgeHost::gitlab_com()],
        }
    }
}

impl ForgeHosts {
    /// github.com, gitlab.com, and the hosts listed: GitHub Enterprise ones (`github`) and
    /// self-managed GitLab ones (`gitlab`), as [`ForgeHost::parse`] reads them; the unusable
    /// are skipped, and so are repeats (the first listing of a host wins).
    #[must_use]
    pub fn with_listed<'a>(
        github: impl IntoIterator<Item = &'a str>,
        gitlab: impl IntoIterator<Item = &'a str>,
    ) -> Self {
        let mut hosts = Self::default();
        let listed = github
            .into_iter()
            .filter_map(|entry| ForgeHost::parse(ForgeKind::GitHub, entry))
            .chain(
                gitlab
                    .into_iter()
                    .filter_map(|entry| ForgeHost::parse(ForgeKind::GitLab, entry)),
            );
        for host in listed {
            if !hosts.hosts.iter().any(|known| known.name == host.name) {
                hosts.hosts.push(host);
            }
        }
        hosts
    }

    /// The hosts, github.com and gitlab.com first.
    #[must_use]
    pub fn hosts(&self) -> &[ForgeHost] {
        &self.hosts
    }

    /// The host an `http(s)://` URL's address (`host[:port]`) names.
    pub(crate) fn for_web(&self, address: &str, secure: bool) -> Option<&ForgeHost> {
        let address = normalized_address(address, secure)?;
        let address = address.strip_prefix("www.").unwrap_or(&address);
        self.hosts.iter().find(|host| host.name == address)
    }

    /// The host an ssh URL's host name names (its port is ssh's, not the web's).
    pub(crate) fn for_ssh(&self, hostname: &str) -> Option<&ForgeHost> {
        let hostname = hostname.to_ascii_lowercase();
        // GitHub's and GitLab's ssh over the HTTPS port.
        let hostname = match hostname.as_str() {
            "ssh.github.com" => GITHUB_COM,
            "altssh.gitlab.com" => GITLAB_COM,
            other => other,
        };
        self.hosts.iter().find(|host| host.hostname() == hostname)
    }
}

/// `host[:port]` lowercased, without the scheme's default port; `None` if it isn't a host.
fn normalized_address(address: &str, secure: bool) -> Option<String> {
    if address.is_empty()
        || !address
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b".-:[]".contains(&byte))
    {
        return None;
    }
    let address = address.to_ascii_lowercase();
    let default_port = if secure { ":443" } else { ":80" };
    let address = address.strip_suffix(default_port).unwrap_or(&address);
    let hostname = hostname_of(address);
    if hostname.is_empty() || hostname.starts_with('-') || hostname.starts_with('.') {
        return None;
    }
    // After the host, only a port: one to five digits.
    let port = address.get(hostname.len()..).unwrap_or_default();
    if let Some(port) = port.strip_prefix(':') {
        if port.is_empty() || port.len() > 5 || !port.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
    } else if !port.is_empty() {
        return None;
    }
    Some(address.to_owned())
}

/// A `host[:port]` address's host (an IPv6 one keeps its brackets; unclosed, there is none).
fn hostname_of(address: &str) -> &str {
    if address.starts_with('[') {
        return address
            .find(']')
            .and_then(|end| address.get(..=end))
            .unwrap_or_default();
    }
    address.split(':').next().unwrap_or(address)
}

/// Whether a host name is the loopback address.
fn is_loopback(hostname: &str) -> bool {
    matches!(hostname, "localhost" | "127.0.0.1" | "[::1]")
}

/// `text` without `prefix`, compared ignoring ASCII case.
pub(crate) fn strip_prefix_ignore_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| text.get(prefix.len()..))
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn github(entry: &str) -> Option<ForgeHost> {
        ForgeHost::parse(ForgeKind::GitHub, entry)
    }

    #[test]
    fn github_com_s_apis_are_on_api_github_com() {
        let host = ForgeHost::github_com();
        assert_eq!(host.graphql_url(), "https://api.github.com/graphql");
        assert_eq!(
            host.rest_url("/repos/a/b/pulls"),
            "https://api.github.com/repos/a/b/pulls"
        );
        assert_eq!(host.web_url("/a/b/pull/1"), "https://github.com/a/b/pull/1");
        assert!(host.is_public());
    }

    #[test]
    fn enterprise_hosts_serve_their_apis_under_api() {
        let host = github("https://GHE.example.com:8443/").unwrap();
        assert_eq!(host.name(), "ghe.example.com:8443");
        assert!(!host.is_public());
        assert_eq!(
            host.graphql_url(),
            "https://ghe.example.com:8443/api/graphql"
        );
        assert_eq!(
            host.rest_url("/repos/a/b/pulls"),
            "https://ghe.example.com:8443/api/v3/repos/a/b/pulls"
        );
        assert_eq!(
            github("ghe.example.com:443").unwrap().name(),
            "ghe.example.com"
        );
    }

    #[test]
    fn gitlab_hosts_serve_graphql_and_rest_v4() {
        let host = ForgeHost::gitlab_com();
        assert_eq!(host.graphql_url(), "https://gitlab.com/api/graphql");
        assert_eq!(
            host.rest_url("/projects/a%2Fb/merge_requests"),
            "https://gitlab.com/api/v4/projects/a%2Fb/merge_requests"
        );
        assert!(host.is_public());
        let own = ForgeHost::parse(ForgeKind::GitLab, "git.example.org").unwrap();
        assert_eq!(own.kind(), ForgeKind::GitLab);
        assert_eq!(own.graphql_url(), "https://git.example.org/api/graphql");
        assert_eq!(
            own.new_token_url(),
            "https://git.example.org/-/user_settings/personal_access_tokens?name=egit&scopes=api"
        );
    }

    #[test]
    fn plain_http_is_only_for_the_loopback_address() {
        assert!(github("http://ghe.example.com").is_none());
        let local = github("http://127.0.0.1:8080").unwrap();
        assert_eq!(local.protocol(), "http");
        assert!(local.is_safe_for_credentials());
        assert_eq!(local.graphql_url(), "http://127.0.0.1:8080/api/graphql");
    }

    #[test]
    fn unusable_entries_are_skipped() {
        for entry in [
            "",
            "-oProxyCommand=x",
            "a b",
            "host/path",
            "https://",
            "host:",
            "host:http",
            "[::1",
        ] {
            assert!(github(entry).is_none(), "{entry}");
        }
        let hosts = ForgeHosts::with_listed(
            ["ghe.example.com", "GHE.example.com", "bad host"],
            ["ghe.example.com", "git.example.org"],
        );
        // github.com, gitlab.com, the Enterprise host (its first listing), the GitLab one.
        assert_eq!(hosts.hosts().len(), 4);
        assert_eq!(hosts.hosts()[2].kind(), ForgeKind::GitHub);
    }

    #[test]
    fn web_and_ssh_addresses_find_their_hosts() {
        let hosts = ForgeHosts::with_listed(["ghe.example.com:8443"], ["git.example.org"]);
        let kind =
            |host: Option<&ForgeHost>| host.map(|host| (host.kind(), host.name().to_owned()));
        let github = Some((ForgeKind::GitHub, "github.com".to_owned()));
        let gitlab = Some((ForgeKind::GitLab, "gitlab.com".to_owned()));
        assert_eq!(kind(hosts.for_web("www.github.com", true)), github);
        assert_eq!(kind(hosts.for_web("github.com:443", true)), github);
        assert!(hosts.for_web("github.com:8443", true).is_none());
        assert!(hosts.for_web("ghe.example.com:8443", true).is_some());
        assert_eq!(kind(hosts.for_ssh("ssh.github.com")), github);
        assert_eq!(kind(hosts.for_web("gitlab.com", true)), gitlab);
        assert_eq!(kind(hosts.for_ssh("altssh.gitlab.com")), gitlab);
        assert_eq!(
            kind(hosts.for_ssh("GIT.example.org")),
            Some((ForgeKind::GitLab, "git.example.org".to_owned()))
        );
        assert_eq!(
            hosts.for_ssh("GHE.example.com").unwrap().name(),
            "ghe.example.com:8443"
        );
        assert!(hosts.for_ssh("bitbucket.org").is_none());
    }
}
