//! Handing things to the desktop: a web page to the user's browser, through the desktop's
//! opener (`xdg-open`, else `gio open`), off the interface thread.

use std::process::{Command, Stdio};

use crate::jobs;

/// Where web pages go: the user's browser, or (for tests) a list of them.
#[derive(Debug, Default)]
pub(crate) struct Opener {
    recorded: Option<Vec<String>>,
}

impl Opener {
    /// An opener that only lists the pages, starting no browser.
    #[cfg(test)]
    pub(crate) const fn recording() -> Self {
        Self {
            recorded: Some(Vec::new()),
        }
    }

    /// The pages listed, by a recording opener.
    #[cfg(test)]
    pub(crate) fn recorded(&self) -> &[String] {
        self.recorded.as_deref().unwrap_or_default()
    }

    /// Opens `url` if it is a web page.
    pub(crate) fn open(&mut self, url: String) {
        if !is_web_page(&url) {
            tracing::warn!(url, "not opening what isn't a web page");
            return;
        }
        match &mut self.recorded {
            Some(recorded) => recorded.push(url),
            None => jobs::detached(move || open_web_page(&url)),
        }
    }
}

/// Whether `url` is a web page egit may hand to the browser: HTTPS, or plain HTTP to the
/// loopback address; nothing that could pass for an option or hold a line break.
pub(crate) fn is_web_page(url: &str) -> bool {
    let safe = !url.is_empty()
        && !url
            .chars()
            .any(|character| character.is_whitespace() || character.is_control());
    let loopback = ["http://127.0.0.1", "http://localhost", "http://[::1]"]
        .iter()
        .any(|prefix| {
            url.strip_prefix(prefix)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with([':', '/']))
        });
    safe && (url.starts_with("https://") || loopback)
}

/// Opens `url` in the user's browser; a failure is logged (the desktop has no opener).
fn open_web_page(url: &str) {
    for (program, args) in [("xdg-open", &[][..]), ("gio", &["open"][..])] {
        let opened = Command::new(program)
            .args(args)
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        match opened {
            Ok(status) if status.success() => return,
            Ok(status) => tracing::warn!(program, %status, "the desktop didn't open a page"),
            Err(error) => tracing::debug!(program, %error, "no desktop opener"),
        }
    }
    tracing::warn!(
        url,
        "the page couldn't be opened: the desktop has no opener"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_web_pages_are_opened() {
        assert!(is_web_page("https://github.com/octo/tool/pull/1"));
        assert!(is_web_page("http://127.0.0.1:8080/x"));
        assert!(!is_web_page("http://example.com"));
        assert!(!is_web_page("http://127.0.0.1.evil.com/"));
        assert!(!is_web_page("file:///etc/passwd"));
        assert!(!is_web_page("--help"));
        assert!(!is_web_page("https://a b"));
        assert!(!is_web_page(""));
    }
}
