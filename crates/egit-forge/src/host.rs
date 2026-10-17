//! GitHub's hosts: github.com, and the GitHub Enterprise Server hosts the user lists. Each knows
//! where its APIs and web pages are.

/// A GitHub host: github.com, or a GitHub Enterprise Server.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ForgeHost {
    /// The host, lowercase, with its port when it isn't the scheme's default.
    name: String,
    /// HTTPS; plain HTTP only for the loopback address (a test server).
    secure: bool,
}

/// github.com's name.
const GITHUB_COM: &str = "github.com";

impl ForgeHost {
    /// github.com.
    #[must_use]
    pub fn github_com() -> Self {
        Self {
            name: GITHUB_COM.to_owned(),
            secure: true,
        }
    }

    /// An Enterprise host as the user lists it: `host`, `host:port`, or either after
    /// `https://` (or `http://`, taken only for the loopback address). `None` when it isn't
    /// usable as a host.
    #[must_use]
    pub fn parse(entry: &str) -> Option<Self> {
        let entry = entry.trim().trim_end_matches('/');
        let (secure, address) = strip_prefix_ignore_case(entry, "https://")
            .map(|rest| (true, rest))
            .or_else(|| strip_prefix_ignore_case(entry, "http://").map(|rest| (false, rest)))
            .unwrap_or((true, entry));
        let name = normalized_address(address, secure)?;
        if !secure && !is_loopback(hostname_of(&name)) {
            return None;
        }
        Some(Self { name, secure })
    }

    /// The host as the user would write it (`github.com`, `ghe.example.com:8443`).
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether this is github.com.
    #[must_use]
    pub fn is_github_com(&self) -> bool {
        self.name == GITHUB_COM
    }

    /// The protocol credentials are kept under for it (`https`).
    #[must_use]
    pub const fn protocol(&self) -> &'static str {
        if self.secure { "https" } else { "http" }
    }

    /// Its GraphQL endpoint.
    pub(crate) fn graphql_url(&self) -> String {
        if self.is_github_com() {
            "https://api.github.com/graphql".to_owned()
        } else {
            format!("{}://{}/api/graphql", self.protocol(), self.name)
        }
    }

    /// Its REST endpoint for `path` (starting with `/`).
    pub(crate) fn rest_url(&self, path: &str) -> String {
        if self.is_github_com() {
            format!("https://api.github.com{path}")
        } else {
            format!("{}://{}/api/v3{path}", self.protocol(), self.name)
        }
    }

    /// Its web page at `path` (starting with `/`).
    #[must_use]
    pub fn web_url(&self, path: &str) -> String {
        format!("{}://{}{path}", self.protocol(), self.name)
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

/// The GitHub hosts known: github.com, then the Enterprise hosts listed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForgeHosts {
    hosts: Vec<ForgeHost>,
}

impl Default for ForgeHosts {
    fn default() -> Self {
        Self {
            hosts: vec![ForgeHost::github_com()],
        }
    }
}

impl ForgeHosts {
    /// github.com and the Enterprise hosts `listed` (as [`ForgeHost::parse`] reads them; the
    /// unusable are skipped, and so are repeats).
    #[must_use]
    pub fn with_enterprise<'a>(listed: impl IntoIterator<Item = &'a str>) -> Self {
        let mut hosts = Self::default();
        for host in listed.into_iter().filter_map(ForgeHost::parse) {
            if !hosts.hosts.iter().any(|known| known.name == host.name) {
                hosts.hosts.push(host);
            }
        }
        hosts
    }

    /// The hosts, github.com first.
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
        // GitHub's ssh over the HTTPS port.
        let hostname = if hostname == "ssh.github.com" {
            GITHUB_COM
        } else {
            &hostname
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

    #[test]
    fn github_com_s_apis_are_on_api_github_com() {
        let host = ForgeHost::github_com();
        assert_eq!(host.graphql_url(), "https://api.github.com/graphql");
        assert_eq!(
            host.rest_url("/repos/a/b/pulls"),
            "https://api.github.com/repos/a/b/pulls"
        );
        assert_eq!(host.web_url("/a/b/pull/1"), "https://github.com/a/b/pull/1");
    }

    #[test]
    fn enterprise_hosts_serve_their_apis_under_api() {
        let host = ForgeHost::parse("https://GHE.example.com:8443/").unwrap();
        assert_eq!(host.name(), "ghe.example.com:8443");
        assert_eq!(
            host.graphql_url(),
            "https://ghe.example.com:8443/api/graphql"
        );
        assert_eq!(
            host.rest_url("/repos/a/b/pulls"),
            "https://ghe.example.com:8443/api/v3/repos/a/b/pulls"
        );
        assert_eq!(
            ForgeHost::parse("ghe.example.com:443").unwrap().name(),
            "ghe.example.com"
        );
    }

    #[test]
    fn plain_http_is_only_for_the_loopback_address() {
        assert!(ForgeHost::parse("http://ghe.example.com").is_none());
        let local = ForgeHost::parse("http://127.0.0.1:8080").unwrap();
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
            assert!(ForgeHost::parse(entry).is_none(), "{entry}");
        }
        let hosts = ForgeHosts::with_enterprise(["ghe.example.com", "GHE.example.com", "bad host"]);
        assert_eq!(hosts.hosts().len(), 2);
    }

    #[test]
    fn web_and_ssh_addresses_find_their_hosts() {
        let hosts = ForgeHosts::with_enterprise(["ghe.example.com:8443"]);
        assert!(
            hosts
                .for_web("www.github.com", true)
                .unwrap()
                .is_github_com()
        );
        assert!(
            hosts
                .for_web("github.com:443", true)
                .unwrap()
                .is_github_com()
        );
        assert!(hosts.for_web("github.com:8443", true).is_none());
        assert!(hosts.for_web("ghe.example.com:8443", true).is_some());
        assert!(hosts.for_ssh("ssh.github.com").unwrap().is_github_com());
        assert_eq!(
            hosts.for_ssh("GHE.example.com").unwrap().name(),
            "ghe.example.com:8443"
        );
        assert!(hosts.for_ssh("gitlab.com").is_none());
    }
}
