//! Git LFS in the commit table's place: whether git-lfs is installed and set up here (and how
//! to, when it isn't), moving its files (fetch, check out, pull, push, prune), the patterns it
//! tracks (tracking one more, or no longer), the files it holds at `HEAD` (whole here, only
//! downloaded, or a pointer), and the locks on its server. Escape or History goes back.

use std::fmt::Write as _;
use std::sync::Arc;

use bstr::ByteSlice as _;
use egit_git::{LfsFile, LfsLock, LfsTransfer};
use egui::{Frame, Margin, RichText, ScrollArea, Ui};
use egui_async::Bind;

use super::history::Shared;
use crate::format::{self, count};
use crate::lfs::{LfsState, LfsTask, LfsTrouble, read_all, read_lfs_files, read_lfs_locks};
use crate::problem::Problem;
use crate::tasks::{Task, remote_names};
use crate::theme::Weighted as _;
use crate::theme::icon;
use crate::widgets::{self, muted};

/// What the page asks of the history view.
#[derive(Debug)]
pub(super) enum LfsEvent {
    Close,
    Task(Task),
}

/// The LFS page's state.
#[derive(Debug)]
pub(super) struct LfsView {
    /// The whole of the repository's LFS, read for the page (the tab reads only where LFS is
    /// used).
    state: Bind<Arc<LfsState>, Problem>,
    /// The tab's read the page's was made after: when another lands (after a task, or the refs
    /// moved), the page reads again.
    state_for: Option<usize>,
    /// Whether the tab's moved on since the page's read began.
    stale: bool,
    files: Bind<Arc<[LfsFile]>, Problem>,
    /// The page's read the files were read for.
    files_for: usize,
    locks: Bind<Arc<[LfsLock]>, Problem>,
    /// A pattern being typed, to track.
    pattern: String,
    lockable: bool,
    /// A path being typed, to lock.
    lock_path: String,
}

impl Default for LfsView {
    fn default() -> Self {
        // Retained: egui-async drops what a `Bind` not drawn the frame before holds, aborting
        // a read under way, and it counts frames for the whole process.
        Self {
            state: Bind::new(true),
            state_for: None,
            stale: false,
            files: Bind::new(true),
            files_for: 0,
            locks: Bind::new(true),
            pattern: String::new(),
            lockable: false,
            lock_path: String::new(),
        }
    }
}

/// The height of a file's row.
const ROW: f32 = 24.0;

/// The most file rows shown before the list scrolls.
const ROWS_SHOWN: f32 = 12.0;

/// Draws the page; returns what it asked.
pub(super) fn show(
    ui: &mut Ui,
    shared: &Shared<'_>,
    lfs: Option<&Arc<LfsState>>,
    view: &mut LfsView,
) -> Option<LfsEvent> {
    let escape = !ui.ctx().text_edit_focused()
        && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
    let mut event = escape.then_some(LfsEvent::Close);
    // git-lfs's version, once the page's read has landed.
    let read = view.state.ok_ref().map(|state| state.version);
    widgets::bar(ui, |ui| {
        ui.horizontal(|ui| {
            if widgets::secondary(ui, true, &format!("{}  History", icon::ARROW_LEFT))
                .on_hover_text("Back to the history (Escape)")
                .clicked()
            {
                event = Some(LfsEvent::Close);
            }
            ui.label(RichText::new("Git LFS").semibold());
            if let Some(read) = &read {
                let version = read.map_or_else(
                    || "not installed".to_owned(),
                    |version| format!("git-lfs {version}"),
                );
                ui.label(muted(ui, version));
            }
        });
    });
    let tab = lfs.map(|lfs| Arc::as_ptr(lfs).addr());
    // A read under way finishes before the next starts (else reads replaced faster than they
    // finish would never land); one more follows when the tab's moved on meanwhile.
    if view.state_for != tab {
        view.state_for = tab;
        view.stale = true;
    }
    if (view.stale || view.state.is_idle())
        && !view.state.is_pending()
        && let Some(git) = shared.git
    {
        view.stale = false;
        view.state
            .request(read_all(git.clone(), Arc::clone(shared.repository)));
    }
    let lfs = match view.state.read_as_ref() {
        Some(Ok(lfs)) => Arc::clone(lfs),
        Some(Err(problem)) => {
            let problem = problem.clone();
            Frame::new()
                .inner_margin(Margin::symmetric(12, 16))
                .show(ui, |ui| widgets::problem_box(ui, &problem, |_| {}));
            return event;
        }
        None => {
            Frame::new()
                .inner_margin(Margin::symmetric(12, 16))
                .show(ui, |ui| widgets::working(ui, "Reading Git LFS…"));
            return event;
        }
    };
    let lfs = &lfs;
    let address = Arc::as_ptr(lfs).addr();
    if view.files_for != address
        && lfs.version.is_some()
        && let Some(git) = shared.git
    {
        view.files_for = address;
        view.files
            .request(read_lfs_files(git.clone(), Arc::clone(shared.repository)));
    }
    ScrollArea::vertical()
        .id_salt(("lfs-page", shared.id))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            Frame::new()
                .inner_margin(Margin::symmetric(12, 16))
                .show(ui, |ui| {
                    // A comfortable measure on a wide screen, the room there is on a narrow one.
                    ui.set_max_width(ui.available_width().min(860.0));
                    let mut ask = |task| event = Some(LfsEvent::Task(Task::Lfs(task)));
                    state_card(ui, shared, lfs, &mut ask);
                    if lfs.version.is_none() {
                        return;
                    }
                    ui.add_space(12.0);
                    transfers_card(ui, shared, lfs, &mut ask);
                    ui.add_space(12.0);
                    patterns_card(ui, shared, lfs, view, &mut ask);
                    ui.add_space(12.0);
                    files_card(ui, shared, view);
                    ui.add_space(12.0);
                    locks_card(ui, shared, view, &mut ask);
                });
        });
    event
}

/// Whether nothing stops a task now (none running, git-lfs set up), or why.
const fn ready(shared: &Shared<'_>, lfs: &LfsState) -> Result<(), &'static str> {
    if shared.busy {
        Err("Another task is running")
    } else if lfs.version.is_none() {
        Err("git-lfs isn't installed")
    } else if !lfs.set_up {
        Err("Set Git LFS up here first")
    } else {
        Ok(())
    }
}

/// What LFS is, and whether it is installed and set up here (with the way to, when it isn't).
fn state_card(ui: &mut Ui, shared: &Shared<'_>, lfs: &LfsState, ask: &mut impl FnMut(LfsTask)) {
    widgets::card(ui, |ui| {
        widgets::heading(ui, "Git LFS here");
        ui.add(
            egui::Label::new(muted(
                ui,
                "Git LFS commits a small pointer for each file a pattern tracks, and keeps the \
                 file itself in a store of its own on the remote's LFS server. Checking out \
                 turns pointers into files, through filters git lfs install sets up; without \
                 them, or before the files are downloaded, the work tree holds pointers.",
            ))
            .wrap(),
        );
        ui.add_space(8.0);
        let palette = shared.palette;
        let (said, color) = match (lfs.version, lfs.set_up, lfs.trouble()) {
            (None, _, Some(LfsTrouble::NotInstalled)) => (
                "This repository keeps files in Git LFS, but git-lfs isn't installed: those files \
                 are only pointers. Install it (the package git-lfs; Git for Windows has it), \
                 then read the repository again (⌘R).",
                palette.warn,
            ),
            (None, _, _) => (
                "git-lfs isn't installed. Install it (the package git-lfs; Git for Windows has \
                 it) to keep large files apart.",
                palette.muted,
            ),
            (Some(_), false, _) => (
                "Git LFS isn't set up here: its files check out as pointers, and pushes leave \
                 them behind. Setting it up adds its filters to this repository's configuration \
                 and its hooks.",
                palette.warn,
            ),
            (Some(_), true, _) => (
                "Set up here: tracked files check out whole, and pushes push them.",
                palette.ok,
            ),
        };
        ui.add(egui::Label::new(RichText::new(said).color(color)).wrap());
        if lfs.version.is_some() && !lfs.set_up {
            ui.add_space(8.0);
            if widgets::primary(ui, !shared.busy, "Set up Git LFS here").clicked() {
                ask(LfsTask::Install);
            }
        }
    });
}

/// Moving LFS files: fetch, check out, pull, push to each remote, prune.
fn transfers_card(ui: &mut Ui, shared: &Shared<'_>, lfs: &LfsState, ask: &mut impl FnMut(LfsTask)) {
    let allowed = ready(shared, lfs);
    let remotes = shared.config.map_or_default(remote_names);
    widgets::card(ui, |ui| {
        widgets::heading(ui, "Files");
        ui.add(
            egui::Label::new(muted(
                ui,
                "git pull and git checkout bring LFS files in with the commits when it is set \
                 up; these move them on their own: after cloning with them skipped, or to get \
                 every branch's.",
            ))
            .wrap(),
        );
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            // Each explained by LFS's own topic: its words are git's everyday ones.
            let offers = [
                (
                    "Pull",
                    "Download the current branch's files and check them out",
                    LfsTransfer::Pull,
                    "lfs-pull",
                ),
                (
                    "Fetch",
                    "Download the current branch's files, leaving the work tree as it is",
                    LfsTransfer::Fetch { all: false },
                    "lfs-fetch",
                ),
                (
                    "Fetch every file",
                    "Download the files every branch and commit holds: all history's",
                    LfsTransfer::Fetch { all: true },
                    "lfs-fetch-all",
                ),
                (
                    "Check out",
                    "Replace pointers in the work tree with files already downloaded",
                    LfsTransfer::Checkout,
                    "lfs-checkout",
                ),
            ];
            for (label, hover, transfer, topic) in offers {
                let offer = widgets::secondary(ui, allowed.is_ok(), label);
                crate::help::note_topic(ui, offer.rect, topic);
                if offer
                    .on_hover_text(hover)
                    .on_disabled_hover_text(allowed.err().unwrap_or_default())
                    .clicked()
                {
                    ask(LfsTask::Transfer(transfer));
                }
            }
            for remote in &remotes {
                let push = widgets::secondary(ui, allowed.is_ok(), &format!("Push to {remote}"));
                crate::help::note_topic(ui, push.rect, "lfs-push");
                if push
                    .on_hover_text(
                        "Upload the files every local branch holds to its LFS store (pushing \
                         commits does it for what they push)",
                    )
                    .clicked()
                {
                    ask(LfsTask::Transfer(LfsTransfer::Push {
                        remote: remote.as_str().into(),
                    }));
                }
            }
            if widgets::secondary(ui, allowed.is_ok(), "Prune")
                .on_hover_text(
                    "Delete downloaded files nothing recent needs: not the checkout's, recent \
                     branches', or commits not pushed yet",
                )
                .clicked()
            {
                ask(LfsTask::Prune);
            }
        });
    });
}

/// The patterns LFS tracks (and excludes), each to stop tracking; and one more to track.
fn patterns_card(
    ui: &mut Ui,
    shared: &Shared<'_>,
    lfs: &LfsState,
    view: &mut LfsView,
    ask: &mut impl FnMut(LfsTask),
) {
    let allowed = ready(shared, lfs);
    widgets::card(ui, |ui| {
        widgets::heading(ui, "Tracked patterns");
        if lfs.patterns.is_empty() {
            ui.label(muted(ui, "None yet: nothing here is kept in Git LFS."));
        }
        for pattern in &lfs.patterns {
            ui.horizontal(|ui| {
                let name = pattern.pattern.to_str_lossy();
                ui.label(RichText::new(name.as_ref()).monospace());
                let mut notes = vec![pattern.source.to_str_lossy().into_owned()];
                if pattern.lockable {
                    notes.push("lockable".to_owned());
                }
                if !pattern.tracked {
                    notes.push("excluded".to_owned());
                }
                ui.label(muted(ui, notes.join(" · ")));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if pattern.tracked
                        && widgets::secondary(ui, allowed.is_ok(), "Stop tracking")
                            .on_hover_text(
                                "Its line leaves .gitattributes; files committed through LFS \
                                 stay pointers until added again",
                            )
                            .clicked()
                    {
                        ask(LfsTask::Untrack(pattern.pattern.clone()));
                    }
                });
            });
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let label = ui.label(muted(ui, "Pattern"));
            ui.add(
                widgets::line_field(&mut view.pattern)
                    .hint_text("*.psd, assets/**")
                    .desired_width(220.0),
            )
            .labelled_by(label.id);
            widgets::checkbox(ui, &mut view.lockable, "Lockable").on_hover_text(
                "Files locked on the server before they are changed: read-only until then",
            );
            let pattern = view.pattern.trim();
            if widgets::secondary(ui, allowed.is_ok() && !pattern.is_empty(), "Track")
                .on_hover_text(
                    "Adds a line to .gitattributes: commit it. Files committed before stay in \
                     git until they are added again",
                )
                .clicked()
            {
                ask(LfsTask::Track {
                    pattern: pattern.to_owned(),
                    lockable: view.lockable,
                });
                view.pattern.clear();
            }
        });
    });
}

/// The files LFS holds at `HEAD`: how many, how large, and each one's state.
fn files_card(ui: &mut Ui, shared: &Shared<'_>, view: &mut LfsView) {
    widgets::card(ui, |ui| {
        let files = match view.files.read_as_ref() {
            None => {
                widgets::heading(ui, "Files held");
                widgets::working(ui, "Listing the LFS files…");
                return;
            }
            Some(Err(problem)) => {
                widgets::heading(ui, "Files held");
                let problem = problem.clone();
                widgets::problem_box(ui, &problem, |_| {});
                return;
            }
            Some(Ok(files)) => Arc::clone(files),
        };
        let size: u64 = files.iter().map(|file| file.size).sum();
        let missing = files.iter().filter(|file| !file.downloaded).count();
        let pointers = files.iter().filter(|file| !file.checked_out).count();
        let number = |n: usize| count(u64::try_from(n).unwrap_or(u64::MAX));
        widgets::heading(ui, &format!("Files held: {}", number(files.len())));
        let mut summary = format!("{} in all", format::size(size));
        if missing > 0 {
            let _ = write!(summary, " · {} not downloaded", number(missing));
        }
        if pointers > 0 {
            let _ = write!(summary, " · {} pointers in the work tree", number(pointers));
        }
        ui.label(muted(ui, summary));
        if files.is_empty() {
            return;
        }
        ui.add_space(6.0);
        let height =
            ROW * ROWS_SHOWN.min(f32::from(u16::try_from(files.len()).unwrap_or(u16::MAX)));
        ScrollArea::vertical()
            .id_salt(("lfs-files", shared.id))
            .max_height(height)
            .auto_shrink([false, true])
            .show_rows(ui, ROW, files.len(), |ui, rows| {
                for file in files.get(rows).unwrap_or_default() {
                    file_row(ui, shared, file);
                }
            });
    });
}

/// A file's row: whether it is here whole, its path, its size.
fn file_row(ui: &mut Ui, shared: &Shared<'_>, file: &LfsFile) {
    let palette = shared.palette;
    let (glyph, color, state) = match (file.checked_out, file.downloaded) {
        (true, _) => (icon::CHECK_CIRCLE, palette.ok, "here"),
        (false, true) => (
            icon::DOWNLOAD_SIMPLE,
            palette.warn,
            "downloaded; a pointer in the work tree",
        ),
        (false, false) => (icon::CLOUD, palette.muted, "not downloaded: a pointer"),
    };
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), ROW),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            // From the right: its size; then from the left, its state and path in the rest.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(muted(ui, format::size(file.size)));
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.label(RichText::new(glyph).color(color))
                        .on_hover_text(state);
                    ui.add(egui::Label::new(file.path.to_str_lossy().as_ref()).truncate());
                });
            });
        },
    );
}

/// The locks on the LFS server (read when asked: they need a server that keeps them), each
/// to unlock; and a file to lock.
fn locks_card(ui: &mut Ui, shared: &Shared<'_>, view: &mut LfsView, ask: &mut impl FnMut(LfsTask)) {
    widgets::card(ui, |ui| {
        widgets::heading(ui, "Locks");
        ui.add(
            egui::Label::new(muted(
                ui,
                "A lockable file is read-only until locked: whoever holds its lock may change \
                 and push it. Locks live on the LFS server (GitHub's and GitLab's keep them).",
            ))
            .wrap(),
        );
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if widgets::secondary(ui, !view.locks.is_pending(), "Read the locks").clicked()
                && let Some(git) = shared.git
            {
                view.locks
                    .request(read_lfs_locks(git.clone(), Arc::clone(shared.repository)));
            }
            let label = ui.label(muted(ui, "Lock"));
            ui.add(
                widgets::line_field(&mut view.lock_path)
                    .hint_text("art/hero.psd")
                    .desired_width(220.0),
            )
            .labelled_by(label.id);
            let path = view.lock_path.trim();
            if widgets::secondary(ui, !shared.busy && !path.is_empty(), "Lock").clicked() {
                ask(LfsTask::Lock(path.into()));
                view.lock_path.clear();
            }
        });
        let pending = view.locks.is_pending();
        match view.locks.read_as_ref() {
            None if pending => widgets::working(ui, "Reading the locks…"),
            None => {}
            Some(Err(problem)) => {
                let problem = problem.clone();
                widgets::problem_box(ui, &problem, |_| {});
            }
            Some(Ok(locks)) if locks.is_empty() => {
                ui.label(muted(ui, "No file is locked."));
            }
            Some(Ok(locks)) => {
                for lock in locks.iter() {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(lock.path.to_str_lossy().as_ref()).medium());
                        ui.label(muted(ui, format!("{} · {}", lock.owner, lock.locked_at)));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            for (label, force) in [("Force unlock", true), ("Unlock", false)] {
                                let unlock = widgets::secondary(ui, !shared.busy, label);
                                if !force {
                                    crate::help::note_topic(ui, unlock.rect, "lfs-unlock");
                                }
                                if unlock.clicked() {
                                    ask(LfsTask::Unlock {
                                        path: lock.path.clone(),
                                        force,
                                    });
                                }
                            }
                        });
                    });
                }
            }
        }
    });
}
