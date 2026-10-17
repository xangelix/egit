//! Exchanging with the remotes, as the history shows it: the branch, its upstream and how they
//! compare, Fetch, Pull, and Push (what the running one is doing, with Cancel); how the last one
//! ended, in words a person can act on; the dialogs it may lead to (diverged branches, forcing a
//! push, a branch's first push, giving up an operation); and the banner of an operation git
//! stopped in the middle of.

use std::fmt::Write as _;

use bstr::ByteSlice as _;
use egit_git::{
    ConfigSnapshot, Head, Operation, PullMode, Pulled, PushFlag, PushOptions, Pushed, Tracking,
};
use egui::{Align, Id, Layout, Modal, RichText, TextStyle, Ui, UiBuilder};

use crate::app::Action;
use crate::problem::Problem;
use crate::repo::RepoId;
use crate::sync::{SyncOutcome, SyncTask, Syncing, has_remotes, remote_names, upstream_of};
use crate::theme::{Palette, icon};
use crate::widgets::{self, muted};
use crate::{fonts, format};

/// What the sync parts remember between frames.
#[derive(Debug, Default)]
pub(crate) struct SyncView {
    dialog: Option<Dialog>,
    /// The task whose outcome was taken in (dismissed, or turned into a dialog).
    seen: u64,
    /// The diverged-branches dialog's "remember" box.
    remember: bool,
}

/// A question the sync parts are asking.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Dialog {
    /// How to bring diverged branches together.
    Diverged,
    /// Whether to replace the remote's branch.
    ForcePush(PushOptions),
    /// Where a branch without an upstream goes.
    FirstPush(PushOptions),
    /// Whether to give up an operation.
    Abort(Operation),
}

/// What the sync parts work with while they are drawn.
pub(crate) struct SyncPane<'a> {
    pub(crate) id: RepoId,
    pub(crate) head: Option<&'a Head>,
    pub(crate) operations: &'a [Operation],
    pub(crate) config: Option<&'a ConfigSnapshot>,
    pub(crate) syncing: &'a mut Syncing,
    pub(crate) actions: &'a mut Vec<Action>,
}

/// What the user asked for that the history view does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SyncEvent {
    /// Show the working tree (its conflicts).
    ShowWorkTree,
}

impl SyncPane<'_> {
    /// The current branch's name, when on one.
    fn branch(&self) -> Option<String> {
        match self.head? {
            Head::Branch(branch) => Some(branch.name.to_string()),
            Head::Detached(_) => None,
        }
    }

    /// Where the current branch pushes to: its upstream, when it has one.
    fn push_target(&self) -> Option<PushOptions> {
        let branch = self.branch()?;
        let (remote, remote_branch) = upstream_of(self.config?, &branch)?;
        Some(PushOptions {
            remote: remote.into(),
            branch: branch.into(),
            remote_branch: remote_branch.into(),
            set_upstream: false,
            force: false,
        })
    }

    fn remotes(&self) -> Vec<String> {
        self.config.map(remote_names).unwrap_or_default()
    }

    fn start(&mut self, task: SyncTask) {
        self.actions.push(Action::Sync(self.id, task));
    }
}

/// The branch, how it compares with its upstream, and Fetch, Pull, Push (or, while one runs,
/// what it is doing and Cancel), from the right.
pub(crate) fn bar(ui: &mut Ui, view: &mut SyncView, pane: &mut SyncPane<'_>) {
    let right = UiBuilder::new().layout(Layout::right_to_left(Align::Center));
    own_ids(ui, "bar", pane.id, right, |ui| controls(ui, view, pane));
}

/// Draws `add` in a `Ui` with an id of its own, so its controls' ids are counted from it, not
/// from whatever the row or panel drew before (which comes and goes): a click pressed on one
/// id and released on another is lost.
fn own_ids<R>(
    ui: &mut Ui,
    part: &str,
    id: RepoId,
    builder: UiBuilder,
    add: impl FnOnce(&mut Ui) -> R,
) -> R {
    ui.scope_builder(builder.id(Id::new(("sync", part, id))), add)
        .inner
}

fn controls(ui: &mut Ui, view: &mut SyncView, pane: &mut SyncPane<'_>) {
    let palette = Palette::of(ui.ctx());
    if pane.syncing.is_busy() {
        // The progress moves on its own, without input.
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(100));
        if widgets::secondary(ui, true, "Cancel").clicked() {
            pane.actions.push(Action::CancelSync(pane.id));
        }
        ui.label(muted(ui, progress_text(pane.syncing)));
        ui.spinner();
        return;
    }
    let has_remotes = pane.config.is_some_and(has_remotes);
    let on_branch = matches!(pane.head, Some(Head::Branch(_)));
    let push = widgets::secondary(
        ui,
        on_branch && has_remotes,
        &format!("{}  Push", icon::ARROW_UP),
    );
    if push.clicked() {
        match (pane.push_target(), pane.branch()) {
            (Some(target), _) => pane.start(SyncTask::Push(target)),
            (None, Some(branch)) => {
                let remotes = pane.remotes();
                let remote = remotes
                    .iter()
                    .find(|remote| *remote == "origin")
                    .or_else(|| remotes.first())
                    .cloned()
                    .unwrap_or_default();
                view.dialog = Some(Dialog::FirstPush(PushOptions {
                    remote: remote.into(),
                    branch: branch.clone().into(),
                    remote_branch: branch.into(),
                    set_upstream: true,
                    force: false,
                }));
            }
            (None, None) => {}
        }
    }
    let has_upstream = matches!(pane.head, Some(Head::Branch(branch)) if branch.upstream.is_some());
    if widgets::secondary(ui, has_upstream, &format!("{}  Pull", icon::ARROW_DOWN))
        .on_disabled_hover_text("The branch has no upstream to pull from")
        .clicked()
    {
        pane.start(SyncTask::Pull(PullMode::AsConfigured));
    }
    if widgets::secondary(
        ui,
        has_remotes,
        &format!("{}  Fetch", icon::ARROWS_CLOCKWISE),
    )
    .on_disabled_hover_text("The repository has no remote")
    .clicked()
    {
        pane.start(SyncTask::Fetch);
    }
    if let Some(problem) = &pane.syncing.background_problem {
        ui.label(RichText::new(icon::WARNING).color(palette.warn))
            .on_hover_text(format!("Fetching in the background failed: {problem}"));
    }
    ui.add_space(8.0);
    branch_label(ui, pane.head, palette);
}

/// The branch and how it compares with its upstream: `main → origin/main ↑1 ↓2`.
fn branch_label(ui: &mut Ui, head: Option<&Head>, palette: Palette) {
    let text = match head {
        Some(Head::Branch(branch)) => {
            let name = branch.name.to_str_lossy();
            branch.upstream.as_ref().map_or_else(
                || format!("{}  {name} (not pushed yet)", icon::GIT_BRANCH),
                |upstream| {
                    format!(
                        "{}  {name} {} {}{}",
                        icon::GIT_BRANCH,
                        icon::ARROW_RIGHT,
                        upstream.name.to_str_lossy(),
                        compared(upstream.tracking)
                    )
                },
            )
        }
        Some(Head::Detached(commit)) => format!(
            "{}  Detached at {}",
            icon::GIT_COMMIT,
            commit.id.abbreviated(8)
        ),
        None => return,
    };
    fonts::note_text(ui, &TextStyle::Body.resolve(ui.style()), &text);
    ui.label(RichText::new(text).color(palette.muted));
}

/// How a branch compares with its upstream: ` ↑1 ↓2`, nothing when even, ` (gone)`.
fn compared(tracking: Tracking) -> String {
    let mut text = String::new();
    match tracking {
        Tracking::Gone => text.push_str(" (gone)"),
        Tracking::Diverged { ahead, behind } => {
            if ahead > 0 {
                let _ = write!(text, " {}{ahead}", icon::ARROW_UP);
            }
            if behind > 0 {
                let _ = write!(text, " {}{behind}", icon::ARROW_DOWN);
            }
        }
    }
    text
}

/// What the running task is doing.
fn progress_text(syncing: &Syncing) -> String {
    let doing = syncing.task.as_ref().map_or("Working", SyncTask::doing);
    format::progress(doing, &syncing.progress.report())
}

/// How the last task ended (until dismissed or another starts): a line, a problem, or the
/// dialog it leads to.
pub(crate) fn outcome(ui: &mut Ui, view: &mut SyncView, pane: &mut SyncPane<'_>) {
    own_ids(ui, "outcome", pane.id, UiBuilder::new(), |ui| {
        outcome_lines(ui, view, pane);
    });
}

fn outcome_lines(ui: &mut Ui, view: &mut SyncView, pane: &mut SyncPane<'_>) {
    if pane.syncing.is_busy() || view.seen == pane.syncing.started {
        return;
    }
    let palette = Palette::of(ui.ctx());
    let started = pane.syncing.started;
    let result = pane
        .syncing
        .result
        .read_as_ref()
        .map(|result| result.cloned().map_err(Clone::clone));
    let mut dismiss = false;
    match result {
        None => {}
        Some(Err(problem)) => {
            ui.add_space(4.0);
            widgets::problem_box(ui, &problem, |ui| {
                if widgets::secondary(ui, true, "Dismiss").clicked() {
                    dismiss = true;
                }
            });
        }
        Some(Ok(SyncOutcome::Pulled(Pulled::Diverged))) => {
            view.dialog = Some(Dialog::Diverged);
            view.remember = false;
            dismiss = true;
        }
        Some(Ok(SyncOutcome::Pushed(pushed))) if !pushed.succeeded() => {
            refused(ui, view, pane, &pushed, &mut dismiss);
        }
        Some(Ok(outcome)) => {
            let (text, color) = match &outcome {
                SyncOutcome::Cancelled => ("Cancelled.".to_owned(), palette.muted),
                SyncOutcome::Fetched => ("Fetched.".to_owned(), palette.ok),
                SyncOutcome::Pulled(Pulled::Updated) => ("Pulled.".to_owned(), palette.ok),
                SyncOutcome::Pulled(_) => (
                    "The pull stopped at conflicts: resolve them, stage them, then Continue."
                        .to_owned(),
                    palette.ok,
                ),
                SyncOutcome::Pushed(pushed) => (pushed_text(pushed), palette.ok),
                SyncOutcome::Aborted => (
                    "Aborted: everything is as it was before.".to_owned(),
                    palette.ok,
                ),
                SyncOutcome::Continued => ("Carried on.".to_owned(), palette.ok),
            };
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(text).color(color));
                if widgets::secondary(ui, true, "Dismiss").clicked() {
                    dismiss = true;
                }
            });
            if let SyncOutcome::Pushed(pushed) = &outcome {
                notes(ui, pushed);
            }
        }
    }
    if dismiss {
        view.seen = started;
    }
}

/// What a push that went through did.
fn pushed_text(pushed: &Pushed) -> String {
    let Some(first) = pushed.refs.first() else {
        return "Pushed.".to_owned();
    };
    let to = first
        .to
        .strip_prefix(b"refs/heads/")
        .unwrap_or(&first.to)
        .to_str_lossy()
        .into_owned();
    match first.flag {
        PushFlag::UpToDate => format!("{to} was already up to date."),
        PushFlag::New => format!("Pushed: {to} is new on the remote."),
        PushFlag::Forced => format!("Force pushed: {to} replaced."),
        _ => format!("Pushed to {to}."),
    }
}

/// What the remote said besides (a link to open a pull request, say).
fn notes(ui: &mut Ui, pushed: &Pushed) {
    let said: Vec<&str> = pushed
        .notes
        .iter()
        .filter_map(|note| note.strip_prefix("remote: "))
        .map(str::trim)
        .filter(|note| !note.is_empty())
        .collect();
    if !said.is_empty() {
        ui.add(egui::Label::new(muted(ui, said.join("\n")).small()).selectable(true));
    }
}

/// A refused push: why, in words, and what can be done.
fn refused(
    ui: &mut Ui,
    view: &mut SyncView,
    pane: &mut SyncPane<'_>,
    pushed: &Pushed,
    dismiss: &mut bool,
) {
    let refusal = pushed
        .refs
        .iter()
        .find(|pushed| pushed.flag == PushFlag::Rejected);
    let reason = refusal
        .and_then(|refused| refused.reason.as_ref())
        .map(|reason| reason.to_str_lossy().into_owned())
        .unwrap_or_default();
    let behind = matches!(reason.as_str(), "fetch first" | "non-fast-forward");
    let explanation = match reason.as_str() {
        "fetch first" => {
            "The remote has commits your branch hasn't. Pull them first, or force push to \
             replace them."
                .to_owned()
        }
        "non-fast-forward" => {
            "Your branch and the remote's have diverged (an amended or rebased commit, say). \
             Pull to bring them together, or force push to replace the remote's."
                .to_owned()
        }
        "stale info" => {
            "The remote moved on since you last fetched, so nothing was forced: fetch, look at \
             what changed, then decide."
                .to_owned()
        }
        "" => "The remote refused the push.".to_owned(),
        other => format!("The remote refused the push: {other}."),
    };
    let problem = Problem::new("The push was refused", explanation);
    let target = pane.push_target().or_else(|| {
        pane.syncing.task.as_ref().and_then(|task| match task {
            SyncTask::Push(options) => Some(options.clone()),
            _ => None,
        })
    });
    widgets::problem_box(ui, &problem, |ui| {
        if behind {
            if let Some(target) = &target
                && widgets::secondary(ui, true, "Force push…").clicked()
            {
                view.dialog = Some(Dialog::ForcePush(PushOptions {
                    force: true,
                    ..target.clone()
                }));
                *dismiss = true;
            }
            if widgets::primary(ui, true, "Pull").clicked() {
                pane.start(SyncTask::Pull(PullMode::AsConfigured));
                *dismiss = true;
            }
        }
        if widgets::secondary(ui, true, "Dismiss").clicked() {
            *dismiss = true;
        }
    });
}

/// The banner of an operation git stopped in the middle of: Continue and Abort (or, for a
/// bisect, Stop).
pub(crate) fn banner(
    ui: &mut Ui,
    view: &mut SyncView,
    pane: &mut SyncPane<'_>,
) -> Option<SyncEvent> {
    own_ids(ui, "banner", pane.id, UiBuilder::new(), |ui| {
        operation_banner(ui, view, pane)
    })
}

fn operation_banner(
    ui: &mut Ui,
    view: &mut SyncView,
    pane: &mut SyncPane<'_>,
) -> Option<SyncEvent> {
    let &operation = pane.operations.first()?;
    let palette = Palette::of(ui.ctx());
    let busy = pane.syncing.is_busy();
    let what = operation_name(operation);
    let mut event = None;
    egui::Frame::new()
        .fill(palette.warn.gamma_multiply(0.12))
        .stroke(egui::Stroke::new(1.0, palette.warn))
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(format!("{}  {what} is in progress.", icon::WARNING))
                        .color(palette.warn)
                        .strong(),
                );
                if operation == Operation::Bisect {
                    ui.label("Mark commits good or bad in a terminal, or stop.");
                    if widgets::secondary(ui, !busy, "Stop bisecting").clicked() {
                        pane.start(SyncTask::Abort(operation));
                    }
                    return;
                }
                ui.label(
                    "Resolve the conflicts (the files marked Conflict), stage them, then \
                     Continue; or Abort to go back to how things were.",
                );
                if widgets::secondary(ui, true, "Show the conflicts").clicked() {
                    event = Some(SyncEvent::ShowWorkTree);
                }
                if widgets::primary(ui, !busy, "Continue").clicked() {
                    pane.start(SyncTask::Continue(operation));
                }
                if widgets::secondary(ui, !busy, "Abort…").clicked() {
                    view.dialog = Some(Dialog::Abort(operation));
                }
            });
        });
    event
}

/// An operation, as a sentence starts with it.
const fn operation_name(operation: Operation) -> &'static str {
    match operation {
        Operation::Merge => "A merge",
        Operation::Rebase => "A rebase",
        Operation::ApplyMailbox => "Applying patches",
        Operation::CherryPick => "A cherry-pick",
        Operation::Revert => "A revert",
        Operation::Bisect => "A bisect",
    }
}

/// The dialog asked, if any.
pub(crate) fn dialogs(ctx: &egui::Context, view: &mut SyncView, pane: &mut SyncPane<'_>) {
    let Some(dialog) = view.dialog.clone() else {
        return;
    };
    let palette = Palette::of(ctx);
    let frame = egui::Frame::new()
        .fill(palette.surface)
        .stroke(egui::Stroke::new(1.0, palette.border))
        .corner_radius(egui::CornerRadius::same(12))
        .inner_margin(egui::Margin::same(20));
    let mut close = false;
    let shown = Modal::new(Id::new(("sync-dialog", pane.id)))
        .frame(frame)
        .show(ctx, |ui| {
            ui.set_width(480.0);
            match dialog {
                Dialog::Diverged => diverged(ui, view, pane, &mut close),
                Dialog::ForcePush(options) => force_push(ui, pane, &options, &mut close),
                Dialog::FirstPush(options) => first_push(ui, view, pane, options, &mut close),
                Dialog::Abort(operation) => abort(ui, pane, operation, &mut close),
            }
        });
    if close || shown.should_close() {
        view.dialog = None;
    }
}

/// A dialog's title.
fn title(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(18.0).strong());
    ui.add_space(8.0);
}

/// How to bring diverged branches together.
fn diverged(ui: &mut Ui, view: &mut SyncView, pane: &mut SyncPane<'_>, close: &mut bool) {
    let upstream = match pane.head {
        Some(Head::Branch(branch)) => branch
            .upstream
            .as_ref()
            .map(|upstream| upstream.name.to_str_lossy().into_owned()),
        _ => None,
    }
    .unwrap_or_else(|| "its upstream".to_owned());
    title(ui, "The branches have diverged");
    ui.add(
        egui::Label::new(format!(
            "Your branch and {upstream} each have commits the other hasn't. Bring them together \
             by putting your commits after theirs (rebase), or by joining both in a merge commit."
        ))
        .wrap(),
    );
    ui.add_space(8.0);
    ui.checkbox(
        &mut view.remember,
        "Remember this for the repository (pull.rebase)",
    );
    ui.add_space(8.0);
    widgets::buttons_row(ui, |ui| {
        if widgets::primary(ui, true, "Rebase").clicked() {
            let task = if view.remember {
                SyncTask::PullAndRemember { rebase: true }
            } else {
                SyncTask::Pull(PullMode::Rebase)
            };
            pane.start(task);
            *close = true;
        }
        if widgets::secondary(ui, true, "Merge").clicked() {
            let task = if view.remember {
                SyncTask::PullAndRemember { rebase: false }
            } else {
                SyncTask::Pull(PullMode::Merge)
            };
            pane.start(task);
            *close = true;
        }
        if widgets::secondary(ui, true, "Cancel").clicked() {
            *close = true;
        }
    });
}

/// Whether to replace the remote's branch.
fn force_push(ui: &mut Ui, pane: &mut SyncPane<'_>, options: &PushOptions, close: &mut bool) {
    title(
        ui,
        &format!(
            "Force push to {}/{}?",
            options.remote.to_str_lossy(),
            options.remote_branch.to_str_lossy()
        ),
    );
    ui.add(
        egui::Label::new(
            "This replaces the remote's branch with yours: its commits that yours hasn't go from \
             it. egit forces only if the remote is still where you last fetched it, and that is \
             in your branch; otherwise nothing is pushed.",
        )
        .wrap(),
    );
    ui.add_space(12.0);
    widgets::buttons_row(ui, |ui| {
        if widgets::primary(ui, true, "Force push").clicked() {
            pane.start(SyncTask::Push(options.clone()));
            *close = true;
        }
        if widgets::secondary(ui, true, "Cancel").clicked() {
            *close = true;
        }
    });
}

/// Where a branch without an upstream goes: which remote, under what name.
fn first_push(
    ui: &mut Ui,
    view: &mut SyncView,
    pane: &mut SyncPane<'_>,
    mut options: PushOptions,
    close: &mut bool,
) {
    title(
        ui,
        &format!("Push {} for the first time", options.branch.to_str_lossy()),
    );
    let remotes = pane.remotes();
    let mut remote = options.remote.to_str_lossy().into_owned();
    let mut name = options.remote_branch.to_str_lossy().into_owned();
    egui::Grid::new("first-push")
        .num_columns(2)
        .spacing([12.0, 8.0])
        .show(ui, |ui| {
            let remote_label = ui.label(muted(ui, "Remote"));
            egui::ComboBox::from_id_salt("first-push-remote")
                .selected_text(&remote)
                .show_ui(ui, |ui| {
                    for choice in &remotes {
                        ui.selectable_value(&mut remote, choice.clone(), choice);
                    }
                })
                .response
                .labelled_by(remote_label.id);
            ui.end_row();
            let name_label = ui.label(muted(ui, "As"));
            ui.add_sized([240.0, 20.0], egui::TextEdit::singleline(&mut name))
                .labelled_by(name_label.id);
            ui.end_row();
        });
    ui.add_space(8.0);
    ui.label(muted(
        ui,
        "It becomes the branch's upstream: later pushes and pulls go there.",
    ));
    options.remote = remote.into();
    options.remote_branch = name.trim().to_owned().into();
    ui.add_space(12.0);
    let ready = !options.remote.is_empty() && !options.remote_branch.is_empty();
    widgets::buttons_row(ui, |ui| {
        if widgets::primary(ui, ready, "Push").clicked() {
            pane.start(SyncTask::Push(options.clone()));
            *close = true;
        }
        if widgets::secondary(ui, true, "Cancel").clicked() {
            *close = true;
        }
    });
    if !*close {
        view.dialog = Some(Dialog::FirstPush(options));
    }
}

/// Whether to give up an operation.
fn abort(ui: &mut Ui, pane: &mut SyncPane<'_>, operation: Operation, close: &mut bool) {
    title(
        ui,
        &format!("Abort {}?", operation_name(operation).to_lowercase()),
    );
    ui.add(
        egui::Label::new(
            "Your branch and files go back to how they were before it began: the conflicts \
             resolved so far are lost.",
        )
        .wrap(),
    );
    ui.add_space(12.0);
    widgets::buttons_row(ui, |ui| {
        if widgets::primary(ui, true, "Abort").clicked() {
            pane.start(SyncTask::Abort(operation));
            *close = true;
        }
        if widgets::secondary(ui, true, "Keep going").clicked() {
            *close = true;
        }
    });
}
