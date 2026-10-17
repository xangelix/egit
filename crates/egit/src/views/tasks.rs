//! A repository's tasks, as the history shows them: the branch, its upstream and how they
//! compare, Fetch, Pull, and Push (what the running task is doing, with Cancel); how the last
//! one ended, in words a person can act on (or the dialog it leads to: `views/dialogs.rs`);
//! and the banner of an operation git stopped in the middle of.

use std::fmt::Write as _;

use bstr::{BString, ByteSlice as _};
use egit_git::{
    ConfigSnapshot, Head, Integrated, Operation, PullMode, Pulled, PushFlag, PushOptions, Pushed,
    Stash, SwitchTo, Tracking,
};
use egui::{Align, Id, Layout, RichText, TextStyle, Ui, UiBuilder};

use super::dialogs::{self, Dialog};
use super::history::ID_DIGITS;

use crate::app::Action;
use crate::problem::Problem;
use crate::repo::RepoId;
use crate::tasks::{Outcome, Task, Tasks, has_remotes, remote_names, upstream_of};
use crate::theme::{Palette, icon};
use crate::widgets::{self, muted};
use crate::{fonts, format};

/// What the task parts remember between frames.
#[derive(Debug, Default)]
pub(crate) struct TasksView {
    /// The question asked, if any.
    pub(super) dialog: Option<Dialog>,
    /// The task whose outcome was taken in (dismissed, or turned into a dialog).
    seen: u64,
}

/// What the task parts work with while they are drawn.
pub(crate) struct TaskPane<'a> {
    pub(crate) id: RepoId,
    pub(crate) head: Option<&'a Head>,
    pub(crate) operations: &'a [Operation],
    pub(crate) config: Option<&'a ConfigSnapshot>,
    /// The stashes, the latest first.
    pub(crate) stashes: &'a [Stash],
    pub(crate) tasks: &'a mut Tasks,
    pub(crate) actions: &'a mut Vec<Action>,
}

/// What the user asked for that the history view does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TaskEvent {
    /// Show the working tree (its conflicts).
    ShowWorkTree,
}

impl TaskPane<'_> {
    /// The current branch's name, when on one.
    pub(super) fn branch(&self) -> Option<String> {
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

    /// The remotes configured, in order.
    pub(super) fn remotes(&self) -> Vec<String> {
        self.config.map(remote_names).unwrap_or_default()
    }

    /// The current branch's upstream, by its short name.
    pub(super) fn upstream(&self) -> Option<String> {
        match self.head? {
            Head::Branch(branch) => branch
                .upstream
                .as_ref()
                .map(|upstream| upstream.name.to_str_lossy().into_owned()),
            Head::Detached(_) => None,
        }
    }

    /// Starts `task`.
    pub(super) fn start(&mut self, task: Task) {
        self.actions.push(Action::Task(self.id, task));
    }
}

/// The branch, how it compares with its upstream, and Fetch, Pull, Push (or, while one runs,
/// what it is doing and Cancel), from the right.
pub(crate) fn bar(ui: &mut Ui, view: &mut TasksView, pane: &mut TaskPane<'_>) {
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
    ui.scope_builder(builder.id(Id::new(("tasks", part, id))), add)
        .inner
}

fn controls(ui: &mut Ui, view: &mut TasksView, pane: &mut TaskPane<'_>) {
    let palette = Palette::of(ui.ctx());
    if pane.tasks.is_busy() {
        // The progress moves on its own, without input.
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(100));
        if widgets::secondary(ui, true, "Cancel").clicked() {
            pane.actions.push(Action::CancelTask(pane.id));
        }
        ui.label(muted(ui, progress_text(pane.tasks)));
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
            (Some(target), _) => pane.start(Task::Push(target)),
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
        pane.start(Task::Pull(PullMode::AsConfigured));
    }
    if widgets::secondary(
        ui,
        has_remotes,
        &format!("{}  Fetch", icon::ARROWS_CLOCKWISE),
    )
    .on_disabled_hover_text("The repository has no remote")
    .clicked()
    {
        pane.start(Task::Fetch);
    }
    if let Some(problem) = &pane.tasks.background_problem {
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
fn progress_text(tasks: &Tasks) -> String {
    let doing = tasks.task.as_ref().map_or("Working", Task::doing);
    format::progress(doing, &tasks.progress.report())
}

/// How the last task ended (until dismissed or another starts): a line, a problem, or the
/// dialog it leads to.
pub(crate) fn outcome(ui: &mut Ui, view: &mut TasksView, pane: &mut TaskPane<'_>) {
    own_ids(ui, "outcome", pane.id, UiBuilder::new(), |ui| {
        outcome_lines(ui, view, pane);
    });
}

fn outcome_lines(ui: &mut Ui, view: &mut TasksView, pane: &mut TaskPane<'_>) {
    if pane.tasks.is_busy() || view.seen == pane.tasks.started {
        return;
    }
    let palette = Palette::of(ui.ctx());
    let started = pane.tasks.started;
    let result = pane
        .tasks
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
        Some(Ok(Outcome::Pulled(Pulled::Diverged))) => {
            view.dialog = Some(Dialog::Diverged { remember: false });
            dismiss = true;
        }
        Some(Ok(Outcome::WouldOverwrite { paths, untracked })) => {
            if let Some(Task::Switch(to)) = &pane.tasks.task {
                view.dialog = Some(Dialog::StashAndSwitch {
                    to: to.clone(),
                    paths,
                    untracked,
                });
            }
            dismiss = true;
        }
        Some(Ok(Outcome::NotMerged)) => {
            if let Some(Task::DeleteBranch { name, .. }) = &pane.tasks.task {
                view.dialog = Some(Dialog::DeleteBranch {
                    name: name.clone(),
                    unmerged: true,
                });
            }
            dismiss = true;
        }
        Some(Ok(Outcome::Pushed(pushed))) if !pushed.succeeded() => {
            refused(ui, view, pane, &pushed, &mut dismiss);
        }
        Some(Ok(outcome)) => {
            let text = ended(pane.tasks.task.as_ref(), &outcome);
            let color = if outcome == Outcome::Cancelled {
                palette.muted
            } else {
                palette.ok
            };
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(text).color(color));
                if widgets::secondary(ui, true, "Dismiss").clicked() {
                    dismiss = true;
                }
            });
            if let Outcome::Pushed(pushed) = &outcome {
                notes(ui, pushed);
            }
        }
    }
    if dismiss {
        view.seen = started;
    }
}

/// How `task` ended, in words.
fn ended(task: Option<&Task>, outcome: &Outcome) -> String {
    const CONFLICTS: &str = "resolve the conflicts, stage them, then Continue.";
    let Some(task) = task else {
        return "Done.".to_owned();
    };
    match (task, outcome) {
        (_, Outcome::Cancelled) => "Cancelled.".to_owned(),
        (_, Outcome::Fetched) => "Fetched.".to_owned(),
        (_, Outcome::Pulled(Pulled::Updated)) => "Pulled.".to_owned(),
        (_, Outcome::Pulled(_)) => format!("The pull stopped at conflicts: {CONFLICTS}"),
        (_, Outcome::Pushed(pushed)) => pushed_text(pushed),
        (_, Outcome::Aborted) => "Aborted: everything is as it was before.".to_owned(),
        (_, Outcome::Continued) => "Carried on.".to_owned(),
        (_, Outcome::Stashed(true)) => "Stashed your changes.".to_owned(),
        (_, Outcome::Stashed(false)) => "There were no changes to stash.".to_owned(),
        (Task::ApplyStash { stash, .. }, Outcome::Integrated(Integrated::Conflicts)) => format!(
            "Applying stash@{{{}}} conflicts: resolve the files marked Conflict, then stage \
             them (a popped stash is kept).",
            stash.index
        ),
        (task, Outcome::Integrated(Integrated::Conflicts)) => {
            format!("{} stopped at conflicts: {CONFLICTS}", task.doing())
        }
        (
            Task::CherryPick(commit) | Task::Revert(commit),
            Outcome::Integrated(Integrated::Empty),
        ) => {
            format!(
                "{} changes nothing here (its changes are in already): skipped.",
                commit.abbreviated(ID_DIGITS)
            )
        }
        (task, _) => done_text(task),
    }
}

/// What a task that went through did.
fn done_text(task: &Task) -> String {
    let short = |name: &BString| name.to_str_lossy().into_owned();
    match task {
        Task::Switch(to) | Task::StashAndSwitch(to) => {
            let stashed = if matches!(task, Task::StashAndSwitch(_)) {
                "Stashed your changes, and switched"
            } else {
                "Switched"
            };
            match to {
                SwitchTo::Branch(name) => format!("{stashed} to {}.", short(name)),
                SwitchTo::Track {
                    remote_branch,
                    local,
                } => format!(
                    "{stashed} to {}, tracking {}.",
                    short(local),
                    short(remote_branch)
                ),
                SwitchTo::NewBranch { name, .. } => {
                    format!("Made {}; {} to it.", short(name), stashed.to_lowercase())
                }
                SwitchTo::Detached(at) => format!(
                    "{stashed} to {}, without a branch.",
                    at.abbreviated(ID_DIGITS)
                ),
            }
        }
        Task::CreateBranch { name, .. } => format!("Made {}.", short(name)),
        Task::RenameBranch { old, new } => format!("Renamed {} to {}.", short(old), short(new)),
        Task::DeleteBranch { name, .. } => format!("Deleted {}.", short(name)),
        Task::Merge { shown, .. } => format!("Merged {shown}."),
        Task::Rebase { shown, .. } => format!("Rebased onto {shown}."),
        Task::CherryPick(commit) => format!("Picked {}.", commit.abbreviated(ID_DIGITS)),
        Task::Revert(commit) => format!("Reverted {}.", commit.abbreviated(ID_DIGITS)),
        Task::Reset(commit, _) => format!("Reset to {}.", commit.abbreviated(ID_DIGITS)),
        Task::CreateTag { name, at, .. } => {
            format!("Tagged {} as {}.", at.abbreviated(ID_DIGITS), short(name))
        }
        Task::DeleteTag(name) => format!("Deleted the tag {}.", short(name)),
        Task::ApplyStash { stash, pop } => format!(
            "{} stash@{{{}}}.",
            if *pop { "Popped" } else { "Applied" },
            stash.index
        ),
        Task::DropStash(stash) => format!("Dropped stash@{{{}}}.", stash.index),
        _ => "Done.".to_owned(),
    }
}

/// What a push that went through did.
fn pushed_text(pushed: &Pushed) -> String {
    let Some(first) = pushed.refs.first() else {
        return "Pushed.".to_owned();
    };
    let to = first.to.as_slice();
    let to = to
        .strip_prefix(b"refs/heads/")
        .or_else(|| to.strip_prefix(b"refs/tags/"))
        .unwrap_or(to)
        .to_str_lossy()
        .into_owned();
    match first.flag {
        PushFlag::Deleted => format!("{to} was deleted on the remote."),
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
    view: &mut TasksView,
    pane: &mut TaskPane<'_>,
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
        pane.tasks.task.as_ref().and_then(|task| match task {
            Task::Push(options) => Some(options.clone()),
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
                pane.start(Task::Pull(PullMode::AsConfigured));
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
    view: &mut TasksView,
    pane: &mut TaskPane<'_>,
) -> Option<TaskEvent> {
    own_ids(ui, "banner", pane.id, UiBuilder::new(), |ui| {
        operation_banner(ui, view, pane)
    })
}

fn operation_banner(
    ui: &mut Ui,
    view: &mut TasksView,
    pane: &mut TaskPane<'_>,
) -> Option<TaskEvent> {
    let &operation = pane.operations.first()?;
    let palette = Palette::of(ui.ctx());
    let busy = pane.tasks.is_busy();
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
                        pane.start(Task::Abort(operation));
                    }
                    return;
                }
                ui.label(
                    "Resolve the conflicts (the files marked Conflict), stage them, then \
                     Continue; or Abort to go back to how things were.",
                );
                if widgets::secondary(ui, true, "Show the conflicts").clicked() {
                    event = Some(TaskEvent::ShowWorkTree);
                }
                if widgets::primary(ui, !busy, "Continue").clicked() {
                    pane.start(Task::Continue(operation));
                }
                if widgets::secondary(ui, !busy, "Abort…").clicked() {
                    view.dialog = Some(Dialog::Abort(operation));
                }
            });
        });
    event
}

/// An operation, as a sentence starts with it.
pub(super) const fn operation_name(operation: Operation) -> &'static str {
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
pub(crate) fn dialogs(ctx: &egui::Context, view: &mut TasksView, pane: &mut TaskPane<'_>) {
    dialogs::show(ctx, &mut view.dialog, pane);
}
