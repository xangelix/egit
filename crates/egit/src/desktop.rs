//! Handing things to the desktop: a web page to the user's browser, through the desktop's
//! opener (`xdg-open`, else `gio open`; macOS's `open`; Windows' URL handler), off the interface
//! thread; a file of the work tree opened in its app, or shown in its folder (the file
//! manager's `ShowItems` over D-Bus, `open -R`, `explorer /select`); and the desktop's file
//! dialogs: where a file is saved, a folder, the files to open.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

use bstr::{BStr, ByteSlice as _};
use egit_git::{Changes, ObjectId};
use egui_async::Bind;

use crate::jobs;
use crate::problem::Problem;
use crate::repo::RepoId;

/// Whether egit runs in a browser: no folders, dialogs, or ssh of its own (its relay's computer
/// has the folders, typed), and web pages opened by the page.
pub(crate) const IN_BROWSER: bool = cfg!(target_arch = "wasm32");

/// Where web pages and files go: the user's browser and file manager, or (for tests) a list.
#[derive(Debug, Default)]
pub(crate) struct Opener {
    recorded: Option<Vec<String>>,
    revealed: Option<Vec<(Reveal, PathBuf)>>,
}

/// What is done with a file or folder of the user's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reveal {
    /// Opened in the app the desktop opens it with (a folder in the file manager).
    Open,
    /// Shown in its folder, picked out (its folder opened, where that can't be).
    InFolder,
}

impl Opener {
    /// An opener that only lists the pages and files, starting nothing.
    #[cfg(test)]
    pub(crate) const fn recording() -> Self {
        Self {
            recorded: Some(Vec::new()),
            revealed: Some(Vec::new()),
        }
    }

    /// The pages listed, by a recording opener.
    #[cfg(test)]
    pub(crate) fn recorded(&self) -> &[String] {
        self.recorded.as_deref().unwrap_or_default()
    }

    /// The files listed, by a recording opener.
    #[cfg(test)]
    pub(crate) fn revealed(&self) -> &[(Reveal, PathBuf)] {
        self.revealed.as_deref().unwrap_or_default()
    }

    /// Opens `path` (absolute) in its app, or shows it in its folder.
    pub(crate) fn reveal(&mut self, path: PathBuf, how: Reveal) {
        if !path.is_absolute() {
            tracing::warn!(?path, "not opening a path that isn't absolute");
            return;
        }
        match &mut self.revealed {
            Some(revealed) => revealed.push((how, path)),
            None => jobs::detached(async move { reveal(&path, how) }),
        }
    }

    /// Opens `url` if it is a web page.
    pub(crate) fn open(&mut self, url: String) {
        if !is_web_page(&url) {
            tracing::warn!(url, "not opening what isn't a web page");
            return;
        }
        match &mut self.recorded {
            Some(recorded) => recorded.push(url),
            // In a tab of the browser egit is in.
            #[cfg(target_arch = "wasm32")]
            None => crate::web::open_page(&url),
            #[cfg(not(target_arch = "wasm32"))]
            None => jobs::detached(async move { open_web_page(&url) }),
        }
    }
}

/// What the desktop's file dialog is asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FileQuestion {
    /// Where a repository's commit's patch goes (a file).
    SavePatch { repo: RepoId, commit: ObjectId },
    /// Where a repository's commits from one up to `HEAD` go as patches (a folder).
    SavePatches { repo: RepoId, from: ObjectId },
    /// Where a repository's uncommitted changes go as a patch (a file).
    SaveChanges { repo: RepoId, changes: Changes },
    /// Which patches to apply to a repository (files).
    ApplyPatches { repo: RepoId },
    /// Which images to attach to an agent's prompt in a repository's tab (files).
    AttachImages { repo: RepoId },
    /// Where commits picked together go: their changes as one patch (`one`: a file), or one
    /// patch each (a folder).
    SavePicked {
        repo: RepoId,
        commits: Arc<[ObjectId]>,
        one: bool,
    },
    /// Where a pipeline's artifact goes (a file).
    SaveArtifact {
        repo: RepoId,
        wanted: Box<crate::downloads::ArtifactWanted>,
    },
    /// Which folder a new worktree goes into (a folder), for the dialog making it.
    WorktreeFolder { repo: RepoId },
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
            let answer = match &question {
                FileQuestion::SavePatch { .. }
                | FileQuestion::SaveChanges { .. }
                | FileQuestion::SaveArtifact { .. }
                | FileQuestion::SavePicked { one: true, .. } => vec![folder.join(name)],
                FileQuestion::SavePatches { .. }
                | FileQuestion::SavePicked { one: false, .. }
                | FileQuestion::WorktreeFolder { .. } => vec![folder.clone()],
                FileQuestion::ApplyPatches { .. } => patches_in(folder),
                FileQuestion::AttachImages { .. } => images_in(folder),
            };
            self.answered = Some((question, answer));
            return;
        }
        let kind = match &question {
            FileQuestion::SavePatch { .. }
            | FileQuestion::SaveChanges { .. }
            | FileQuestion::SaveArtifact { .. }
            | FileQuestion::SavePicked { one: true, .. } => Asked::File,
            FileQuestion::SavePatches { .. }
            | FileQuestion::SavePicked { one: false, .. }
            | FileQuestion::WorktreeFolder { .. } => Asked::Folder,
            FileQuestion::ApplyPatches { .. } => Asked::Patches,
            FileQuestion::AttachImages { .. } => Asked::Images,
        };
        self.question = Some(question);
        // A browser's are its own: files sent and received through the page (`web`).
        #[cfg(target_arch = "wasm32")]
        let _ = (kind, title, name);
        #[cfg(not(target_arch = "wasm32"))]
        self.asking.request(async move {
            let dialog = rfd::AsyncFileDialog::new().set_title(title);
            let chosen = match kind {
                Asked::File => dialog
                    .set_file_name(name)
                    .save_file()
                    .await
                    .into_iter()
                    .collect::<Vec<_>>(),
                Asked::Folder => dialog.pick_folder().await.into_iter().collect(),
                Asked::Patches => dialog
                    .add_filter("Patches", &["patch", "diff", "mbox", "eml"])
                    .add_filter("Any file", &["*"])
                    .pick_files()
                    .await
                    .unwrap_or_default(),
                Asked::Images => dialog
                    .add_filter("Images", &["png", "jpg", "jpeg", "gif", "webp"])
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

/// What a dialog asks for: a file to save, a folder, or patches or images to open.
#[derive(Clone, Copy, Debug)]
enum Asked {
    File,
    Folder,
    Patches,
    Images,
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

/// The images a test's folder holds, by name.
#[cfg(test)]
fn images_in(folder: &std::path::Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(folder)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| crate::attachments::is_image(path))
                .collect()
        })
        .unwrap_or_default();
    found.sort();
    found
}

/// The images a folder holds: only tests answer from a folder.
#[cfg(not(test))]
const fn images_in(_folder: &std::path::Path) -> Vec<PathBuf> {
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

/// The programs that open a web page in the user's browser, in turn: macOS's `open`, Windows'
/// URL handler (as `start` would, without a shell reading the address), or the desktop's
/// opener elsewhere.
const OPENERS: &[(&str, &[&str])] = if cfg!(target_os = "macos") {
    &[("open", &[])]
} else if cfg!(windows) {
    &[("rundll32", &["url.dll,FileProtocolHandler"])]
} else {
    &[("xdg-open", &[]), ("gio", &["open"])]
};

/// Opens `url` in the user's browser; a failure is logged (the desktop has no opener).
#[cfg(not(target_arch = "wasm32"))]
fn open_web_page(url: &str) {
    if !hand_over(OsStr::new(url)) {
        tracing::warn!(
            url,
            "the page couldn't be opened: the desktop has no opener"
        );
    }
}

/// Hands `target` (a page, or an absolute path) to the first of [`OPENERS`] that takes it.
fn hand_over(target: &OsStr) -> bool {
    for &(program, args) in OPENERS {
        let opened = Command::new(program)
            .args(args)
            .arg(target)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        match opened {
            Ok(status) if status.success() => return true,
            Ok(status) => tracing::warn!(program, %status, "the desktop didn't open it"),
            Err(error) => tracing::debug!(program, %error, "no desktop opener"),
        }
    }
    false
}

/// A path of the work tree (`/`-separated, as git writes it) on this system.
pub(crate) fn in_work_tree(work_tree: &Path, path: &BStr) -> PathBuf {
    path.split_str("/")
        .filter(|part| !part.is_empty())
        .fold(work_tree.to_owned(), |path, part| {
            path.join(part.to_os_str_lossy())
        })
}

/// Opens `path` in its app, or shows it in its folder; what is gone shows the nearest folder
/// still there. A failure is logged.
fn reveal(path: &Path, how: Reveal) {
    let (path, how) = if path.exists() {
        (path, how)
    } else {
        let Some(folder) = path.ancestors().find(|folder| folder.is_dir()) else {
            return;
        };
        (folder, Reveal::Open)
    };
    let done = match how {
        Reveal::Open => open_path(path),
        Reveal::InFolder => show_in_folder(path),
    };
    if !done {
        tracing::warn!(?path, ?how, "the desktop couldn't show it");
    }
}

/// Opens `path` in its app: the desktop's opener (Explorer on Windows, which opens a file
/// as a double click would).
fn open_path(path: &Path) -> bool {
    if cfg!(windows) {
        // Explorer's exit status says nothing: started is all there is to know.
        return Command::new("explorer").arg(path).spawn().is_ok();
    }
    hand_over(path.as_os_str())
}

/// Shows `path` picked out in its folder: Finder's `open -R`.
#[cfg(target_os = "macos")]
fn show_in_folder(path: &Path) -> bool {
    Command::new("open")
        .arg("-R")
        .arg(path)
        .status()
        .is_ok_and(|status| status.success())
}

/// Shows `path` picked out in its folder: `explorer /select,"<path>"` (Explorer reads that
/// argument itself, so it is passed as it is).
#[cfg(windows)]
fn show_in_folder(path: &Path) -> bool {
    use std::os::windows::process::CommandExt as _;
    let mut select = std::ffi::OsString::from("/select,\"");
    select.push(path);
    select.push("\"");
    Command::new("explorer").raw_arg(select).spawn().is_ok()
}

/// Shows `path` picked out in its folder: the file manager's `ShowItems` over D-Bus (through
/// `gdbus`, else `dbus-send`), else its folder opened.
#[cfg(not(any(target_os = "macos", windows)))]
fn show_in_folder(path: &Path) -> bool {
    let uri = file_uri(path);
    let asked: [(&str, Vec<String>); 2] = [
        (
            "gdbus",
            vec![
                "call".into(),
                "--session".into(),
                "--dest".into(),
                "org.freedesktop.FileManager1".into(),
                "--object-path".into(),
                "/org/freedesktop/FileManager1".into(),
                "--method".into(),
                "org.freedesktop.FileManager1.ShowItems".into(),
                format!("['{uri}']"),
                // The startup id: an empty string, as GVariant text.
                "''".into(),
            ],
        ),
        (
            "dbus-send",
            vec![
                "--session".into(),
                "--print-reply".into(),
                "--dest=org.freedesktop.FileManager1".into(),
                "/org/freedesktop/FileManager1".into(),
                "org.freedesktop.FileManager1.ShowItems".into(),
                format!("array:string:{uri}"),
                "string:".into(),
            ],
        ),
    ];
    for (program, args) in asked {
        let shown = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if shown.is_ok_and(|status| status.success()) {
            return true;
        }
    }
    path.parent()
        .is_some_and(|folder| hand_over(folder.as_os_str()))
}

/// `path` as a `file://` address, every byte but the unreserved and `/` escaped (so no quote
/// or comma reaches D-Bus's argument syntax).
#[cfg(not(any(target_os = "macos", windows)))]
fn file_uri(path: &Path) -> String {
    use std::fmt::Write as _;
    let mut uri = String::from("file://");
    for byte in path.as_os_str().as_encoded_bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(byte) {
            uri.push(char::from(*byte));
        } else {
            let _ = write!(uri, "%{byte:02X}");
        }
    }
    uri
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(any(target_os = "macos", windows)))]
    #[test]
    fn paths_become_file_addresses_escaped() {
        assert_eq!(
            file_uri(Path::new("/home/ada/my repo/a'b,c/日.rs")),
            "file:///home/ada/my%20repo/a%27b%2Cc/%E6%97%A5.rs"
        );
    }

    #[test]
    fn work_tree_paths_take_the_system_s_separators() {
        let root = Path::new("/repo");
        assert_eq!(
            in_work_tree(root, "src/core/a.c".into()),
            root.join("src").join("core").join("a.c")
        );
    }

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
