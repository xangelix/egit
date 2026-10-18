//! What can be done from the commit table: a commit's context menu (switch to it, branch, tag,
//! or make a worktree from it, bring it into the current branch, reset to it), a ref badge's
//! (switch to a branch, merge or rebase onto it, fast-forward; pull, push, or set the upstream of
//! a branch; make a worktree from it; rename or delete it, here or on its remote or both; copy
//! its name or link; pin it to the left, or show only some branches; push or delete a tag),
//! double-clicking a branch to switch to it, and the stash menu.
//!
//! Each choice asks for a task, or a dialog first ([`Ask`]); the history view carries it out.

use std::path::Path;

use bstr::{BStr, BString, ByteSlice as _};
use egit_forge::ForgeRepository;
use egit_git::{
    BisectMark, BisectTerms, Bisection, Changes, ConfigSnapshot, ObjectId, PullMode, PushOptions,
    RefKind, Reference, ResetMode, Stash, SwitchTo, patch_file_name,
};
use egui::{Response, RichText, Ui};

use super::dialogs::{Dialog, WorktreeAsked};
use super::history::ID_DIGITS;
use crate::forge::{RepoForge, request_noun, request_number};
use crate::format;
use crate::history::{Refs, Scope};
use crate::tasks::{ForwardSource, Task, remote_names, upstream_of};
use crate::theme::Weighted as _;
use crate::theme::{Palette, icon};
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
    /// A web page, in the browser.
    Open(String),
    /// A ref dragged onto another, at a point: what it can do is asked there.
    Dropped {
        dragged: Reference,
        onto: Reference,
        at: egui::Pos2,
    },
    /// The preferences changed (lists of files as a tree, say).
    Preferences(crate::preferences::Preferences),
    /// The pull requests, one selected (its repository's place in the forge's, its number).
    PullRequest(Option<(usize, u32)>),
    /// This much of the history shown.
    Scope(Scope),
    /// A commit's patch saved, under a name asked for in the desktop's dialog (`name`, offered).
    SavePatch {
        commit: ObjectId,
        name: String,
    },
    /// The commits from one up to `HEAD` saved as patches, in a folder asked for.
    SavePatches {
        from: ObjectId,
    },
    /// Uncommitted changes saved as a patch, under a name asked for.
    SaveChanges(Changes),
    /// Patches to apply, chosen in the desktop's dialog.
    ApplyFiles,
    /// A patch to apply, pasted.
    Paste,
    /// A ref's reflog (`HEAD`, or a branch's full name), in the commit table's place.
    Reflog(BString),
    /// The dialog opening a pull request from `branch`, pushed to `remote`'s `head_branch`,
    /// which is `head`.
    CreatePullRequest {
        branch: BString,
        remote: BString,
        head_branch: String,
        head: ForgeRepository,
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
    /// Where the bisection under way stands, if one is.
    pub(super) bisect: Option<&'a Bisection>,
    /// Its remotes on GitHub and GitLab, and their requests.
    pub(super) forge: &'a RepoForge,
    /// What of the history is shown, as asked: the branches soloed, the one pinned.
    pub(super) scope: &'a Scope,
    /// The repository's work tree (new worktrees go beside it).
    pub(super) work_tree: &'a Path,
    /// The user's home folder, shown as `~`.
    pub(super) home: Option<&'a Path>,
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
    pub(super) const fn idle(&self) -> Result<(), &'static str> {
        if self.busy {
            Err("Another task is running")
        } else {
            Ok(())
        }
    }

    /// Whether the current branch may change, and why not.
    pub(super) const fn can_change(&self) -> Result<(), &'static str> {
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

    /// A local branch's upstream: its remote, and its name there.
    fn upstream(&self, branch: &BStr) -> Option<(String, String)> {
        upstream_of(self.config?, &branch.to_str_lossy())
    }

    /// The local branch following a remote branch (of the same name), if one does.
    fn local_of<'n>(&self, remote_branch: &'n BStr) -> Option<&'n BStr> {
        let (remote, branch) = self.split_remote(remote_branch)?;
        let (its_remote, its_branch) = self.upstream(branch)?;
        (its_remote.as_bytes() == remote.as_bytes() && its_branch.as_bytes() == branch.as_bytes())
            .then_some(branch)
    }

    /// Where a new worktree named for `suffix` goes by default: beside the work tree, its
    /// folder's name and `suffix`.
    pub(super) fn worktree_path(&self, suffix: &str) -> String {
        let folder = self.work_tree.file_name().map_or_else(
            || "worktree".to_owned(),
            |name| name.to_string_lossy().into_owned(),
        );
        let suffix: String = suffix
            .chars()
            .map(|c| {
                if c == '/' || c.is_whitespace() {
                    '-'
                } else {
                    c
                }
            })
            .collect();
        let parent = self.work_tree.parent().unwrap_or(self.work_tree);
        format::display_path(&parent.join(format!("{folder}-{suffix}")), self.home)
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

/// A menu's item: its icon, in a column of its own, then its words (it is named by them); a
/// click closes the menu. Disabled, it says why on hover.
pub(crate) fn item(ui: &mut Ui, entry: (&str, &str), allowed: Result<(), &str>) -> bool {
    menu_entry(ui, entry, allowed, false)
}

/// A menu's item with a keyboard shortcut (written as a Mac writes it), shown at its right.
pub(crate) fn shortcut_item(ui: &mut Ui, (glyph, label): (&str, &str), keys: &'static str) -> bool {
    menu_rows(ui);
    let palette = Palette::of(ui.ctx());
    let button = egui::Button::new((RichText::new(glyph).color(palette.muted), label))
        .shortcut_text(crate::commands::keys_text(keys));
    let response = ui.add(button);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    if response.clicked() {
        ui.close();
        true
    } else {
        false
    }
}

/// A menu's item for what loses work (deleting, dropping, a hard reset): in the danger colour.
pub(crate) fn danger_item(ui: &mut Ui, entry: (&str, &str), allowed: Result<(), &str>) -> bool {
    menu_entry(ui, entry, allowed, true)
}

/// Draws an item; returns whether it was clicked.
fn menu_entry(
    ui: &mut Ui,
    (glyph, label): (&str, &str),
    allowed: Result<(), &str>,
    danger: bool,
) -> bool {
    menu_rows(ui);
    let palette = Palette::of(ui.ctx());
    let (glyph, words) = if danger {
        (
            RichText::new(glyph).color(palette.danger),
            RichText::new(label).color(palette.danger),
        )
    } else {
        (
            RichText::new(glyph).color(palette.muted),
            RichText::new(label),
        )
    };
    let response = ui.add_enabled(allowed.is_ok(), egui::Button::new((glyph, words)));
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, allowed.is_ok(), label)
    });
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

/// A menu's submenu: its icon and words, like an item's, then an arrow; `add_contents` in the
/// submenu it opens.
pub(crate) fn submenu(
    ui: &mut Ui,
    (glyph, label): (&str, &str),
    add_contents: impl FnOnce(&mut Ui),
) {
    use egui::containers::menu::SubMenuButton;

    menu_rows(ui);
    let palette = Palette::of(ui.ctx());
    let button = egui::Button::new((RichText::new(glyph).color(palette.muted), label))
        .right_text(SubMenuButton::RIGHT_ARROW);
    let (response, _) = SubMenuButton::from_button(button).ui(ui, add_contents);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
}

/// A group's header in a menu: its name in small capitals, muted, over its items, which it
/// lines up with.
pub(crate) fn menu_header(ui: &mut Ui, title: &str) {
    menu_rows(ui);
    let palette = Palette::of(ui.ctx());
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(ui.spacing().button_padding.x);
        ui.add(
            egui::Label::new(
                RichText::new(title.to_uppercase())
                    .size(crate::theme::SMALL)
                    .semibold()
                    .extra_letter_spacing(0.6)
                    .color(palette.muted),
            )
            .selectable(false),
        );
    });
    ui.add_space(4.0);
}

/// Lays a menu's rows out: touching (each a whole target, the pointer never between two),
/// their icons and words inset from the highlight's edge.
pub(crate) fn menu_rows(ui: &mut Ui) {
    let spacing = ui.spacing_mut();
    spacing.item_spacing.y = 0.0;
    spacing.button_padding = egui::vec2(8.0, 0.0);
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
    here_items(ui, menus, id, other, &mut ask);
    ui.separator();
    into_items(ui, menus, id, (other, can), &mut ask);
    rewrite_items(ui, menus, id, &mut ask);
    bisect_menu(ui, menus, id, &mut ask);
    let reset = format!("Reset {current} to this commit");
    submenu(ui, (icon::ARROW_U_UP_LEFT, &reset), |ui| {
        if item(
            ui,
            (icon::ARROW_U_UP_LEFT, "Soft: keep the changes staged"),
            other,
        ) {
            ask(Ask::Task(Task::Reset(id, ResetMode::Soft)));
        }
        if item(
            ui,
            (icon::ARROW_U_UP_LEFT, "Mixed: keep the changes, unstaged"),
            other,
        ) {
            ask(Ask::Task(Task::Reset(id, ResetMode::Mixed)));
        }
        if danger_item(ui, (icon::TRASH, "Hard: discard the changes…"), can) {
            ask(Ask::Dialog(Dialog::HardReset { at: id }));
        }
    });
    if let Some(repository) = menus.forge.repositories.first() {
        ui.separator();
        let forge = repository.host().kind().name();
        if item(
            ui,
            (icon::ARROW_SQUARE_OUT, &format!("Open on {forge}")),
            Ok(()),
        ) {
            ask(Ask::Open(repository.commit_url(id)));
        }
        if item(
            ui,
            (icon::LINK, &format!("Copy link to this commit on {forge}")),
            Ok(()),
        ) {
            ui.ctx().copy_text(repository.commit_url(id));
        }
    }
}

/// What can be made at a commit: switching there detached, a branch, a tag, a worktree.
fn here_items(
    ui: &mut Ui,
    menus: &Menus<'_>,
    id: ObjectId,
    other: Result<(), &str>,
    ask: &mut impl FnMut(Ask),
) {
    if item(
        ui,
        (icon::GIT_COMMIT, "Switch here, without a branch"),
        other,
    ) {
        ask(Ask::Task(Task::Switch(SwitchTo::Detached(id))));
    }
    if item(
        ui,
        (icon::GIT_BRANCH, "Create a branch here…"),
        menus.idle(),
    ) {
        ask(Ask::Dialog(Dialog::CreateBranch {
            at: id,
            name: String::new(),
            switch: true,
        }));
    }
    if item(ui, (icon::TAG, "Create a tag here…"), menus.idle()) {
        ask(Ask::Dialog(Dialog::CreateTag {
            at: id,
            name: String::new(),
            message: String::new(),
        }));
    }
    if item(
        ui,
        (icon::TREE_STRUCTURE, "Create a worktree here…"),
        menus.idle(),
    ) {
        ask(Ask::Dialog(Dialog::AddWorktree {
            from: WorktreeAsked::Commit(id),
            path: menus.worktree_path(&id.abbreviated(ID_DIGITS).to_string()),
            name: String::new(),
            open: true,
        }));
    }
}

/// Bringing a commit into the current branch: merging it, rebasing onto it (keeping merges or
/// not), cherry-picking it, reverting it (`other` allows all but reverting, which `can`
/// allows: reverting HEAD itself is fine).
fn into_items(
    ui: &mut Ui,
    menus: &Menus<'_>,
    id: ObjectId,
    (other, can): (Result<(), &str>, Result<(), &str>),
    ask: &mut impl FnMut(Ask),
) {
    let current = menus.current();
    menu_header(ui, &format!("Into {current}"));
    let shown = id.abbreviated(ID_DIGITS).to_string();
    let what = BString::from(id.to_string());
    if item(
        ui,
        (icon::GIT_MERGE, &format!("Merge into {current}")),
        other,
    ) {
        ask(Ask::Task(Task::Merge {
            what: what.clone(),
            shown: shown.clone(),
        }));
    }
    for (label, keep_merges) in [
        (format!("Rebase {current} onto this commit"), false),
        (
            format!("Rebase {current} onto this commit, keeping merges"),
            true,
        ),
    ] {
        if item(ui, (icon::STACK, &label), other) {
            ask(Ask::Task(Task::Rebase {
                onto: what.clone(),
                shown: shown.clone(),
                keep_merges,
            }));
        }
    }
    if item(
        ui,
        (icon::CHERRIES, &format!("Cherry-pick onto {current}")),
        other,
    ) {
        ask(Ask::Task(Task::CherryPick(id)));
    }
    if item(
        ui,
        (
            icon::ARROW_COUNTER_CLOCKWISE,
            &format!("Revert in {current}"),
        ),
        can,
    ) {
        ask(Ask::Task(Task::Revert(id)));
    }
}

/// Rewriting the current branch from a commit: interactively, its message, squashing it into
/// its parent, or dropping it.
fn rewrite_items(ui: &mut Ui, menus: &Menus<'_>, id: ObjectId, ask: &mut impl FnMut(Ask)) {
    ui.separator();
    menu_header(ui, &format!("Rewrite {}", menus.current()));
    let can = if menus.branch.is_none() {
        Err("Switch to a branch first: rewriting history needs one")
    } else {
        menus.can_change()
    };
    if item(
        ui,
        (icon::LIST_NUMBERS, "Rebase interactively from here…"),
        can,
    ) {
        ask(Ask::Rebase {
            from: id,
            reword: None,
        });
    }
    if item(ui, (icon::PENCIL_SIMPLE, "Edit the message…"), can) {
        ask(Ask::Rebase {
            from: id,
            reword: Some(id),
        });
    }
    if item(
        ui,
        (icon::ARROWS_IN_LINE_VERTICAL, "Squash into its parent"),
        can,
    ) {
        ask(Ask::Task(Task::SquashIntoParent(id)));
    }
    if danger_item(ui, (icon::TRASH, "Drop this commit…"), can) {
        ask(Ask::Dialog(Dialog::DropCommit(id)));
    }
    ui.separator();
}

/// Bisecting from a commit: marking it bad or good (the bisection's own words for them, when
/// it has some), which starts one when none is under way; or skipping it.
fn bisect_menu(ui: &mut Ui, menus: &Menus<'_>, id: ObjectId, ask: &mut impl FnMut(Ask)) {
    submenu(ui, (icon::BUG, "Bisect"), |ui| {
        let (terms, allowed) = menus.bisect.map_or_else(
            || (BisectTerms::default(), menus.can_change()),
            |bisection| (bisection.terms.clone(), menus.idle()),
        );
        let marks = [
            (
                BisectMark::Bad,
                icon::X_CIRCLE,
                format!("Mark as {}", terms.bad),
            ),
            (
                BisectMark::Good,
                icon::CHECK_CIRCLE,
                format!("Mark as {}", terms.good),
            ),
            (
                BisectMark::Skip,
                icon::SKIP_FORWARD,
                "Skip it (it can't be tested)".to_owned(),
            ),
        ];
        if menus.bisect.is_none() {
            ui.label(muted(
                ui,
                "Mark a commit with the change you're after, and one without it: git then \
                 checks out commits between them for you to test.",
            ));
        }
        for (mark, glyph, label) in marks {
            let allowed = if mark == BisectMark::Skip && menus.bisect.is_none() {
                Err("Skipping is for a bisection under way")
            } else {
                allowed
            };
            if item(ui, (glyph, &label), allowed) {
                ask(Ask::Task(Task::Bisect { mark, commit: id }));
            }
        }
    });
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
                if item(ui, (icon::CHECK_CIRCLE, &label), other) {
                    ask(Ask::Task(Task::Switch(to)));
                }
            }
            ui.separator();
            ref_into_items(ui, menus, reference, (is_current, other), &mut ask);
            if reference.kind == RefKind::Branch {
                ui.separator();
                remote_items(ui, menus, short, is_current, &mut ask);
            }
            pull_request_items(ui, menus, reference, &mut ask);
            ui.separator();
            worktree_item(ui, menus, reference, is_current, &mut ask);
            ui.separator();
            branch_items(ui, menus, reference, is_current, &mut ask);
            ui.separator();
            copy_items(ui, menus, reference);
            ui.separator();
            graph_items(ui, menus, reference, &mut ask);
            ui.separator();
            if item(
                ui,
                (
                    icon::CLOCK_COUNTER_CLOCKWISE,
                    &format!("Show {name}'s reflog"),
                ),
                Ok(()),
            ) {
                ask(Ask::Reflog(reference.name.clone()));
            }
        }
        RefKind::Tag => {
            let busy = menus.idle();
            let remotes = menus.remotes();
            if let Some(remote) = remotes
                .iter()
                .find(|remote| *remote == "origin")
                .or_else(|| remotes.first())
                && item(
                    ui,
                    (icon::ARROW_UP, &format!("Push {name} to {remote}")),
                    busy,
                )
            {
                ask(Ask::Task(Task::PushTag {
                    remote: remote.as_str().into(),
                    tag: short.to_owned(),
                }));
            }
            if danger_item(ui, (icon::TRASH, &format!("Delete the tag {name}…")), busy) {
                ask(Ask::Dialog(Dialog::DeleteTag(short.to_owned())));
            }
        }
        RefKind::Other => {}
    }
}

/// A branch's pull (or merge) request, when one of the remotes is on GitHub or GitLab: the
/// open one bringing it
/// in, or opening one (once it is pushed there).
fn pull_request_items(
    ui: &mut Ui,
    menus: &Menus<'_>,
    reference: &Reference,
    ask: &mut impl FnMut(Ask),
) {
    let forge = menus.forge;
    if forge.repositories.is_empty() {
        return;
    }
    let short = reference.short_name();
    // Where the branch is on a remote: its upstream, or the remote's branch itself.
    let pushed = match reference.kind {
        RefKind::Branch => menus.config.and_then(|config| {
            let (remote, branch) = upstream_of(config, &short.to_str_lossy())?;
            Some((BString::from(remote), branch))
        }),
        _ => menus
            .split_remote(short)
            .map(|(remote, branch)| (remote.to_owned(), branch.to_str_lossy().into_owned())),
    };
    let on_forge = pushed.as_ref().and_then(|(remote, branch)| {
        Some((remote, branch, forge.repository_of(remote.as_bstr())?))
    });
    ui.separator();
    match on_forge {
        Some((_, branch, head)) if let Some(open) = forge.pull_request_for(head, branch) => {
            let kind = head.host().kind();
            let shown = format!(
                "Show {} {}",
                request_noun(kind),
                request_number(kind, open.1)
            );
            if item(ui, (icon::GIT_PULL_REQUEST, &shown), Ok(())) {
                ask(Ask::PullRequest(Some(open)));
            }
        }
        Some((remote, branch, head)) if reference.kind == RefKind::Branch => {
            let create = format!("Create {}…", request_noun(head.host().kind()));
            if item(ui, (icon::GIT_PULL_REQUEST, &create), menus.idle()) {
                ask(Ask::CreatePullRequest {
                    branch: short.to_owned(),
                    remote: remote.clone(),
                    head_branch: branch.clone(),
                    head: head.clone(),
                });
            }
        }
        None if reference.kind == RefKind::Branch => {
            item(
                ui,
                (
                    icon::GIT_PULL_REQUEST,
                    &format!("Create {}…", request_noun(forge.kind())),
                ),
                Err("Push it to a remote on GitHub or GitLab first"),
            );
        }
        Some(_) | None => {}
    }
}

/// What a ref dragged onto another can do, git merging into and rebasing the branch checked
/// out: dropped on the current branch, merging the dragged one into it, rebasing it onto that,
/// or fast-forwarding it; the current branch dropped on another, rebasing it onto that (or
/// keeping merges), merging that in, or fast-forwarding; else fast-forwarding the branch
/// dropped on, or switching to it first.
pub(super) fn drop_items(
    ui: &mut Ui,
    menus: &Menus<'_>,
    (dragged, onto): (&Reference, &Reference),
    ask: &mut impl FnMut(Ask),
) {
    let from = dragged.short_name().to_str_lossy().into_owned();
    let to = onto.short_name().to_str_lossy().into_owned();
    menu_header(ui, &format!("{from} onto {to}"));
    for (glyph, label, task, allowed) in drop_offers(menus, (dragged, onto), (&from, &to)) {
        let allowed = allowed.as_ref().copied().map_err(String::as_str);
        if item(ui, (glyph, &label), allowed)
            && let Some(task) = task
        {
            ask(Ask::Task(task));
        }
    }
}

/// An item a drop offers: its icon, its words, its task (none: only said), and whether it can
/// be done now (why not).
type Offer = (&'static str, String, Option<Task>, Result<(), String>);

/// What [`drop_items`] offers for `dragged` dropped onto `onto` (named `from` and `to`).
fn drop_offers(
    menus: &Menus<'_>,
    (dragged, onto): (&Reference, &Reference),
    (from, to): (&str, &str),
) -> Vec<Offer> {
    let current = |reference: &Reference| {
        reference.kind == RefKind::Branch && menus.branch == Some(reference.short_name())
    };
    let allowed = menus.can_change().map_err(str::to_owned);
    let merge = |what: &Reference, shown: &str| {
        Some(Task::Merge {
            what: what.name.clone(),
            shown: shown.to_owned(),
        })
    };
    let rebase = |onto: &Reference, shown: &str, keep_merges| {
        Some(Task::Rebase {
            onto: onto.name.clone(),
            shown: shown.to_owned(),
            keep_merges,
        })
    };
    let forward = |to: &Reference, shown: &str| {
        Some(Task::FastForward {
            to: to.name.clone(),
            shown: shown.to_owned(),
        })
    };
    if current(onto) {
        return vec![
            (
                icon::GIT_MERGE,
                format!("Merge {from} into {to}"),
                merge(dragged, from),
                allowed.clone(),
            ),
            (
                icon::STACK,
                format!("Rebase {to} onto {from}"),
                rebase(dragged, from, false),
                allowed.clone(),
            ),
            (
                icon::FAST_FORWARD,
                format!("Fast-forward {to} to {from}"),
                forward(dragged, from),
                allowed,
            ),
        ];
    }
    if current(dragged) {
        return vec![
            (
                icon::STACK,
                format!("Rebase {from} onto {to}"),
                rebase(onto, to, false),
                allowed.clone(),
            ),
            (
                icon::STACK,
                format!("Rebase {from} onto {to}, keeping merges"),
                rebase(onto, to, true),
                allowed.clone(),
            ),
            (
                icon::GIT_MERGE,
                format!("Merge {to} into {from}"),
                merge(onto, to),
                allowed.clone(),
            ),
            (
                icon::FAST_FORWARD,
                format!("Fast-forward {from} to {to}"),
                forward(onto, to),
                allowed,
            ),
        ];
    }
    elsewhere_offers(menus, (dragged, onto), (from, to))
}

/// What a ref dropped onto a branch not checked out offers: fast-forwarding that branch, and
/// switching to it (merging into it waits for that).
fn elsewhere_offers(
    menus: &Menus<'_>,
    (dragged, onto): (&Reference, &Reference),
    (from, to): (&str, &str),
) -> Vec<Offer> {
    let mut offers = Vec::with_capacity(3);
    if onto.kind == RefKind::Branch {
        offers.push((
            icon::FAST_FORWARD,
            format!("Fast-forward {to} to {from}"),
            Some(Task::FastForwardBranch {
                branch: onto.short_name().to_owned(),
                from: ForwardSource::Here(dragged.name.clone()),
                shown: from.to_owned(),
            }),
            menus.idle().map_err(str::to_owned),
        ));
    }
    offers.push((
        icon::GIT_MERGE,
        format!("Merge {from} into {to}"),
        None,
        Err(format!(
            "git merges into the branch checked out: switch to {to} first"
        )),
    ));
    if let Some(switch) = menus.switch_to(onto) {
        offers.push((
            icon::ARROW_BEND_UP_RIGHT,
            format!("Switch to {to}"),
            Some(Task::Switch(switch)),
            menus.can_change().map_err(str::to_owned),
        ));
    }
    offers
}

/// Bringing a branch into the current one: merging it, rebasing onto it (keeping merges or
/// not), fast-forwarding to it; `other` allows them (not for the current branch itself).
fn ref_into_items(
    ui: &mut Ui,
    menus: &Menus<'_>,
    reference: &Reference,
    (is_current, other): (bool, Result<(), &str>),
    ask: &mut impl FnMut(Ask),
) {
    let current = menus.current();
    let name = reference.short_name().to_str_lossy();
    menu_header(ui, &format!("Into {current}"));
    if item(
        ui,
        (icon::GIT_MERGE, &format!("Merge {name} into {current}")),
        other,
    ) {
        ask(Ask::Task(Task::Merge {
            what: reference.name.clone(),
            shown: name.clone().into_owned(),
        }));
    }
    for (label, keep_merges) in [
        (format!("Rebase {current} onto {name}"), false),
        (
            format!("Rebase {current} onto {name}, keeping merges"),
            true,
        ),
    ] {
        if item(ui, (icon::STACK, &label), other) {
            ask(Ask::Task(Task::Rebase {
                onto: reference.name.clone(),
                shown: name.clone().into_owned(),
                keep_merges,
            }));
        }
    }
    forward_items(ui, menus, reference, is_current, ask);
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
            && danger_item(
                ui,
                (
                    icon::TRASH,
                    &format!(
                        "Delete {} on {}…",
                        branch.to_str_lossy(),
                        remote.to_str_lossy()
                    ),
                ),
                busy,
            )
        {
            ask(Ask::Dialog(Dialog::DeleteRemoteBranch {
                remote: remote.to_owned(),
                branch: branch.to_owned(),
            }));
        }
        if let Some(local) = menus.local_of(short)
            && let Some((remote, branch)) = menus.split_remote(short)
        {
            let allowed = if menus.branch == Some(local) {
                Err("Its branch here is the current one")
            } else {
                busy
            };
            if danger_item(
                ui,
                (
                    icon::TRASH,
                    &format!("Delete {} and {name}…", local.to_str_lossy()),
                ),
                allowed,
            ) {
                ask(Ask::Dialog(Dialog::DeleteBranch {
                    name: local.to_owned(),
                    unmerged: false,
                    remote: Some((remote.to_owned(), branch.to_owned())),
                }));
            }
        }
        return;
    }
    if item(ui, (icon::PENCIL_SIMPLE, &format!("Rename {name}…")), busy) {
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
    if danger_item(ui, (icon::TRASH, &format!("Delete {name}…")), deletable) {
        ask(Ask::Dialog(Dialog::DeleteBranch {
            name: short.to_owned(),
            unmerged: false,
            remote: None,
        }));
    }
    if let Some((remote, branch)) = menus.upstream(short)
        && menus.refs.has(
            format!("refs/remotes/{remote}/{branch}")
                .as_bytes()
                .as_bstr(),
        )
        && danger_item(
            ui,
            (
                icon::TRASH,
                &format!("Delete {name} and {remote}/{branch}…"),
            ),
            deletable,
        )
    {
        ask(Ask::Dialog(Dialog::DeleteBranch {
            name: short.to_owned(),
            unmerged: false,
            remote: Some((remote.into(), branch.into())),
        }));
    }
}

/// Fast-forwarding the current branch to this one; and a remote branch's local branch to it.
fn forward_items(
    ui: &mut Ui,
    menus: &Menus<'_>,
    reference: &Reference,
    is_current: bool,
    ask: &mut impl FnMut(Ask),
) {
    let short = reference.short_name();
    let name = short.to_str_lossy().into_owned();
    if !is_current
        && item(
            ui,
            (
                icon::FAST_FORWARD,
                &format!("Fast-forward {} to {name}", menus.current()),
            ),
            menus.can_change(),
        )
    {
        ask(Ask::Task(Task::FastForward {
            to: reference.name.clone(),
            shown: name.clone(),
        }));
    }
    if reference.kind == RefKind::RemoteBranch
        && let Some(local) = menus.local_of(short)
        && menus.branch != Some(local)
        && item(
            ui,
            (
                icon::FAST_FORWARD,
                &format!("Fast-forward {} to {name}", local.to_str_lossy()),
            ),
            menus.idle(),
        )
    {
        ask(Ask::Task(Task::FastForwardBranch {
            branch: local.to_owned(),
            from: ForwardSource::Here(reference.name.clone()),
            shown: name,
        }));
    }
}

/// A local branch's exchanges with its upstream: pulling (the current branch as configured,
/// another by fast-forward only), pushing (where it has never gone: asked first), and setting
/// the upstream.
fn remote_items(
    ui: &mut Ui,
    menus: &Menus<'_>,
    branch: &BStr,
    is_current: bool,
    ask: &mut impl FnMut(Ask),
) {
    let name = branch.to_str_lossy().into_owned();
    let upstream = menus.upstream(branch);
    let no_upstream = Err("It has no upstream: set one, or push it");
    if is_current {
        let allowed = if upstream.is_some() {
            menus.can_change()
        } else {
            no_upstream
        };
        if item(ui, (icon::ARROW_DOWN, "Pull"), allowed) {
            ask(Ask::Task(Task::Pull(PullMode::AsConfigured)));
        }
    } else {
        let allowed = if upstream.is_some() {
            menus.idle()
        } else {
            no_upstream
        };
        if item(
            ui,
            (
                icon::ARROW_DOWN,
                &format!("Pull {name} (fast-forward only)"),
            ),
            allowed,
        ) && let Some((remote, remote_branch)) = &upstream
        {
            ask(Ask::Task(Task::FastForwardBranch {
                branch: branch.to_owned(),
                from: ForwardSource::Remote {
                    remote: remote.as_str().into(),
                    branch: remote_branch.as_str().into(),
                },
                shown: format!("{remote}/{remote_branch}"),
            }));
        }
    }
    let remotes = menus.remotes();
    if let Some((remote, remote_branch)) = &upstream {
        if item(
            ui,
            (
                icon::ARROW_UP,
                &format!("Push {name} to {remote}/{remote_branch}"),
            ),
            menus.idle(),
        ) {
            ask(Ask::Task(Task::Push(PushOptions {
                remote: remote.as_str().into(),
                branch: branch.to_owned(),
                remote_branch: remote_branch.as_str().into(),
                set_upstream: false,
                force: false,
            })));
        }
    } else {
        let allowed = if remotes.is_empty() {
            Err("The repository has no remote")
        } else {
            menus.idle()
        };
        if item(ui, (icon::ARROW_UP, &format!("Push {name}…")), allowed) {
            let remote = remotes
                .iter()
                .find(|remote| *remote == "origin")
                .or_else(|| remotes.first())
                .cloned()
                .unwrap_or_default();
            ask(Ask::Dialog(Dialog::FirstPush(PushOptions {
                remote: remote.into(),
                branch: branch.to_owned(),
                remote_branch: branch.to_owned(),
                set_upstream: true,
                force: false,
            })));
        }
    }
    if item(ui, (icon::LINK, "Set upstream…"), menus.idle()) {
        let had = upstream.map(|(remote, branch)| format!("{remote}/{branch}"));
        let choices: Vec<String> = menus
            .refs
            .remote_branches()
            .map(|name| name.to_str_lossy().into_owned())
            .collect();
        // Offered first: where it is, or its namesake on the first remote.
        let namesake = remotes
            .first()
            .map(|remote| format!("{remote}/{name}"))
            .filter(|namesake| choices.contains(namesake));
        ask(Ask::Dialog(Dialog::SetUpstream {
            branch: branch.to_owned(),
            upstream: had.clone().or(namesake).unwrap_or_default(),
            had,
            choices,
        }));
    }
}

/// A worktree checking out a local branch (not the current one), or a remote's through a new
/// branch following it.
fn worktree_item(
    ui: &mut Ui,
    menus: &Menus<'_>,
    reference: &Reference,
    is_current: bool,
    ask: &mut impl FnMut(Ask),
) {
    let short = reference.short_name();
    let name = short.to_str_lossy();
    let allowed = if is_current {
        Err("It is checked out here")
    } else {
        menus.idle()
    };
    if !item(
        ui,
        (
            icon::TREE_STRUCTURE,
            &format!("Create a worktree from {name}…"),
        ),
        allowed,
    ) {
        return;
    }
    let (from, branch) = match reference.kind {
        RefKind::RemoteBranch => {
            let local = menus
                .split_remote(short)
                .map_or(short, |(_, branch)| branch);
            (
                WorktreeAsked::RemoteBranch {
                    full: reference.name.clone(),
                    short: short.to_owned(),
                },
                local.to_str_lossy().into_owned(),
            )
        }
        _ => (WorktreeAsked::Branch(short.to_owned()), String::new()),
    };
    let suffix = if branch.is_empty() {
        &name
    } else {
        branch.as_str()
    };
    ask(Ask::Dialog(Dialog::AddWorktree {
        path: menus.worktree_path(suffix),
        from,
        name: branch,
        open: true,
    }));
}

/// Copying a branch's name, and a link to it on its forge (where its remote is GitHub or
/// GitLab: a local branch's upstream's).
fn copy_items(ui: &mut Ui, menus: &Menus<'_>, reference: &Reference) {
    let short = reference.short_name();
    if item(ui, (icon::COPY, "Copy branch name"), Ok(())) {
        ui.ctx().copy_text(short.to_str_lossy().into_owned());
    }
    let on_remote = match reference.kind {
        RefKind::RemoteBranch => menus
            .split_remote(short)
            .map(|(remote, branch)| (remote.to_owned(), branch.to_str_lossy().into_owned())),
        _ => menus
            .upstream(short)
            .map(|(remote, branch)| (BString::from(remote), branch)),
    };
    if let Some((remote, branch)) = on_remote
        && let Some(repository) = menus.forge.repository_of(remote.as_bstr())
        && item(
            ui,
            (
                icon::LINK,
                &format!(
                    "Copy link to {branch} on {}",
                    repository.host().kind().name()
                ),
            ),
            Ok(()),
        )
    {
        ui.ctx().copy_text(repository.branch_url(&branch));
    }
}

/// The graph: this branch pinned to the left (or not any more), only some branches' history
/// shown (this one alone, too, or not), or every branch's again.
fn graph_items(ui: &mut Ui, menus: &Menus<'_>, reference: &Reference, ask: &mut impl FnMut(Ask)) {
    let scope = menus.scope;
    let full = &reference.name;
    let name = reference.short_name().to_str_lossy();
    let mut asked = |scope: Scope| ask(Ask::Scope(scope));
    menu_header(ui, "In the graph");
    if scope.pinned.as_ref() == Some(full) {
        if item(ui, (icon::PUSH_PIN_SLASH, &format!("Unpin {name}")), Ok(())) {
            asked(Scope {
                pinned: None,
                ..scope.clone()
            });
        }
    } else if item(
        ui,
        (icon::PUSH_PIN, &format!("Pin {name} to the left")),
        Ok(()),
    ) {
        asked(Scope {
            pinned: Some(full.clone()),
            ..scope.clone()
        });
    }
    if scope.only.is_empty() {
        let checked_out =
            reference.kind == RefKind::Branch && menus.branch == Some(reference.short_name());
        let hiding = if checked_out {
            Err("The branch checked out always shows")
        } else {
            Ok(())
        };
        if item(
            ui,
            (icon::EYE_SLASH, &format!("Hide {name} from the graph")),
            hiding,
        ) {
            asked(scope.with_hidden(full.as_bstr(), true));
        }
        if item(ui, (icon::FUNNEL, &format!("Show only {name}")), Ok(())) {
            asked(Scope {
                only: vec![full.clone()],
                ..scope.clone()
            });
        }
    } else if scope.only.contains(full) {
        if item(ui, (icon::EYE_SLASH, &format!("Hide {name}")), Ok(())) {
            let only = scope
                .only
                .iter()
                .filter(|shown| *shown != full)
                .cloned()
                .collect();
            asked(Scope {
                only,
                ..scope.clone()
            });
        }
    } else if item(ui, (icon::EYE, &format!("Show {name} too")), Ok(())) {
        let mut only = scope.only.clone();
        only.push(full.clone());
        asked(Scope {
            only,
            ..scope.clone()
        });
    }
    if !scope.only.is_empty() && item(ui, (icon::EYE, "Show every branch"), Ok(())) {
        asked(Scope {
            only: Vec::new(),
            ..scope.clone()
        });
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

/// A commit's patches: saved, copied, or saved with the commits after it up to `HEAD`.
pub(super) fn patch_items(
    ui: &mut Ui,
    menus: &Menus<'_>,
    id: ObjectId,
    subject: Option<&str>,
    asked: &mut Option<Ask>,
) {
    if item(ui, (icon::FLOPPY_DISK, "Save as a patch…"), menus.idle()) {
        let subject = subject.map_or_else(|| id.abbreviated(ID_DIGITS).to_string(), str::to_owned);
        *asked = Some(Ask::SavePatch {
            commit: id,
            name: patch_file_name(&subject),
        });
    }
    if item(ui, (icon::COPY, "Copy as a patch"), menus.idle()) {
        *asked = Some(Ask::Task(Task::CopyPatch(id)));
    }
    if item(
        ui,
        (icon::FLOPPY_DISK, "Save this and later commits as patches…"),
        menus.idle(),
    ) {
        *asked = Some(Ask::SavePatches { from: id });
    }
}

/// What the work tree holds, for the patches menu: whether anything is staged, whether any
/// tracked file changed, and whether there is a commit yet.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct WorkChanges {
    pub(super) staged: bool,
    pub(super) tracked: bool,
    pub(super) has_commit: bool,
}

/// The patches menu, beside the commit count: patches to apply (from files, or pasted), and the
/// uncommitted changes as a patch (copied, or saved), staged or all.
pub(super) fn patches_menu(
    ui: &mut Ui,
    menus: &Menus<'_>,
    changes: WorkChanges,
    look: crate::widgets::Look,
    asked: &mut Option<Ask>,
) -> Response {
    crate::widgets::tool_menu(ui, crate::theme::icon::GIT_DIFF, "Patches", look, |ui| {
        ui.set_min_width(300.0);
        let applying = if menus.operation {
            Err("Finish or abort the operation in progress first")
        } else {
            menus.idle()
        };
        if item(
            ui,
            (icon::FILE_ARROW_DOWN, "Apply patches from files…"),
            applying,
        ) {
            *asked = Some(Ask::ApplyFiles);
        }
        if item(
            ui,
            (icon::CLIPBOARD_TEXT, "Paste a patch to apply…"),
            applying,
        ) {
            *asked = Some(Ask::Paste);
        }
        ui.separator();
        let staged = if changes.staged {
            menus.idle()
        } else {
            Err("Nothing is staged")
        };
        let all = if !changes.has_commit {
            Err("There is no commit to compare with yet")
        } else if changes.tracked {
            menus.idle()
        } else {
            Err("No tracked file changed (new files aren't in it until staged)")
        };
        if item(
            ui,
            (icon::COPY, "Copy the staged changes as a patch"),
            staged,
        ) {
            *asked = Some(Ask::Task(Task::CopyChanges(Changes::Staged)));
        }
        if item(ui, (icon::COPY, "Copy every change as a patch"), all) {
            *asked = Some(Ask::Task(Task::CopyChanges(Changes::All)));
        }
        if item(
            ui,
            (icon::FLOPPY_DISK, "Save the staged changes as a patch…"),
            staged,
        ) {
            *asked = Some(Ask::SaveChanges(Changes::Staged));
        }
        if item(
            ui,
            (icon::FLOPPY_DISK, "Save every change as a patch…"),
            all,
        ) {
            *asked = Some(Ask::SaveChanges(Changes::All));
        }
    })
    .0
}

/// The stash menu, under `button`: stashing the work tree's changes, and each stash to apply,
/// pop, or drop.
pub(super) fn stash_menu(
    ui: &mut Ui,
    menus: &Menus<'_>,
    stashes: &[Stash],
    has_changes: bool,
    look: crate::widgets::Look,
    asked: &mut Option<Ask>,
) -> Response {
    let label = stash_label(stashes.len());
    let busy = menus.idle();
    let can_stash = if has_changes {
        busy
    } else {
        Err("There are no changes to stash")
    };
    let now = format::now();
    crate::widgets::tool_menu(ui, crate::theme::icon::ARCHIVE, &label, look, |ui| {
        ui.set_width(STASH_MENU_WIDTH);
        if item(ui, (icon::ARCHIVE, "Stash the changes…"), can_stash) {
            *asked = Some(Ask::Dialog(Dialog::Stash {
                message: String::new(),
                untracked: true,
            }));
        }
        if stashes.is_empty() {
            return;
        }
        ui.separator();
        menu_header(ui, "Stashes, latest first");
        // A row each, touching, in a list that scrolls once it is longer than a screenful:
        // only the rows in view are drawn. Its scroll bar takes room of its own, beside the
        // rows' ages rather than over them.
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.spacing_mut().scroll = egui::style::ScrollStyle::solid();
        egui::ScrollArea::vertical()
            .id_salt("stash-menu-list")
            .max_height(crate::theme::CONTROL * STASHES_SHOWN)
            .auto_shrink([false, true])
            .show_rows(ui, crate::theme::CONTROL, stashes.len(), |ui, rows| {
                for stash in stashes.get(rows).unwrap_or_default() {
                    stash_row(ui, stash, busy, now, asked);
                }
            });
    })
    .0
}

/// The stash menu's button's words: "Stash", or how many there are.
pub(super) fn stash_label(stashes: usize) -> String {
    match stashes {
        0 => "Stash".to_owned(),
        count => format!("Stashes · {count}"),
    }
}

/// The stash menu's width: a stash's message is cut to fit.
const STASH_MENU_WIDTH: f32 = 380.0;

/// A stash's submenu's width: its whole message wraps inside.
const STASH_ACTIONS_WIDTH: f32 = 260.0;

/// How many stashes the stash menu shows at once; the rest scroll.
const STASHES_SHOWN: f32 = 10.0;

/// One stash in the stash menu: its message and how long ago, its submenu offering Apply, Pop,
/// and Drop under its whole message.
fn stash_row(
    ui: &mut Ui,
    stash: &Stash,
    busy: Result<(), &str>,
    now: u64,
    asked: &mut Option<Ask>,
) {
    use egui::AtomExt as _;
    use egui::containers::menu::SubMenuButton;

    let when =
        u64::try_from(stash.time).map_or_else(|_| String::new(), |time| format::ago(time, now));
    let glyph = RichText::new(icon::ARCHIVE).color(Palette::of(ui.ctx()).muted);
    let message = RichText::new(&stash.message).atom_shrink(true);
    let button = egui::Button::new((glyph, message))
        .right_text((muted(ui, &when), SubMenuButton::RIGHT_ARROW))
        .wrap_mode(egui::TextWrapMode::Truncate)
        .min_size(egui::vec2(ui.available_width(), crate::theme::CONTROL));
    let (response, _) = SubMenuButton::from_button(button).ui(ui, |ui| {
        ui.set_width(STASH_ACTIONS_WIDTH);
        ui.label(muted(ui, format!("stash@{{{}}}", stash.index)).small());
        ui.label(&stash.message);
        ui.separator();
        for (label, pop) in [("Apply", false), ("Pop", true)] {
            if item(ui, (icon::TRAY_ARROW_UP, label), busy) {
                *asked = Some(Ask::Task(Task::ApplyStash {
                    stash: stash.clone(),
                    pop,
                }));
            }
        }
        if danger_item(ui, (icon::TRASH, "Drop…"), busy) {
            *asked = Some(Ask::Dialog(Dialog::DropStash(stash.clone())));
        }
    });
    // Named as it reads: a screen reader hears what the row shows.
    let name = format!("{} · {when}", stash.message);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &name));
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
