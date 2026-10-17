//! The questions a repository's tasks ask, one at a time in a modal: how to bring diverged
//! branches together, forcing a push, a branch's first push, giving up an operation; a new
//! branch's or tag's name, a branch's new name; and, before anything that loses work, what it
//! loses (deleting a branch or a remote's, a tag, a stash; a hard reset; stashing changes in
//! the way of a switch).
//!
//! Each dialog keeps what is typed into it in its variant, and ends by starting a task or by
//! being closed.

use bstr::{BString, ByteSlice as _};
use egit_git::{
    ObjectId, Operation, PullMode, PushOptions, ResetMode, Stash, SwitchTo, check_ref_name,
};
use egui::{Id, Key, Modal, RichText, TextEdit, Ui};

use super::history::ID_DIGITS;
use super::tasks::TaskPane;
use crate::tasks::Task;
use crate::theme::Palette;
use crate::widgets::{self, muted};

/// A question asked before (or after) a task.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Dialog {
    /// How to bring diverged branches together (and whether to remember it).
    Diverged { remember: bool },
    /// Whether to replace the remote's branch.
    ForcePush(PushOptions),
    /// Where a branch without an upstream goes.
    FirstPush(PushOptions),
    /// Whether to give up an operation.
    Abort(Operation),
    /// A new branch's name, at a commit, and whether to switch to it.
    CreateBranch {
        at: ObjectId,
        name: String,
        switch: bool,
    },
    /// A branch's new name.
    RenameBranch { old: BString, name: String },
    /// Whether to delete a branch (`unmerged`: git said its commits are nowhere else).
    DeleteBranch { name: BString, unmerged: bool },
    /// Whether to delete a remote's branch.
    DeleteRemoteBranch { remote: BString, branch: BString },
    /// A new tag's name at a commit, and a message (which makes it annotated).
    CreateTag {
        at: ObjectId,
        name: String,
        message: String,
    },
    /// Whether to delete a tag.
    DeleteTag(BString),
    /// Whether to reset the current branch to a commit, discarding uncommitted changes.
    HardReset { at: ObjectId },
    /// Whether to stash the changes in the way of a switch, then switch.
    StashAndSwitch {
        to: SwitchTo,
        paths: Vec<BString>,
        untracked: bool,
    },
    /// The work tree's changes to stash: a message, and whether untracked files go too.
    Stash { message: String, untracked: bool },
    /// Whether to drop a stash.
    DropStash(Stash),
}

/// How a dialog was answered.
enum Answer {
    /// Closed, nothing started.
    Closed,
    /// This task started.
    Run(Task),
}

/// Shows `dialog` while there is one.
pub(super) fn show(ctx: &egui::Context, dialog: &mut Option<Dialog>, pane: &mut TaskPane<'_>) {
    let Some(asked) = dialog.as_mut() else {
        return;
    };
    let palette = Palette::of(ctx);
    let frame = egui::Frame::new()
        .fill(palette.surface)
        .stroke(egui::Stroke::new(1.0, palette.border))
        .corner_radius(egui::CornerRadius::same(12))
        .inner_margin(egui::Margin::same(20));
    let shown = Modal::new(Id::new(("task-dialog", pane.id)))
        .frame(frame)
        .show(ctx, |ui| {
            ui.set_width(480.0);
            ask(ui, asked, pane)
        });
    let answer = if shown.should_close() {
        Some(Answer::Closed)
    } else {
        shown.inner
    };
    match answer {
        Some(Answer::Run(task)) => {
            pane.start(task);
            *dialog = None;
        }
        Some(Answer::Closed) => *dialog = None,
        None => {}
    }
}

/// The dialog's question; its answer, once given.
fn ask(ui: &mut Ui, dialog: &mut Dialog, pane: &TaskPane<'_>) -> Option<Answer> {
    match dialog {
        Dialog::Diverged { remember } => diverged(ui, pane, remember),
        Dialog::ForcePush(options) => force_push(ui, options),
        Dialog::FirstPush(options) => first_push(ui, pane, options),
        Dialog::Abort(operation) => abort(ui, *operation),
        Dialog::CreateBranch { at, name, switch } => create_branch(ui, *at, name, switch),
        Dialog::RenameBranch { old, name } => rename_branch(ui, old, name),
        Dialog::DeleteBranch { name, unmerged } => delete_branch(ui, name, *unmerged),
        Dialog::DeleteRemoteBranch { remote, branch } => delete_remote_branch(ui, remote, branch),
        Dialog::CreateTag { at, name, message } => create_tag(ui, *at, name, message),
        Dialog::DeleteTag(name) => confirm(
            ui,
            &format!("Delete the tag {}?", name.to_str_lossy()),
            "Here only: a remote that has it keeps its own.",
            "Delete",
            || Task::DeleteTag(name.clone()),
        ),
        Dialog::HardReset { at } => hard_reset(ui, pane, *at),
        Dialog::StashAndSwitch {
            to,
            paths,
            untracked,
        } => stash_and_switch(ui, to, paths, *untracked),
        Dialog::Stash { message, untracked } => stash(ui, message, untracked),
        Dialog::DropStash(stash) => confirm(
            ui,
            &format!("Drop stash@{{{}}}?", stash.index),
            &format!("{}: its changes are gone.", stash.message),
            "Drop",
            || Task::DropStash(stash.clone()),
        ),
    }
}

/// A dialog's title.
fn title(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(18.0).strong());
    ui.add_space(8.0);
}

/// A paragraph that wraps.
fn says(ui: &mut Ui, text: &str) {
    ui.add(egui::Label::new(text).wrap());
}

/// A question with one task to start (`go`, labelled `yes`) or Cancel.
fn confirm(
    ui: &mut Ui,
    question: &str,
    said: &str,
    yes: &str,
    go: impl FnOnce() -> Task,
) -> Option<Answer> {
    title(ui, question);
    says(ui, said);
    ui.add_space(12.0);
    buttons(ui, yes, true, go)
}

/// The dialog's buttons: `yes` (when `ready`), starting `go`, and Cancel.
fn buttons(ui: &mut Ui, yes: &str, ready: bool, go: impl FnOnce() -> Task) -> Option<Answer> {
    let mut answer = None;
    widgets::buttons_row(ui, |ui| {
        if widgets::primary(ui, ready, yes).clicked() {
            answer = Some(Answer::Run(go()));
        }
        if widgets::secondary(ui, true, "Cancel").clicked() {
            answer = Some(Answer::Closed);
        }
    });
    answer
}

/// A labelled one-line field, focused when nothing else is; returns whether Enter was pressed
/// in it.
fn name_field(ui: &mut Ui, label: &str, text: &mut String) -> bool {
    let label = ui.label(muted(ui, label));
    let field = ui
        .add(TextEdit::singleline(text).desired_width(f32::INFINITY))
        .labelled_by(label.id);
    if ui.memory(|memory| memory.focused().is_none()) {
        field.request_focus();
    }
    field.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter))
}

/// Why `name` won't do as a branch's or tag's, under its field (nothing while it is empty).
fn name_problem(ui: &mut Ui, name: &str) -> bool {
    let name = name.trim();
    match check_ref_name(name) {
        Ok(()) => true,
        Err(_) if name.is_empty() => false,
        Err(why) => {
            let palette = Palette::of(ui.ctx());
            ui.label(RichText::new(format!("Not a name git takes: {why}.")).color(palette.warn));
            false
        }
    }
}

/// How to bring diverged branches together.
fn diverged(ui: &mut Ui, pane: &TaskPane<'_>, remember: &mut bool) -> Option<Answer> {
    let upstream = pane.upstream().unwrap_or_else(|| "its upstream".to_owned());
    title(ui, "The branches have diverged");
    says(
        ui,
        &format!(
            "Your branch and {upstream} each have commits the other hasn't. Bring them together \
             by putting your commits after theirs (rebase), or by joining both in a merge commit."
        ),
    );
    ui.add_space(8.0);
    ui.checkbox(remember, "Remember this for the repository (pull.rebase)");
    ui.add_space(8.0);
    let mut answer = None;
    widgets::buttons_row(ui, |ui| {
        for (label, rebase, primary) in [("Rebase", true, true), ("Merge", false, false)] {
            let button = if primary {
                widgets::primary(ui, true, label)
            } else {
                widgets::secondary(ui, true, label)
            };
            if button.clicked() {
                answer = Some(Answer::Run(if *remember {
                    Task::PullAndRemember { rebase }
                } else if rebase {
                    Task::Pull(PullMode::Rebase)
                } else {
                    Task::Pull(PullMode::Merge)
                }));
            }
        }
        if widgets::secondary(ui, true, "Cancel").clicked() {
            answer = Some(Answer::Closed);
        }
    });
    answer
}

/// Whether to replace the remote's branch.
fn force_push(ui: &mut Ui, options: &PushOptions) -> Option<Answer> {
    confirm(
        ui,
        &format!(
            "Force push to {}/{}?",
            options.remote.to_str_lossy(),
            options.remote_branch.to_str_lossy()
        ),
        "This replaces the remote's branch with yours: its commits that yours hasn't go from it. \
         egit forces only if the remote is still where you last fetched it, and that is in your \
         branch; otherwise nothing is pushed.",
        "Force push",
        || Task::Push(options.clone()),
    )
}

/// Where a branch without an upstream goes: which remote, under what name.
fn first_push(ui: &mut Ui, pane: &TaskPane<'_>, options: &mut PushOptions) -> Option<Answer> {
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
            ui.add_sized([240.0, 20.0], TextEdit::singleline(&mut name))
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
    buttons(ui, "Push", ready, || Task::Push(options.clone()))
}

/// Whether to give up an operation.
fn abort(ui: &mut Ui, operation: Operation) -> Option<Answer> {
    title(
        ui,
        &format!(
            "Abort {}?",
            super::tasks::operation_name(operation).to_lowercase()
        ),
    );
    says(
        ui,
        "Your branch and files go back to how they were before it began: the conflicts \
         resolved so far are lost.",
    );
    ui.add_space(12.0);
    let mut answer = None;
    widgets::buttons_row(ui, |ui| {
        if widgets::primary(ui, true, "Abort").clicked() {
            answer = Some(Answer::Run(Task::Abort(operation)));
        }
        if widgets::secondary(ui, true, "Keep going").clicked() {
            answer = Some(Answer::Closed);
        }
    });
    answer
}

/// A new branch at `at`: its name, and whether to switch to it.
fn create_branch(
    ui: &mut Ui,
    at: ObjectId,
    name: &mut String,
    switch: &mut bool,
) -> Option<Answer> {
    title(
        ui,
        &format!("Create a branch at {}", at.abbreviated(ID_DIGITS)),
    );
    let entered = name_field(ui, "Name", name);
    let ready = name_problem(ui, name);
    ui.add_space(4.0);
    ui.checkbox(switch, "Switch to it");
    ui.add_space(12.0);
    let go = || {
        let name = BString::from(name.trim());
        if *switch {
            Task::Switch(SwitchTo::NewBranch { name, at })
        } else {
            Task::CreateBranch { name, at }
        }
    };
    if entered && ready {
        return Some(Answer::Run(go()));
    }
    buttons(ui, "Create", ready, go)
}

/// A branch's new name.
fn rename_branch(ui: &mut Ui, old: &BString, name: &mut String) -> Option<Answer> {
    title(ui, &format!("Rename {}", old.to_str_lossy()));
    let entered = name_field(ui, "New name", name);
    let ready = name_problem(ui, name) && name.trim().as_bytes() != old.as_slice();
    ui.add_space(12.0);
    let go = || Task::RenameBranch {
        old: old.clone(),
        new: name.trim().into(),
    };
    if entered && ready {
        return Some(Answer::Run(go()));
    }
    buttons(ui, "Rename", ready, go)
}

/// Whether to delete a branch.
fn delete_branch(ui: &mut Ui, name: &BString, unmerged: bool) -> Option<Answer> {
    let name_shown = name.to_str_lossy();
    let said = if unmerged {
        format!(
            "{name_shown} has commits no other branch has: deleting it loses them (git keeps \
             them in its reflog for a while)."
        )
    } else {
        "The branch goes; its commits stay where other branches have them.".to_owned()
    };
    let yes = if unmerged { "Delete anyway" } else { "Delete" };
    confirm(ui, &format!("Delete {name_shown}?"), &said, yes, || {
        Task::DeleteBranch {
            name: name.clone(),
            force: unmerged,
        }
    })
}

/// Whether to delete a remote's branch.
fn delete_remote_branch(ui: &mut Ui, remote: &BString, branch: &BString) -> Option<Answer> {
    let remote_shown = remote.to_str_lossy();
    confirm(
        ui,
        &format!("Delete {} on {remote_shown}?", branch.to_str_lossy()),
        &format!("It goes for everyone using {remote_shown}; your own branches stay as they are."),
        "Delete",
        || Task::DeleteRemoteBranch {
            remote: remote.clone(),
            branch: branch.clone(),
        },
    )
}

/// A new tag at `at`: its name, and a message making it annotated.
fn create_tag(
    ui: &mut Ui,
    at: ObjectId,
    name: &mut String,
    message: &mut String,
) -> Option<Answer> {
    title(ui, &format!("Tag {}", at.abbreviated(ID_DIGITS)));
    name_field(ui, "Name", name);
    let ready = name_problem(ui, name);
    ui.add_space(8.0);
    let label = ui.label(muted(
        ui,
        "Message (makes it an annotated tag, with its author and date)",
    ));
    ui.add(
        TextEdit::multiline(message)
            .desired_rows(3)
            .desired_width(f32::INFINITY),
    )
    .labelled_by(label.id);
    ui.add_space(12.0);
    buttons(ui, "Create tag", ready, || Task::CreateTag {
        name: name.trim().into(),
        at,
        message: (!message.trim().is_empty()).then(|| message.clone()),
    })
}

/// Whether to reset the current branch to `at`, discarding uncommitted changes.
fn hard_reset(ui: &mut Ui, pane: &TaskPane<'_>, at: ObjectId) -> Option<Answer> {
    let commit = at.abbreviated(ID_DIGITS);
    let branch = pane.branch().unwrap_or_else(|| "HEAD".to_owned());
    confirm(
        ui,
        &format!("Reset {branch} to {commit}, discarding your changes?"),
        &format!(
            "{branch} moves to {commit}, and the index and the work tree become that commit's: \
             uncommitted changes are lost for good. The commits after it stay in the reflog for \
             a while."
        ),
        "Reset",
        || Task::Reset(at, ResetMode::Hard),
    )
}

/// Whether to stash the changes in the way of a switch, then switch.
fn stash_and_switch(
    ui: &mut Ui,
    to: &SwitchTo,
    paths: &[BString],
    untracked: bool,
) -> Option<Answer> {
    const NAMED: usize = 5;
    let mut named: Vec<String> = paths
        .iter()
        .take(NAMED)
        .map(|path| path.to_str_lossy().into_owned())
        .collect();
    if paths.len() > NAMED {
        named.push(format!("{} more", paths.len().saturating_sub(NAMED)));
    }
    let named = named.join(", ");
    let what = if untracked {
        format!("{named}, which git doesn't track")
    } else {
        format!("your changes to {named}")
    };
    confirm(
        ui,
        "Your changes are in the way",
        &format!(
            "Switching would overwrite {what}. Stash your changes (untracked files too), then \
             switch? They wait in the stash list."
        ),
        "Stash and switch",
        || Task::StashAndSwitch(to.clone()),
    )
}

/// The work tree's changes to stash: a message, and whether untracked files go too.
fn stash(ui: &mut Ui, message: &mut String, untracked: &mut bool) -> Option<Answer> {
    title(ui, "Stash your changes");
    let entered = name_field(ui, "Message (optional)", message);
    ui.add_space(4.0);
    ui.checkbox(untracked, "Files git doesn't track too");
    ui.add_space(12.0);
    let go = || Task::Stash {
        message: message.trim().to_owned(),
        untracked: *untracked,
    };
    if entered {
        return Some(Answer::Run(go()));
    }
    buttons(ui, "Stash", true, go)
}
