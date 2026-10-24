//! The questions a repository's tasks ask, one at a time in a modal: how to bring diverged
//! branches together, forcing a push, a branch's first push, giving up an operation; a new
//! branch's or tag's name, a branch's new name, a branch's upstream, where a new worktree goes;
//! and, before anything that loses work, what it loses (deleting a branch, here or on its
//! remote too, or a remote's, a tag, a stash; a hard reset; stashing changes in the way of a
//! switch).
//!
//! Each dialog keeps what is typed into it in its variant, and ends by starting a task or by
//! being closed.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use bstr::{BString, ByteSlice as _};
use egit_git::{
    ObjectId, Operation, PullMode, PushOptions, ResetMode, Stash, SwitchTo, check_ref_name,
};
use egui::{Id, Key, Modal, RichText, Ui};

use super::history::ID_DIGITS;
use super::tasks::TaskPane;
use crate::discarding::Discard;
use crate::tasks::{Task, WorktreeSource};
use crate::theme::Palette;
use crate::widgets::{self, muted};

/// The most remote branches the upstream dialog offers at once.
const UPSTREAMS_OFFERED: usize = 8;

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
    /// Whether to delete a branch (`unmerged`: git said its commits are nowhere else), and with
    /// `remote`, its remote's too (the remote, its name there).
    DeleteBranch {
        name: BString,
        unmerged: bool,
        remote: Option<(BString, BString)>,
    },
    /// A branch's upstream: one of `choices` (remote branches' short names), typed; or none.
    SetUpstream {
        branch: BString,
        upstream: String,
        had: Option<String>,
        choices: Vec<String>,
    },
    /// Where a new worktree goes, as typed (a relative path is beside the repository), what it
    /// checks out, and a new branch's name; `open`: show it once made.
    AddWorktree {
        from: WorktreeAsked,
        path: String,
        name: String,
        open: bool,
    },
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
    /// Whether to drop a commit from the current branch.
    DropCommit(ObjectId),
    /// Whether to sign commits of the current branch picked together (and make every commit
    /// after them again).
    SignCommits(Vec<ObjectId>),
    /// Whether to stash the changes in the way of a switch, then switch.
    StashAndSwitch {
        to: SwitchTo,
        paths: Vec<BString>,
        untracked: bool,
    },
    /// Whether to stash the changes in the way of a pull, then pull (putting them back, unless
    /// they are files git doesn't track).
    PullStashing {
        mode: PullMode,
        paths: Vec<BString>,
        untracked: bool,
    },
    /// The work tree's changes to stash: a message, and whether untracked files go too; or
    /// only those at some paths (untracked ones among them go too).
    Stash {
        message: String,
        untracked: bool,
        paths: Vec<BString>,
    },
    /// Whether to drop a stash.
    DropStash(Stash),
    /// Whether to throw whole files' changes not staged away (untracked files deleted).
    Discard(Discard),
    /// A remote to add: its name and address, and whether to fetch from it at once.
    AddRemote {
        name: String,
        url: String,
        fetch: bool,
    },
    /// A remote's addresses: where it fetches from, and where it pushes to (empty: the same).
    EditRemote {
        remote: BString,
        url: String,
        push: String,
    },
    /// A remote's new name.
    RenameRemote { old: BString, name: String },
    /// Whether to remove a remote, with its branches.
    RemoveRemote(BString),
    /// Whether to remove a worktree; `changes`: it has some, which would go with it.
    RemoveWorktree { path: PathBuf, changes: bool },
    /// Why a worktree is locked (optional).
    LockWorktree { path: PathBuf, reason: String },
}

/// What a new worktree checks out, as a menu asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WorktreeAsked {
    /// A local branch.
    Branch(BString),
    /// A remote's branch (its full and short names), through a new branch following it.
    RemoteBranch { full: BString, short: BString },
    /// A commit: on a new branch, or without one.
    Commit(ObjectId),
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
        .corner_radius(egui::CornerRadius::same(crate::theme::RADIUS_CARD))
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
        Dialog::DeleteBranch {
            name,
            unmerged,
            remote,
        } => delete_branch(ui, name, *unmerged, remote.as_ref()),
        Dialog::SetUpstream {
            branch,
            upstream,
            had,
            choices,
        } => set_upstream(ui, branch, upstream, had.as_deref(), choices),
        Dialog::AddWorktree {
            from,
            path,
            name,
            open,
        } => add_worktree(ui, pane, from, path, name, open),
        Dialog::DeleteRemoteBranch { remote, branch } => delete_remote_branch(ui, remote, branch),
        Dialog::CreateTag { at, name, message } => create_tag(ui, *at, name, message),
        Dialog::DeleteTag(name) => confirm_danger(
            ui,
            &format!("Delete the tag {}?", name.to_str_lossy()),
            "Here only: a remote that has it keeps its own.",
            "Delete",
            || Task::DeleteTag(name.clone()),
        ),
        Dialog::HardReset { at } => hard_reset(ui, pane, *at),
        Dialog::SignCommits(commits) => sign_commits(ui, pane, commits),
        Dialog::DropCommit(commit) => {
            let branch = pane.branch().unwrap_or_else(|| "HEAD".to_owned());
            confirm_danger(
                ui,
                &format!("Drop {} from {branch}?", commit.abbreviated(ID_DIGITS)),
                "Its changes go from the branch, and the commits after it are rewritten (git's \
                 reflog keeps the old ones for a while).",
                "Drop",
                || Task::DropCommit(*commit),
            )
        }
        Dialog::StashAndSwitch {
            to,
            paths,
            untracked,
        } => stash_and_switch(ui, to, paths, *untracked),
        Dialog::PullStashing {
            mode,
            paths,
            untracked,
        } => pull_stashing(ui, *mode, paths, *untracked),
        Dialog::Stash {
            message,
            untracked,
            paths,
        } => stash(ui, message, untracked, paths),
        Dialog::DropStash(stash) => confirm_danger(
            ui,
            &format!("Drop stash@{{{}}}?", stash.index),
            &format!("{}: its changes are gone.", stash.message),
            "Drop",
            || Task::DropStash(stash.clone()),
        ),
        Dialog::Discard(discard) => discard_files(ui, discard),
        Dialog::AddRemote { name, url, fetch } => add_remote(ui, pane, name, url, fetch),
        Dialog::EditRemote { remote, url, push } => edit_remote(ui, remote, url, push),
        Dialog::RenameRemote { old, name } => rename_remote(ui, pane, old, name),
        Dialog::RemoveRemote(name) => {
            let name_shown = name.to_str_lossy();
            confirm_danger(
                ui,
                &format!("Remove the remote {name_shown}?"),
                &format!(
                    "Its remote-tracking branches go too ({name_shown}/…), and branches \
                     following them follow nothing after. The remote itself is untouched."
                ),
                "Remove",
                || Task::RemoveRemote(name.clone()),
            )
        }
        Dialog::RemoveWorktree { path, changes } => remove_worktree(ui, pane, path, *changes),
        Dialog::LockWorktree { path, reason } => lock_worktree(ui, pane, path, reason),
    }
}

/// Whether to discard whole files: what each kind of file becomes, and how long Undo can bring
/// them back.
fn discard_files(ui: &mut Ui, discard: &Discard) -> Option<Answer> {
    let Discard::Files { tracked, untracked } = discard else {
        return Some(Answer::Closed);
    };
    let deleting = tracked.is_empty() && untracked.len() == 1;
    let question = if deleting {
        format!("Delete {}?", discard.words())
    } else {
        format!("Discard {}?", discard.words())
    };
    let back = match tracked.len() {
        0 => None,
        1 => Some(
            "its changes not staged are lost: it goes back to what is staged (or, with \
                   nothing staged, to the last commit)"
                .to_owned(),
        ),
        files => Some(format!(
            "{files} files go back to what is staged (or, with nothing staged, to the last \
             commit)"
        )),
    };
    let gone = match untracked.len() {
        0 => None,
        1 if deleting => Some("it isn't tracked, so all of it goes".to_owned()),
        1 => Some("an untracked file is deleted".to_owned()),
        files => Some(format!("{files} untracked files are deleted")),
    };
    let what = back
        .into_iter()
        .chain(gone)
        .collect::<Vec<_>>()
        .join(", and ");
    let megabytes = egit_git::KEEP_BYTES >> 20;
    let undo = if discard.files() == 1 {
        format!("Undo brings it back while this tab is open (unless it is over {megabytes} MiB).")
    } else {
        format!(
            "Undo brings them back while this tab is open (unless there are over {} files, or \
             one is over {megabytes} MiB).",
            egit_git::KEEP_FILES
        )
    };
    let said = format!(
        "{}{}. {undo}",
        what.get(..1).unwrap_or_default().to_uppercase(),
        what.get(1..).unwrap_or_default(),
    );
    confirm_danger(
        ui,
        &question,
        &said,
        if deleting { "Delete" } else { "Discard" },
        || Task::Discard(discard.clone()),
    )
}

/// A dialog's title.
fn title(ui: &mut Ui, text: &str) {
    widgets::panel_title(ui, text);
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

/// As [`confirm`], for what loses something: `yes` in the danger colour.
fn confirm_danger(
    ui: &mut Ui,
    question: &str,
    said: &str,
    yes: &str,
    go: impl FnOnce() -> Task,
) -> Option<Answer> {
    title(ui, question);
    says(ui, said);
    ui.add_space(12.0);
    let mut answer = None;
    widgets::buttons_row(ui, |ui| {
        if widgets::danger(ui, true, yes).clicked() {
            answer = Some(Answer::Run(go()));
        }
        if widgets::secondary(ui, true, "Cancel").clicked() {
            answer = Some(Answer::Closed);
        }
    });
    answer
}

/// The dialog's buttons: `yes` (when `ready`), starting `go`, and Cancel.
fn buttons(ui: &mut Ui, yes: &str, ready: bool, go: impl FnOnce() -> Task) -> Option<Answer> {
    buttons_of(ui, (yes, None), ready, go)
}

/// [`buttons`], the first explained by the help topic `topic` where its word alone would
/// point elsewhere (a worktree's "Lock").
fn buttons_of(
    ui: &mut Ui,
    (yes, topic): (&str, Option<&'static str>),
    ready: bool,
    go: impl FnOnce() -> Task,
) -> Option<Answer> {
    let mut answer = None;
    widgets::buttons_row(ui, |ui| {
        let run = widgets::primary(ui, ready, yes);
        if let Some(topic) = topic {
            crate::help::note_topic(ui, run.rect, topic);
        }
        if run.clicked() {
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
        .add(crate::widgets::line_field(text).desired_width(f32::INFINITY))
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
    crate::widgets::checkbox(
        ui,
        remember,
        "Remember this for the repository (pull.rebase)",
    );
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
    confirm_danger(
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
    // Laid out as every dialog's form: each label over its field, the field as wide as the
    // dialog.
    let remote_label = ui.label(muted(ui, "Remote"));
    egui::ComboBox::from_id_salt("first-push-remote")
        .icon(crate::widgets::combo_caret)
        .selected_text(&remote)
        .width(ui.available_width())
        .show_ui(ui, |ui| {
            for choice in &remotes {
                ui.selectable_value(&mut remote, choice.clone(), choice);
            }
        })
        .response
        .labelled_by(remote_label.id);
    ui.add_space(8.0);
    let entered = name_field(ui, "Its name there", &mut name);
    ui.add_space(8.0);
    ui.label(muted(
        ui,
        "It becomes the branch's upstream: later pushes and pulls go there.",
    ));
    options.remote = remote.into();
    options.remote_branch = name.trim().to_owned().into();
    ui.add_space(12.0);
    let ready = !options.remote.is_empty() && !options.remote_branch.is_empty();
    if entered && ready {
        return Some(Answer::Run(Task::Push(options.clone())));
    }
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
        if widgets::danger(ui, true, "Abort").clicked() {
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
    crate::widgets::checkbox(ui, switch, "Switch to it");
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

/// Whether to delete a branch, and its remote's with `remote` (the remote, its name there).
fn delete_branch(
    ui: &mut Ui,
    name: &BString,
    unmerged: bool,
    remote: Option<&(BString, BString)>,
) -> Option<Answer> {
    let name_shown = name.to_str_lossy();
    let mut said = if unmerged {
        format!(
            "{name_shown} has commits no other branch has: deleting it loses them (git keeps \
             them in its reflog for a while)."
        )
    } else {
        "The branch goes; its commits stay where other branches have them.".to_owned()
    };
    let question = match remote {
        Some((remote, branch)) => {
            let remote = remote.to_str_lossy();
            let _ = write!(
                said,
                " On {remote}, {} goes too, for everyone using it.",
                branch.to_str_lossy()
            );
            format!(
                "Delete {name_shown} and {remote}/{}?",
                branch.to_str_lossy()
            )
        }
        None => format!("Delete {name_shown}?"),
    };
    let yes = if unmerged { "Delete anyway" } else { "Delete" };
    confirm_danger(ui, &question, &said, yes, || match remote {
        Some((remote, remote_branch)) => Task::DeleteBranchEverywhere {
            name: name.clone(),
            remote: remote.clone(),
            remote_branch: remote_branch.clone(),
            force: unmerged,
        },
        None => Task::DeleteBranch {
            name: name.clone(),
            force: unmerged,
        },
    })
}

/// A branch's upstream: typed (the remote branches holding what is typed offered to pick), or
/// none.
fn set_upstream(
    ui: &mut Ui,
    branch: &BString,
    upstream: &mut String,
    had: Option<&str>,
    choices: &[String],
) -> Option<Answer> {
    let branch_shown = branch.to_str_lossy();
    title(ui, &format!("Set {branch_shown}'s upstream"));
    says(
        ui,
        "The remote branch it pulls from and pushes to by default, and is compared with.",
    );
    ui.add_space(8.0);
    let entered = name_field(ui, "Upstream", upstream);
    let typed = upstream.trim().to_owned();
    let known = choices.contains(&typed);
    let offered: Vec<&String> = choices
        .iter()
        .filter(|choice| choice.contains(typed.as_str()) && **choice != typed)
        .take(UPSTREAMS_OFFERED)
        .collect();
    let mut picked = None;
    if !offered.is_empty() {
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            for choice in offered {
                let offer = ui.add(crate::widgets::small(choice.as_str()));
                crate::help::note_topic(ui, offer.rect, "upstream");
                if offer.clicked() {
                    picked = Some(choice.clone());
                }
            }
        });
    }
    if let Some(picked) = picked {
        *upstream = picked;
    }
    if !typed.is_empty() && !known {
        let palette = Palette::of(ui.ctx());
        ui.label(
            RichText::new("Not a remote branch here: fetch, or push the branch first.")
                .color(palette.warn),
        );
    }
    ui.add_space(12.0);
    let set = |upstream: Option<&str>| Task::SetUpstream {
        branch: branch.clone(),
        upstream: upstream.map(BString::from),
    };
    if entered && known {
        return Some(Answer::Run(set(Some(&typed))));
    }
    let mut answer = None;
    widgets::buttons_row(ui, |ui| {
        if widgets::primary(ui, known, "Set upstream").clicked() {
            answer = Some(Answer::Run(set(Some(upstream.trim()))));
        }
        if widgets::secondary(ui, true, "Cancel").clicked() {
            answer = Some(Answer::Closed);
        }
        // Forgetting it is another answer: at the row's other end, apart from the two.
        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
            if let Some(had) = had
                && widgets::secondary(ui, true, &format!("Forget {had}"))
                    .on_hover_text("Leave the branch without an upstream")
                    .clicked()
            {
                answer = Some(Answer::Run(set(None)));
            }
        });
    });
    answer
}

/// Where a new worktree goes (and a new branch's name); what it checks out.
fn add_worktree(
    ui: &mut Ui,
    pane: &TaskPane<'_>,
    from: &WorktreeAsked,
    path: &mut String,
    name: &mut String,
    open: &mut bool,
) -> Option<Answer> {
    let (what, said) = match from {
        WorktreeAsked::Branch(branch) => (
            branch.to_str_lossy().into_owned(),
            "The branch is checked out there, as in another clone sharing this one's history."
                .to_owned(),
        ),
        WorktreeAsked::RemoteBranch { short, .. } => (
            short.to_str_lossy().into_owned(),
            "A new branch following it is checked out there.".to_owned(),
        ),
        WorktreeAsked::Commit(commit) => (
            commit.abbreviated(ID_DIGITS).to_string(),
            "On a new branch, or without one if it has no name.".to_owned(),
        ),
    };
    title(ui, &format!("Create a worktree from {what}"));
    says(ui, &said);
    ui.add_space(8.0);
    let label = ui.label(muted(ui, "Folder (a new or empty one)"));
    ui.add(crate::widgets::line_field(path).desired_width(f32::INFINITY))
        .labelled_by(label.id);
    let target = worktree_path(path, pane.work_tree, pane.home);
    let needs_name = !matches!(from, WorktreeAsked::Branch(_));
    let mut ready = target.is_some();
    if needs_name {
        ui.add_space(8.0);
        let optional = matches!(from, WorktreeAsked::Commit(_));
        let label = if optional {
            "New branch (none: without a branch)"
        } else {
            "New branch"
        };
        name_field(ui, label, name);
        let named = name_problem(ui, name);
        ready &= named || (optional && name.trim().is_empty());
    }
    ui.add_space(4.0);
    crate::widgets::checkbox(ui, open, "Open it once made");
    ui.add_space(12.0);
    let go = || {
        let name = name.trim();
        let from = match from {
            WorktreeAsked::Branch(branch) => WorktreeSource::Branch(branch.clone()),
            WorktreeAsked::RemoteBranch { full, .. } => WorktreeSource::NewBranch {
                name: name.into(),
                at: full.clone(),
                track: true,
            },
            WorktreeAsked::Commit(commit) if name.is_empty() => WorktreeSource::Detached(*commit),
            WorktreeAsked::Commit(commit) => WorktreeSource::NewBranch {
                name: name.into(),
                at: commit.to_string().into(),
                track: false,
            },
        };
        Task::AddWorktree {
            path: target.clone().unwrap_or_default(),
            from,
            open: *open,
        }
    };
    buttons(ui, "Create worktree", ready, go)
}

/// Where a worktree typed as `typed` goes: `~` is the home folder, and a relative path is
/// beside the repository's work tree.
pub(super) fn worktree_path(typed: &str, work_tree: &Path, home: Option<&Path>) -> Option<PathBuf> {
    let typed = typed.trim();
    if typed.is_empty() {
        return None;
    }
    let path = super::home::expand_home(typed, home);
    if path.is_absolute() {
        Some(path)
    } else {
        Some(work_tree.parent().unwrap_or(work_tree).join(path))
    }
}

/// Whether to delete a remote's branch.
fn delete_remote_branch(ui: &mut Ui, remote: &BString, branch: &BString) -> Option<Answer> {
    let remote_shown = remote.to_str_lossy();
    confirm_danger(
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
    crate::widgets::bounded_text_area(
        ui,
        crate::widgets::text_area(message)
            .desired_rows(3)
            .desired_width(f32::INFINITY),
        220.0,
        "tag-message",
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
    confirm_danger(
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

/// Whether to sign `commits` of the current branch: what that makes again, and what stays.
fn sign_commits(ui: &mut Ui, pane: &TaskPane<'_>, commits: &[ObjectId]) -> Option<Answer> {
    let branch = pane.branch().unwrap_or_else(|| "HEAD".to_owned());
    let count = if commits.len() == 1 {
        "1 commit".to_owned()
    } else {
        format!("{} commits", commits.len())
    };
    title(ui, &format!("Sign {count}?"));
    says(
        ui,
        "Each is made again with the same changes, author, and message, signed with your key (as \
         your Profile sets it), with you as its committer, now: their names change.",
    );
    ui.add_space(6.0);
    says(
        ui,
        &format!(
            "Every commit of {branch} after them is made again too, its parent being new, and \
             signed as well. Nothing is checked out: your uncommitted changes stay, and merges \
             keep their resolutions."
        ),
    );
    ui.add_space(6.0);
    says(
        ui,
        &format!(
            "Pushed already? Pushing after takes a force push. Other branches and tags at these \
             commits stay at the old ones. Undo puts {branch} back."
        ),
    );
    ui.add_space(12.0);
    let commits = commits.to_vec();
    buttons_of(ui, ("Sign", Some("sign-commits")), true, || {
        Task::SignCommits { commits }
    })
}

/// Whether to stash the changes in the way of a switch, then switch.
fn stash_and_switch(
    ui: &mut Ui,
    to: &SwitchTo,
    paths: &[BString],
    untracked: bool,
) -> Option<Answer> {
    let named = named_paths(paths);
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

/// Whether to stash the changes in a pull's way, then pull: put back after (`--autostash`),
/// or, for files git doesn't track (which that leaves alone), kept in the stash list.
fn pull_stashing(
    ui: &mut Ui,
    mode: PullMode,
    paths: &[BString],
    untracked: bool,
) -> Option<Answer> {
    let named = named_paths(paths);
    if untracked {
        return confirm(
            ui,
            "Your files are in the way",
            &format!(
                "The pull brings {named}, which you have too, but git doesn't track. Stash your \
                 changes (untracked files too), then pull? They wait in the stash list: move \
                 your own files aside before you apply it."
            ),
            "Stash and pull",
            || Task::PullStashing {
                mode,
                untracked: true,
            },
        );
    }
    let what = if paths.is_empty() {
        "A pull that rebases needs the work tree clean, and you have changes not committed."
            .to_owned()
    } else {
        format!("Pulling would overwrite your changes to {named}.")
    };
    confirm(
        ui,
        "Your changes are in the way",
        &format!(
            "{what} Stash them, pull, and put them back? Where they meet the pull's changes, \
             you resolve the conflicts as after a merge. What you staged comes back not staged."
        ),
        "Stash, pull, and put back",
        || Task::PullStashing {
            mode,
            untracked: false,
        },
    )
}

/// A path by name, or how many files: "notes.txt", "3 files".
fn files_words(paths: &[BString]) -> String {
    match paths {
        [one] => one.to_str_lossy().into_owned(),
        _ => format!(
            "{} files",
            crate::format::count(u64::try_from(paths.len()).unwrap_or(u64::MAX))
        ),
    }
}

impl Dialog {
    /// Stashing the work tree's changes (every one, untracked files too, until unticked), or
    /// only those at `paths`.
    pub(super) const fn stash(paths: Vec<BString>) -> Self {
        Self::Stash {
            message: String::new(),
            untracked: true,
            paths,
        }
    }
}

/// Up to five paths by name, and how many more.
pub(super) fn named_paths(paths: &[BString]) -> String {
    const NAMED: usize = 5;
    let mut named: Vec<String> = paths
        .iter()
        .take(NAMED)
        .map(|path| path.to_str_lossy().into_owned())
        .collect();
    if paths.len() > NAMED {
        named.push(format!("{} more", paths.len().saturating_sub(NAMED)));
    }
    named.join(", ")
}

/// The work tree's changes to stash: a message, and whether untracked files go too; or the
/// changes at some paths, named.
fn stash(
    ui: &mut Ui,
    message: &mut String,
    untracked: &mut bool,
    paths: &[BString],
) -> Option<Answer> {
    if paths.is_empty() {
        title(ui, "Stash your changes");
    } else {
        title(ui, &format!("Stash the changes to {}", files_words(paths)));
        let said = if paths.len() == 1 {
            "Only its changes go into the stash, staged or not; the other changes stay.".to_owned()
        } else {
            format!(
                "Only these go into the stash, whether staged, not staged, or not tracked: \
                 {}. The other changes stay.",
                named_paths(paths)
            )
        };
        ui.add(egui::Label::new(muted(ui, said)).wrap());
        ui.add_space(4.0);
    }
    let entered = name_field(ui, "Message (optional)", message);
    if paths.is_empty() {
        ui.add_space(4.0);
        crate::widgets::checkbox(ui, untracked, "Files git doesn't track too");
    }
    ui.add_space(12.0);
    let go = || Task::Stash {
        message: message.trim().to_owned(),
        untracked: *untracked,
        paths: paths.to_vec(),
    };
    if entered {
        return Some(Answer::Run(go()));
    }
    buttons(ui, "Stash", true, go)
}

/// Whether `name` could be a new remote's: one git takes, not one already here.
fn remote_problem(ui: &mut Ui, pane: &TaskPane<'_>, name: &str) -> bool {
    let name = name.trim();
    let taken = pane.config.is_some_and(|config| {
        crate::tasks::remote_names(config)
            .iter()
            .any(|known| known == name)
    });
    if taken {
        let palette = Palette::of(ui.ctx());
        ui.label(
            RichText::new(format!("There is a remote called {name} already.")).color(palette.warn),
        );
        return false;
    }
    name_problem(ui, name)
}

/// Whether `url` could be an address: something, on one line.
fn url_ready(url: &str) -> bool {
    let url = url.trim();
    !url.is_empty() && !url.contains(['\n', '\r'])
}

/// A labelled one-line field that doesn't take the focus.
fn field(ui: &mut Ui, label: &str, text: &mut String) -> bool {
    let label = ui.label(muted(ui, label));
    let field = ui
        .add(crate::widgets::line_field(text).desired_width(f32::INFINITY))
        .labelled_by(label.id);
    field.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter))
}

/// A remote to add.
fn add_remote(
    ui: &mut Ui,
    pane: &TaskPane<'_>,
    name: &mut String,
    url: &mut String,
    fetch: &mut bool,
) -> Option<Answer> {
    title(ui, "Add a remote");
    says(
        ui,
        "Another repository this one fetches from and pushes to: a colleague's, or the one \
         yours was forked from.",
    );
    ui.add_space(8.0);
    let named = name_field(ui, "Name", name);
    let name_ready = remote_problem(ui, pane, name);
    ui.add_space(4.0);
    let addressed = field(ui, "Address (URL)", url);
    ui.add_space(4.0);
    crate::widgets::checkbox(ui, fetch, "Fetch from it now");
    ui.add_space(12.0);
    let ready = name_ready && url_ready(url);
    let go = || Task::AddRemote {
        name: name.trim().to_owned(),
        url: url.trim().to_owned(),
        fetch: *fetch,
    };
    if (named || addressed) && ready {
        return Some(Answer::Run(go()));
    }
    buttons(ui, "Add remote", ready, go)
}

/// A remote's addresses.
fn edit_remote(
    ui: &mut Ui,
    remote: &BString,
    url: &mut String,
    push: &mut String,
) -> Option<Answer> {
    title(ui, &format!("Change {}'s address", remote.to_str_lossy()));
    let entered = name_field(ui, "Fetches from", url);
    ui.add_space(4.0);
    let pushed = field(ui, "Pushes to (empty: where it fetches from)", push);
    ui.add_space(12.0);
    let push_ready = push.trim().is_empty() || url_ready(push);
    let ready = url_ready(url) && push_ready;
    let go = || Task::SetRemoteUrls {
        remote: remote.clone(),
        url: url.trim().to_owned(),
        push: Some(push.trim().to_owned()).filter(|push| !push.is_empty()),
    };
    if (entered || pushed) && ready {
        return Some(Answer::Run(go()));
    }
    buttons(ui, "Save", ready, go)
}

/// A remote's new name.
fn rename_remote(
    ui: &mut Ui,
    pane: &TaskPane<'_>,
    old: &BString,
    name: &mut String,
) -> Option<Answer> {
    title(ui, &format!("Rename the remote {}", old.to_str_lossy()));
    says(
        ui,
        "Its remote-tracking branches, and the branches following them, are renamed with it.",
    );
    ui.add_space(8.0);
    let entered = name_field(ui, "New name", name);
    let ready = remote_problem(ui, pane, name) && name.trim().as_bytes() != old.as_slice();
    ui.add_space(12.0);
    let go = || Task::RenameRemote {
        old: old.clone(),
        new: name.trim().to_owned(),
    };
    if entered && ready {
        return Some(Answer::Run(go()));
    }
    buttons(ui, "Rename", ready, go)
}

/// Whether to remove a worktree (again, saying what goes with it, when it has changes).
fn remove_worktree(ui: &mut Ui, pane: &TaskPane<'_>, path: &Path, changes: bool) -> Option<Answer> {
    let shown = crate::format::display_path(path, pane.home);
    let (said, yes) = if changes {
        (
            "It has changes not committed, or files git doesn't track: removing it deletes them.",
            "Remove anyway",
        )
    } else {
        (
            "Its folder is deleted; its branch, and the commits on it, stay.",
            "Remove",
        )
    };
    confirm_danger(
        ui,
        &format!("Remove the worktree at {shown}?"),
        said,
        yes,
        || Task::RemoveWorktree {
            path: path.to_path_buf(),
            force: changes,
        },
    )
}

/// Why a worktree is locked.
fn lock_worktree(
    ui: &mut Ui,
    pane: &TaskPane<'_>,
    path: &Path,
    reason: &mut String,
) -> Option<Answer> {
    let shown = crate::format::display_path(path, pane.home);
    title(ui, &format!("Lock the worktree at {shown}"));
    says(
        ui,
        "A locked worktree isn't pruned when its folder is missing (on a drive not always \
         there, say), nor moved or removed.",
    );
    ui.add_space(8.0);
    let entered = name_field(ui, "Why (optional)", reason);
    ui.add_space(12.0);
    let go = || Task::LockWorktree {
        path: path.to_path_buf(),
        reason: reason.trim().to_owned(),
    };
    if entered {
        return Some(Answer::Run(go()));
    }
    buttons_of(ui, ("Lock", Some("worktree-lock")), true, go)
}
