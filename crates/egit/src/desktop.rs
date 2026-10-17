//! Handing things to the desktop: a web page to the user's browser, through the desktop's
//! opener (`xdg-open`, else `gio open`), off the interface thread; and the desktop's file
//! dialogs: where a file is saved, a folder, the files to open.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use egit_git::{Changes, ObjectId};
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

/// What the desktop's file dialog is asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FileQuestion {
    /// Where a repository's commit's patch goes (a file).
    SavePatch { repo: RepoId, commit: ObjectId },
    /// Where a repository's commits from one up to `HEAD` go as patches (a folder).
    SavePatches { repo: RepoId, from: ObjectId },
    /// Where a repository's uncommitted changes go as a patch (a file).
    SaveChanges { repo: RepoId, changes: Changes },
    /// Which patches to apply to a repository (files).
    ApplyPatches { repo: RepoId },
}

/// The desktop's file dialogs (`rfd`, one at a time): where a file is saved, a folder, the
/// files to open; or (for tests) a folder answering every question without asking.
#[derive(Debug)]
pub(crate) struct FileDialogs {
    /// For tests: the folder that answers (files saved in it, its patches opened).
    folder: Option<PathBuf>,
    /// The dialog, while it is open (and its answer, until taken).
    asking: Bind<Vec<PathBuf>, Problem>,
    /// What it was asked for.
    question: Option<FileQuestion>,
    /// The answer, once known.
    answered: Option<(FileQuestion, Vec<PathBuf>)>,
}

impl Default for FileDialogs {
    fn default() -> Self {
        Self {
            folder: None,
            asking: Bind::new(true),
            question: None,
            answered: None,
        }
    }
}

impl FileDialogs {
    /// Dialogs answered by `folder`, asking nothing: files are saved in it, patches opened from
    /// it (each it holds, by name).
    #[cfg(test)]
    pub(crate) fn in_folder(folder: PathBuf) -> Self {
        Self {
            folder: Some(folder),
            ..Self::default()
        }
    }

    /// Asks `question` (`title` on the dialog; `name`, a file offered), unless a dialog is
    /// open already.
    pub(crate) fn ask(&mut self, question: FileQuestion, title: &'static str, name: String) {
        if self.asking.is_pending() {
            return;
        }
        if let Some(folder) = &self.folder {
            let answer = match question {
                FileQuestion::SavePatch { .. } | FileQuestion::SaveChanges { .. } => {
                    vec![folder.join(name)]
                }
                FileQuestion::SavePatches { .. } => vec![folder.clone()],
                FileQuestion::ApplyPatches { .. } => patches_in(folder),
            };
            self.answered = Some((question, answer));
            return;
        }
        self.question = Some(question);
        self.asking.request(async move {
            let dialog = rfd::AsyncFileDialog::new().set_title(title);
            let chosen = match question {
                FileQuestion::SavePatch { .. } | FileQuestion::SaveChanges { .. } => dialog
                    .set_file_name(name)
                    .save_file()
                    .await
                    .into_iter()
                    .collect::<Vec<_>>(),
                FileQuestion::SavePatches { .. } => {
                    dialog.pick_folder().await.into_iter().collect()
                }
                FileQuestion::ApplyPatches { .. } => dialog
                    .add_filter("Patches", &["patch", "diff", "mbox", "eml"])
                    .add_filter("Any file", &["*"])
                    .pick_files()
                    .await
                    .unwrap_or_default(),
            };
            Ok(chosen.iter().map(|file| file.path().to_owned()).collect())
        });
    }

    /// The question answered and its answer, once given (none when the dialog was cancelled).
    pub(crate) fn answered(&mut self) -> Option<(FileQuestion, Vec<PathBuf>)> {
        if let Some(Ok(chosen)) = self.asking.take()
            && let Some(question) = self.question.take()
            && !chosen.is_empty()
        {
            self.answered = Some((question, chosen));
        }
        self.answered.take()
    }
}

/// The patches a test's folder holds, by name.
#[cfg(test)]
fn patches_in(folder: &std::path::Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(folder)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| {
                    egit_git::looks_like_patch(bstr::BStr::new(path.as_os_str().as_encoded_bytes()))
                })
                .collect()
        })
        .unwrap_or_default();
    found.sort();
    found
}

/// The patches a folder holds: only tests answer from a folder.
#[cfg(not(test))]
const fn patches_in(_folder: &std::path::Path) -> Vec<PathBuf> {
    Vec::new()
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
