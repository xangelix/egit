//! Handing things to the desktop: a web page to the user's browser, through the desktop's
//! opener (`xdg-open`, else `gio open`), off the interface thread; and where a file is saved,
//! asked in the desktop's save dialog.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use egit_git::ObjectId;
use egui_async::Bind;

use crate::jobs;
use crate::problem::Problem;
use crate::repo::RepoId;

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

/// What a file saved is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Saving {
    /// A repository's commit's patch.
    Patch { repo: RepoId, commit: ObjectId },
}

/// Where files are saved: chosen in the desktop's save dialog, one at a time, or (for tests)
/// put in a folder without asking.
#[derive(Debug)]
pub(crate) struct Saver {
    /// For tests: the folder files go into, without a dialog.
    folder: Option<PathBuf>,
    /// The dialog, while it is open (and its answer, until taken).
    asking: Bind<Option<PathBuf>, Problem>,
    /// What the file asked for is.
    saving: Option<Saving>,
    /// Where it goes, once known.
    chosen: Option<(Saving, PathBuf)>,
}

impl Default for Saver {
    fn default() -> Self {
        Self {
            folder: None,
            asking: Bind::new(true),
            saving: None,
            chosen: None,
        }
    }
}

impl Saver {
    /// A saver putting every file in `folder`, asking nothing.
    #[cfg(test)]
    pub(crate) fn into_folder(folder: PathBuf) -> Self {
        Self {
            folder: Some(folder),
            ..Self::default()
        }
    }

    /// Asks where the file `saving` is, offered as `name`, goes (unless the dialog is open
    /// for another).
    pub(crate) fn ask(&mut self, saving: Saving, title: &'static str, name: String) {
        if self.asking.is_pending() {
            return;
        }
        if let Some(folder) = &self.folder {
            self.chosen = Some((saving, folder.join(name)));
            return;
        }
        self.saving = Some(saving);
        self.asking.request(async move {
            let file = rfd::AsyncFileDialog::new()
                .set_title(title)
                .set_file_name(name)
                .save_file()
                .await;
            Ok(file.map(|file| file.path().to_owned()))
        });
    }

    /// The file asked for and where it goes, once chosen (`None` too when the dialog was
    /// cancelled).
    pub(crate) fn chosen(&mut self) -> Option<(Saving, PathBuf)> {
        if let Some(Ok(Some(path))) = self.asking.take()
            && let Some(saving) = self.saving.take()
        {
            self.chosen = Some((saving, path));
        }
        self.chosen.take()
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
