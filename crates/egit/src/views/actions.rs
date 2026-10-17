//! What can be done from the commit table: a commit's context menu (switch to it, branch or tag
//! it, bring it into the current branch, reset to it), a ref badge's (switch to a branch, merge
//! or rebase onto it, rename or delete it; push or delete a tag), double-clicking a branch to
//! switch to it, and the stash menu.
//!
//! Each choice asks for a task, or a dialog first ([`Ask`]); the history view carries it out.

use bstr::{BStr, BString, ByteSlice as _};
use egit_git::{ConfigSnapshot, ObjectId, RefKind, Reference, ResetMode, Stash, SwitchTo};
use egui::{Response, Ui};

use super::dialogs::Dialog;
use super::history::ID_DIGITS;
use crate::format;
use crate::history::Refs;
use crate::tasks::{Task, remote_names};
use crate::widgets::muted;

/// What a menu asked for.
#[derive(Debug)]
pub(super) enum Ask {
    Task(Task),
    Dialog(Dialog),
    /// The interactive rebase editor, from a commit (with one to reword).
    Rebase {
        from: ObjectId,
        reword: Option<ObjectId>,
    },
}

/// What the menus know of the repository.
pub(super) struct Menus<'a> {
    /// The current branch's short name (`None` when detached).
    pub(super) branch: Option<&'a BStr>,
    /// The commit `HEAD` is at.
    pub(super) head: Option<ObjectId>,
    /// The repository's configuration, once read (its remotes, read when a menu needs them).
    pub(super) config: Option<&'a ConfigSnapshot>,
    pub(super) refs: &'a Refs,
    /// Whether a task is running (nothing else starts meanwhile).
    pub(super) busy: bool,
    /// Whether an operation is in progress (the current branch can't change meanwhile).
    pub(super) operation: bool,
}

impl Menus<'_> {
    /// The current branch, as menus name it.
    fn current(&self) -> String {
        self.branch.map_or_else(
            || "HEAD".to_owned(),
            |branch| branch.to_str_lossy().into_owned(),
        )
    }

    /// Whether a task may start, and why not.
    const fn idle(&self) -> Result<(), &'static str> {
        if self.busy {
            Err("Another task is running")
        } else {
            Ok(())
        }
    }

    /// Whether the current branch may change, and why not.
    const fn can_change(&self) -> Result<(), &'static str> {
        if self.busy {
            Err("Another task is running")
        } else if self.operation {
            Err("Finish or abort the operation in progress first")
        } else {
            Ok(())
        }
    }

    /// The remotes configured, in order.
    fn remotes(&self) -> Vec<String> {
        self.config.map(remote_names).unwrap_or_default()
    }

    /// The remote a remote-tracking branch's short name belongs to, and its name there.
    fn split_remote<'n>(&self, name: &'n BStr) -> Option<(&'n BStr, &'n BStr)> {
        split_remote(&self.remotes(), name)
    }

    /// Where switching to `reference` goes: a local branch; a remote branch through the local
    /// one of its name (made, tracking it, unless there is one).
    fn switch_to(&self, reference: &Reference) -> Option<SwitchTo> {
        let short = reference.short_name();
        match reference.kind {
            RefKind::Branch => Some(SwitchTo::Branch(short.to_owned())),
            RefKind::RemoteBranch => {
                let (_, local) = self.split_remote(short)?;
                Some(if self.refs.has_branch(local) {
                    SwitchTo::Branch(local.to_owned())
                } else {
                    SwitchTo::Track {
                        remote_branch: short.to_owned(),
                        local: local.to_owned(),
                    }
                })
            }
            RefKind::Tag | RefKind::Other => None,
        }
    }
}

/// The remote a remote-tracking branch's short name (`origin/topic`) belongs to, and its name
/// there: the longest of `remotes` it starts with (remote names may hold `/`).
fn split_remote<'n>(remotes: &[String], name: &'n BStr) -> Option<(&'n BStr, &'n BStr)> {
    let remote = remotes
        .iter()
        .filter(|remote| {
            name.strip_prefix(remote.as_bytes())
                .is_some_and(|rest| rest.starts_with(b"/"))
        })
        .max_by_key(|remote| remote.len())?;
    let (remote, rest) = name.split_at(remote.len());
    Some((remote.as_bstr(), rest.get(1..)?.as_bstr()))
}

/// An item that may be disabled (with why on hover).
fn item(ui: &mut Ui, label: &str, allowed: Result<(), &str>) -> bool {
    let response = ui.add_enabled(allowed.is_ok(), egui::Button::new(label));
    let response = match allowed {
        Err(why) => response.on_disabled_hover_text(why),
        Ok(()) => response,
    };
    if response.clicked() {
        ui.close();
        true
    } else {
        false
    }
}

/// A commit's context menu.
pub(super) fn commit_menu(ui: &mut Ui, menus: &Menus<'_>, id: ObjectId, asked: &mut Option<Ask>) {
    let current = menus.current();
    let can = menus.can_change();
    let other = if menus.head == Some(id) {
        Err("It is where HEAD is")
    } else {
        can
    };
    let mut ask = |what| *asked = Some(what);
    if item(ui, "Switch here, without a branch", other) {
        ask(Ask::Task(Task::Switch(SwitchTo::Detached(id))));
    }
    if item(ui, "Create a branch here…", menus.idle()) {
        ask(Ask::Dialog(Dialog::CreateBranch {
            at: id,
            name: String::new(),
            switch: true,
        }));
    }
    if item(ui, "Create a tag here…", menus.idle()) {
        ask(Ask::Dialog(Dialog::CreateTag {
            at: id,
            name: String::new(),
            message: String::new(),
        }));
    }
    ui.separator();
    let shown = id.abbreviated(ID_DIGITS).to_string();
    let what = BString::from(id.to_string());
    if item(ui, &format!("Merge into {current}"), other) {
        ask(Ask::Task(Task::Merge {
            what: what.clone(),
            shown: shown.clone(),
        }));
    }
    if item(ui, &format!("Rebase {current} onto this commit"), other) {
        ask(Ask::Task(Task::Rebase { onto: what, shown }));
    }
    if item(ui, &format!("Cherry-pick onto {current}"), other) {
        ask(Ask::Task(Task::CherryPick(id)));
    }
    if item(ui, &format!("Revert in {current}"), can) {
        ask(Ask::Task(Task::Revert(id)));
    }
    rewrite_items(ui, menus, id, &mut ask);
    ui.menu_button(format!("Reset {current} to this commit"), |ui| {
        if item(ui, "Soft: keep the changes staged", other) {
            ask(Ask::Task(Task::Reset(id, ResetMode::Soft)));
        }
        if item(ui, "Mixed: keep the changes, unstaged", other) {
            ask(Ask::Task(Task::Reset(id, ResetMode::Mixed)));
        }
        if item(ui, "Hard: discard the changes…", can) {
            ask(Ask::Dialog(Dialog::HardReset { at: id }));
        }
    });
}

/// Rewriting the current branch from a commit: interactively, its message, squashing it into
/// its parent, or dropping it.
fn rewrite_items(ui: &mut Ui, menus: &Menus<'_>, id: ObjectId, ask: &mut impl FnMut(Ask)) {
    ui.separator();
    let can = if menus.branch.is_none() {
        Err("Switch to a branch first: rewriting history needs one")
    } else {
        menus.can_change()
    };
    if item(ui, "Rebase interactively from here…", can) {
        ask(Ask::Rebase {
            from: id,
            reword: None,
        });
    }
    if item(ui, "Edit the message…", can) {
        ask(Ask::Rebase {
            from: id,
            reword: Some(id),
        });
    }
    if item(ui, "Squash into its parent", can) {
        ask(Ask::Task(Task::SquashIntoParent(id)));
    }
    if item(ui, "Drop this commit…", can) {
        ask(Ask::Dialog(Dialog::DropCommit(id)));
    }
    ui.separator();
}

/// A ref badge's context menu.
pub(super) fn ref_menu(
    ui: &mut Ui,
    menus: &Menus<'_>,
    reference: &Reference,
    asked: &mut Option<Ask>,
) {
    let short = reference.short_name();
    let name = short.to_str_lossy();
    let current = menus.current();
    let is_current = reference.kind == RefKind::Branch && menus.branch == Some(short);
    let can = menus.can_change();
    let other = if is_current {
        Err("It is the current branch")
    } else {
        can
    };
    let mut ask = |what| *asked = Some(what);
    match reference.kind {
        RefKind::Branch | RefKind::RemoteBranch => {
            if let Some(to) = menus.switch_to(reference) {
                let label = match &to {
                    SwitchTo::Track { local, .. } => {
                        format!("Switch to {}, tracking {name}", local.to_str_lossy())
                    }
                    SwitchTo::Branch(local) => format!("Switch to {}", local.to_str_lossy()),
                    SwitchTo::NewBranch { .. } | SwitchTo::Detached(_) => String::new(),
                };
                if item(ui, &label, other) {
                    ask(Ask::Task(Task::Switch(to)));
                }
            }
            if item(ui, &format!("Merge {name} into {current}"), other) {
                ask(Ask::Task(Task::Merge {
                    what: reference.name.clone(),
                    shown: name.clone().into_owned(),
                }));
            }
            if item(ui, &format!("Rebase {current} onto {name}"), other) {
                ask(Ask::Task(Task::Rebase {
                    onto: reference.name.clone(),
                    shown: name.clone().into_owned(),
                }));
            }
            ui.separator();
            branch_items(ui, menus, reference, is_current, &mut ask);
        }
        RefKind::Tag => {
            let busy = menus.idle();
            let remotes = menus.remotes();
            if let Some(remote) = remotes
                .iter()
                .find(|remote| *remote == "origin")
                .or_else(|| remotes.first())
                && item(ui, &format!("Push {name} to {remote}"), busy)
            {
                ask(Ask::Task(Task::PushTag {
                    remote: remote.as_str().into(),
                    tag: short.to_owned(),
                }));
            }
            if item(ui, &format!("Delete the tag {name}…"), busy) {
                ask(Ask::Dialog(Dialog::DeleteTag(short.to_owned())));
            }
        }
        RefKind::Other => {}
    }
}

/// Renaming and deleting a branch (on its remote, for a remote's).
fn branch_items(
    ui: &mut Ui,
    menus: &Menus<'_>,
    reference: &Reference,
    is_current: bool,
    ask: &mut impl FnMut(Ask),
) {
    let short = reference.short_name();
    let name = short.to_str_lossy();
    let busy = menus.idle();
    if reference.kind == RefKind::RemoteBranch {
        if let Some((remote, branch)) = menus.split_remote(short)
            && item(
                ui,
                &format!(
                    "Delete {} on {}…",
                    branch.to_str_lossy(),
                    remote.to_str_lossy()
                ),
                busy,
            )
        {
            ask(Ask::Dialog(Dialog::DeleteRemoteBranch {
                remote: remote.to_owned(),
                branch: branch.to_owned(),
            }));
        }
        return;
    }
    if item(ui, &format!("Rename {name}…"), busy) {
        ask(Ask::Dialog(Dialog::RenameBranch {
            old: short.to_owned(),
            name: name.clone().into_owned(),
        }));
    }
    let deletable = if is_current {
        Err("It is the current branch")
    } else {
        busy
    };
    if item(ui, &format!("Delete {name}…"), deletable) {
        ask(Ask::Dialog(Dialog::DeleteBranch {
            name: short.to_owned(),
            unmerged: false,
        }));
    }
}

/// What double-clicking a ref badge asks: switching to a branch (a remote's through its local
/// one), unless it is the current one or the branch can't change now.
pub(super) fn ref_double_clicked(menus: &Menus<'_>, reference: &Reference) -> Option<Ask> {
    let is_current =
        reference.kind == RefKind::Branch && menus.branch == Some(reference.short_name());
    if is_current || menus.can_change().is_err() {
        return None;
    }
    menus
        .switch_to(reference)
        .map(|to| Ask::Task(Task::Switch(to)))
}

/// The stash menu, under `button`: stashing the work tree's changes, and each stash to apply,
/// pop, or drop.
pub(super) fn stash_menu(
    ui: &mut Ui,
    menus: &Menus<'_>,
    stashes: &[Stash],
    has_changes: bool,
    asked: &mut Option<Ask>,
) -> Response {
    let label = match stashes.len() {
        0 => format!("{}  Stash", crate::theme::icon::ARCHIVE),
        count => format!("{}  Stashes · {count}", crate::theme::icon::ARCHIVE),
    };
    let busy = menus.idle();
    let can_stash = if has_changes {
        busy
    } else {
        Err("There are no changes to stash")
    };
    let now = format::now();
    ui.menu_button(label, |ui| {
        ui.set_min_width(320.0);
        if item(ui, "Stash the changes…", can_stash) {
            *asked = Some(Ask::Dialog(Dialog::Stash {
                message: String::new(),
                untracked: true,
            }));
        }
        if stashes.is_empty() {
            return;
        }
        ui.separator();
        for stash in stashes {
            let when = u64::try_from(stash.time)
                .map_or_else(|_| String::new(), |time| format::ago(time, now));
            ui.label(format!("stash@{{{}}} · {when}", stash.index));
            ui.label(muted(ui, &stash.message));
            ui.horizontal(|ui| {
                for (label, task) in [
                    (
                        "Apply",
                        Task::ApplyStash {
                            stash: stash.clone(),
                            pop: false,
                        },
                    ),
                    (
                        "Pop",
                        Task::ApplyStash {
                            stash: stash.clone(),
                            pop: true,
                        },
                    ),
                ] {
                    if item(ui, label, busy) {
                        *asked = Some(Ask::Task(task));
                    }
                }
                if item(ui, "Drop…", busy) {
                    *asked = Some(Ask::Dialog(Dialog::DropStash(stash.clone())));
                }
            });
            ui.add_space(4.0);
        }
    })
    .response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_remote_branch_belongs_to_its_longest_remote() {
        let remotes = ["origin".to_owned(), "team".to_owned(), "team/eu".to_owned()];
        let split = |name: &str| {
            split_remote(&remotes, BStr::new(name))
                .map(|(remote, branch)| (remote.to_string(), branch.to_string()))
        };
        assert_eq!(split("origin/main"), Some(("origin".into(), "main".into())));
        assert_eq!(
            split("team/eu/fix/x"),
            Some(("team/eu".into(), "fix/x".into()))
        );
        assert_eq!(split("team/x"), Some(("team".into(), "x".into())));
        assert_eq!(split("originals/x"), None);
    }
}
