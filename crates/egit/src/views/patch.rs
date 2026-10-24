//! Patches to apply, looked at first, in the commit table's place: from files (picked, or
//! dropped on the window), pasted, or the one a stopped `git am` is at. What they hold (a
//! mailbox's commits, or a diff; the files they change, with their lines), whether they apply
//! here (`git apply --check`), how to apply them (as commits, signed off or not; as changes,
//! staged or not), and the patch itself, highlighted. Escape or History goes back.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use bstr::ByteSlice as _;
use egit_git::{
    Applies, ApplyAs, CancelToken, Git, PatchChange, PatchKind, PatchSummary, Repository,
    current_patch, summarize_patch,
};
use egit_text::Text;
use egui::{Frame, Key, Margin, Modifiers, RichText, ScrollArea, Ui};
use egui_async::Bind;

use super::history::Shared;
use crate::format::count;
use crate::grid::{Plain, TextGrid};
use crate::jobs;
use crate::problem::Problem;
use crate::repo::RepoId;
use crate::tasks::{Patch, Task};
use crate::theme::Weighted as _;
use crate::theme::icon;
use crate::widgets::{self, muted};

/// How long typing (pasting) must pause before the pasted patch is read, in seconds.
const PAUSE: f64 = 0.4;

/// The most commits and files listed above the patch (it shows them all).
const LISTED: usize = 200;

/// Where a patch comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PatchSource {
    /// Files, applied in order.
    Files(Vec<PathBuf>),
    /// Pasted into the view.
    Pasted,
    /// The patch a stopped `git am` is at (to look at, not apply).
    Stopped,
}

/// A patch read: its bytes, what it holds, and its text to show.
#[derive(Debug)]
pub(super) struct ReadPatch {
    bytes: Arc<[u8]>,
    summary: PatchSummary,
    text: Plain,
}

/// What the view asks of the history view.
#[derive(Debug)]
pub(super) enum PatchEvent {
    Close,
    Apply(Task),
}

/// The patch view's state.
#[derive(Debug)]
pub(super) struct PatchView {
    source: PatchSource,
    /// What is pasted, while pasting.
    pasted: String,
    /// When the pasted text last changed (egui's time), until it is read.
    edited: Option<f64>,
    read: Bind<Arc<ReadPatch>, Problem>,
    /// Whether the read was asked for.
    asked: bool,
    cancel: CancelToken,
    options: Options,
    grid: TextGrid,
}

/// How the patch is to be applied, as chosen.
#[derive(Clone, Copy, Debug)]
struct Options {
    /// A mailbox's commits as commits (else as changes).
    as_commits: bool,
    /// Commits with a `Signed-off-by` line.
    sign_off: bool,
    /// Changes staged (and merged in three ways where they don't apply as they are).
    staged: bool,
}

impl Drop for PatchView {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl PatchView {
    pub(super) fn new(source: PatchSource, repo: RepoId) -> Self {
        Self {
            source,
            pasted: String::new(),
            edited: None,
            read: Bind::new(true),
            asked: false,
            cancel: CancelToken::new(),
            options: Options {
                as_commits: true,
                sign_off: false,
                staged: true,
            },
            grid: TextGrid::new(("patch", repo)),
        }
    }

    /// Reads the patch (again), in a job.
    fn read(&mut self, git: &Git, repository: &Arc<Repository>) {
        self.cancel.cancel();
        self.cancel = CancelToken::new();
        self.asked = true;
        let source = self.source.clone();
        let pasted = self.pasted.clone();
        let (git, repository, cancel) = (git.clone(), Arc::clone(repository), self.cancel.clone());
        self.read.request(jobs::blocking(async move {
            let bytes = match source {
                PatchSource::Files(files) => read_files(&files)?,
                PatchSource::Pasted => with_final_newline(pasted.into_bytes()),
                PatchSource::Stopped => {
                    current_patch(&git, &repository).await.map_err(|error| {
                        Problem::git("The patch it stopped at couldn't be read", &error)
                    })?
                }
            };
            let bytes: Arc<[u8]> = Arc::from(bytes);
            let summary = summarize_patch(&git, &repository, &bytes, &cancel)
                .await
                .map_err(|error| Problem::git("This isn't a patch git can apply", &error))?;
            let text = Text::new(Arc::clone(&bytes)).map_err(|error| {
                Problem::new("The patch is too large to show", error.to_string())
            })?;
            let text = Plain::named(text, crate::preferences::text_options(), b"patch.patch");
            Ok(Arc::new(ReadPatch {
                bytes,
                summary,
                text,
            }))
        }));
    }
}

/// `files`' bytes, one after another (each ending its last line).
fn read_files(files: &[PathBuf]) -> Result<Vec<u8>, Problem> {
    let mut bytes = Vec::new();
    for file in files {
        let read = std::fs::read(file).map_err(|error| {
            Problem::new(
                "The patch couldn't be read",
                format!("{}: {error}.", file.display()),
            )
        })?;
        bytes.extend(with_final_newline(read));
    }
    Ok(bytes)
}

/// `bytes`, its last line ended.
fn with_final_newline(mut bytes: Vec<u8>) -> Vec<u8> {
    if !bytes.is_empty() && !bytes.ends_with(b"\n") {
        bytes.push(b'\n');
    }
    bytes
}

/// Draws the view in the commit table's place; returns what it asks.
pub(super) fn show(ui: &mut Ui, shared: &Shared<'_>, view: &mut PatchView) -> Option<PatchEvent> {
    let escape = !ui.ctx().text_edit_focused()
        && ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape));
    let mut event = escape.then_some(PatchEvent::Close);
    if let Some(git) = shared.git {
        start_reading(ui, view, git, shared.repository);
    }
    if let Some(asked) = bar(ui, shared, view) {
        event = Some(asked);
    }
    let read = view
        .read
        .read_as_ref()
        .map(|read| read.map(Arc::clone).map_err(Clone::clone));
    match read {
        None if view.source == PatchSource::Pasted && !view.asked => paste(ui, shared, view),
        None => {
            Frame::new()
                .inner_margin(Margin::symmetric(12, 16))
                .show(ui, |ui| widgets::working(ui, "Reading the patch…"));
        }
        Some(Err(problem)) => {
            Frame::new()
                .inner_margin(Margin::symmetric(12, 16))
                .show(ui, |ui| {
                    widgets::problem_box(ui, &problem, |ui| {
                        if view.source == PatchSource::Pasted
                            && widgets::secondary(ui, true, "Paste another").clicked()
                        {
                            view.asked = false;
                            view.read = Bind::new(true);
                        }
                    });
                });
        }
        Some(Ok(read)) => {
            if let Some(asked) = summary(ui, shared, view, &read) {
                event = Some(asked);
            }
            view.grid.show(ui, &read.text);
        }
    }
    event
}

/// Starts reading files, or a stopped `git am`'s patch, at once; a pasted patch once typing
/// pauses.
fn start_reading(ui: &Ui, view: &mut PatchView, git: &Git, repository: &Arc<Repository>) {
    match view.source {
        PatchSource::Files(_) | PatchSource::Stopped if !view.asked => view.read(git, repository),
        PatchSource::Pasted => {
            let Some(edited) = view.edited else {
                return;
            };
            let waited = ui.input(|input| input.time) - edited;
            if view.pasted.trim().is_empty() {
                view.edited = None;
            } else if waited >= PAUSE {
                view.edited = None;
                view.read(git, repository);
            } else {
                ui.ctx()
                    .request_repaint_after(Duration::from_secs_f64(PAUSE - waited));
            }
        }
        PatchSource::Files(_) | PatchSource::Stopped => {}
    }
}

/// The bar: the way back, what is applied to what, and where it comes from.
fn bar(ui: &mut Ui, shared: &Shared<'_>, view: &PatchView) -> Option<PatchEvent> {
    let mut event = None;
    let branch = shared.branch().map_or_else(
        || "HEAD".to_owned(),
        |branch| branch.to_str_lossy().into_owned(),
    );
    widgets::bar(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            if widgets::secondary(ui, true, &format!("{}  History", icon::ARROW_LEFT)).clicked() {
                event = Some(PatchEvent::Close);
            }
            let title = match &view.source {
                PatchSource::Files(_) => format!("Apply patches to {branch}"),
                PatchSource::Pasted => format!("Apply a pasted patch to {branch}"),
                PatchSource::Stopped => "The patch git am stopped at".to_owned(),
            };
            ui.label(RichText::new(title).semibold());
            if let PatchSource::Files(files) = &view.source {
                let names: Vec<String> = files
                    .iter()
                    .map(|file| {
                        file.file_name()
                            .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
                    })
                    .collect();
                let paths: Vec<String> = files
                    .iter()
                    .map(|file| file.display().to_string())
                    .collect();
                ui.label(muted(ui, names.join(", ")))
                    .on_hover_text(paths.join("\n"));
            }
        });
    });
    event
}

/// Where a patch is pasted.
fn paste(ui: &mut Ui, shared: &Shared<'_>, view: &mut PatchView) {
    Frame::new().inner_margin(Margin::same(12)).show(ui, |ui| {
        let label = ui.label(muted(
            ui,
            "Paste a patch: commits by mail (git format-patch's), or a diff.",
        ));
        let most = (ui.available_height() - 80.0).max(200.0);
        let field = crate::widgets::bounded_text_area(
            ui,
            crate::widgets::text_area(&mut view.pasted)
                .code_editor()
                .desired_rows(20)
                .desired_width(f32::INFINITY)
                .hint_text("From 1234abcd… or diff --git a/…"),
            most,
            "pasted-patch",
        )
        .labelled_by(label.id);
        if field.changed() {
            view.edited = Some(ui.input(|input| input.time));
        }
        if !view.pasted.trim().is_empty() {
            ui.label(RichText::new("Reading it once you stop typing…").color(shared.palette.muted));
        }
    });
}

/// What the patch holds, whether it applies, how to apply it, and Apply.
fn summary(
    ui: &mut Ui,
    shared: &Shared<'_>,
    view: &mut PatchView,
    read: &ReadPatch,
) -> Option<PatchEvent> {
    let mut event = None;
    let summary = &read.summary;
    Frame::new()
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.label(RichText::new(held(summary)).semibold());
            applies_line(ui, shared, &summary.applies);
            ScrollArea::vertical()
                .id_salt(("patch-lists", shared.id))
                .max_height(180.0)
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    commits_list(ui, summary);
                    files_list(ui, shared, summary);
                });
            if view.source == PatchSource::Stopped {
                return;
            }
            ui.add_space(6.0);
            if summary.kind == PatchKind::Mailbox {
                ui.horizontal_wrapped(|ui| {
                    crate::widgets::radio_value(
                        ui,
                        &mut view.options.as_commits,
                        true,
                        "As commits, keeping their authors and messages",
                    );
                    crate::widgets::radio_value(
                        ui,
                        &mut view.options.as_commits,
                        false,
                        "As changes to commit yourself",
                    );
                });
            }
            let as_commits = view.options.as_commits && summary.kind == PatchKind::Mailbox;
            if as_commits {
                crate::widgets::checkbox(
                    ui,
                    &mut view.options.sign_off,
                    "Add a Signed-off-by line to each",
                );
            } else {
                crate::widgets::checkbox(
                    ui,
                    &mut view.options.staged,
                    "Stage them (merging in three ways where they don't apply as they are)",
                );
            }
            let allowed = if shared.busy {
                Err("Another task is running")
            } else if shared.operation.is_some() {
                Err("Finish or abort the operation in progress first")
            } else if let Applies::Not(_) = summary.applies {
                Err("git can't apply it here")
            } else {
                Ok(())
            };
            let branch = shared.branch().map_or_else(
                || "HEAD".to_owned(),
                |branch| branch.to_str_lossy().into_owned(),
            );
            ui.add_space(4.0);
            let apply = widgets::primary(ui, allowed.is_ok(), &format!("Apply to {branch}"));
            let apply = match allowed {
                Err(why) => apply.on_disabled_hover_text(why),
                Ok(()) => apply,
            };
            if apply.clicked() {
                let how = if as_commits {
                    ApplyAs::Commits {
                        sign_off: view.options.sign_off,
                    }
                } else {
                    ApplyAs::Changes {
                        staged: view.options.staged,
                    }
                };
                event = Some(PatchEvent::Apply(Task::ApplyPatch {
                    patch: Patch(Arc::clone(&read.bytes)),
                    how,
                    commits: summary.commit_count,
                }));
            }
        });
    event
}

/// What a patch holds, in a line: "3 commits by mail, changing 2 files".
fn held(summary: &PatchSummary) -> String {
    let files = match summary.files.len() {
        1 => "1 file".to_owned(),
        files => format!("{} files", count(u64::try_from(files).unwrap_or(u64::MAX))),
    };
    match (summary.kind, summary.commit_count) {
        (PatchKind::Mailbox, 1) => format!("1 commit by mail, changing {files}"),
        (PatchKind::Mailbox, commits) => format!(
            "{} commits by mail, changing {files}",
            count(u64::try_from(commits).unwrap_or(u64::MAX))
        ),
        (PatchKind::Diff, _) => format!("Changes to {files}"),
    }
}

/// Whether it applies, in words and a colour.
fn applies_line(ui: &mut Ui, shared: &Shared<'_>, applies: &Applies) {
    let palette = shared.palette;
    let (text, color, said) = match applies {
        Applies::Cleanly => (
            format!("{}  Applies cleanly here.", icon::CHECK_CIRCLE),
            palette.ok,
            None,
        ),
        Applies::Merging { conflicts: false } => (
            format!(
                "{}  Doesn't apply as it is, but merges in cleanly (three ways).",
                icon::CHECK_CIRCLE
            ),
            palette.ok,
            None,
        ),
        Applies::Merging { conflicts: true } => (
            format!(
                "{}  Merges in with conflicts to resolve in the merge editor.",
                icon::WARNING
            ),
            palette.warn,
            None,
        ),
        Applies::Not(said) => (
            format!("{}  git can't apply it here.", icon::X_CIRCLE),
            palette.danger,
            Some(said.clone()),
        ),
        Applies::Unchecked => (
            "A series: each commit is checked as it is applied.".to_owned(),
            palette.muted,
            None,
        ),
    };
    ui.label(RichText::new(text).color(color));
    if let Some(said) = said {
        ui.label(muted(ui, said));
    }
}

/// A mailbox's commits.
fn commits_list(ui: &mut Ui, summary: &PatchSummary) {
    for (number, commit) in (1_u64..).zip(summary.commits.iter().take(LISTED)) {
        ui.horizontal_wrapped(|ui| {
            ui.label(muted(ui, format!("{number}.")));
            ui.label(&commit.subject);
            ui.label(muted(ui, format!("{} · {}", commit.author, commit.date)));
        });
    }
    let more = summary.commit_count.saturating_sub(LISTED);
    if more > 0 {
        ui.label(muted(
            ui,
            format!(
                "and {} more",
                count(u64::try_from(more).unwrap_or(u64::MAX))
            ),
        ));
    }
}

/// The files a patch changes, with their lines.
fn files_list(ui: &mut Ui, shared: &Shared<'_>, summary: &PatchSummary) {
    let palette = shared.palette;
    ui.add_space(4.0);
    for file in summary.files.iter().take(LISTED) {
        ui.horizontal_wrapped(|ui| {
            let word = match &file.change {
                PatchChange::Added => "Added",
                PatchChange::Deleted => "Deleted",
                PatchChange::Renamed { .. } => "Renamed",
                PatchChange::Modified => "Modified",
            };
            crate::tables::status_mark(ui, word, palette);
            ui.label(file.path.to_str_lossy());
            if let PatchChange::Renamed { from } = &file.change {
                ui.label(muted(ui, format!("from {}", from.to_str_lossy())));
            }
            match file.lines {
                Some((added, removed)) => {
                    ui.label(RichText::new(format!("+{}", count(added))).color(palette.ok));
                    ui.label(RichText::new(format!("−{}", count(removed))).color(palette.danger));
                }
                None => {
                    ui.label(muted(ui, "binary"));
                }
            }
        });
    }
    let more = summary.files.len().saturating_sub(LISTED);
    if more > 0 {
        ui.label(muted(
            ui,
            format!(
                "and {} more files",
                count(u64::try_from(more).unwrap_or(u64::MAX))
            ),
        ));
    }
}
