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
    BisectMark, BisectTerms, Bisection, Changes, ConfigSnapshot, FastForward, ObjectId, PullMode,
    PushOptions, RefKind, Reference, ResetMode, Stash, SwitchTo, patch_file_name,
};
use egui::{Response, RichText, Ui};

use super::dialogs::{Dialog, WorktreeAsked};
use super::history::ID_DIGITS;
use crate::foresight::{Bringing, Foresight, Look};
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
    /// What changes from `HEAD` to this commit, compared in the details' place.
    Compare(ObjectId),
    /// A web page, in the browser.
    Open(String),
    /// A file or folder here, opened in its app or shown in its folder.
    Reveal(std::path::PathBuf, crate::desktop::Reveal),
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
    /// A commit's pipeline, in the commit table's place.
    Pipeline(Box<crate::ci::CiMark>),
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
    /// What bringing commits in would do is foreseen with these (none in a partial clone).
    pub(super) foreseeing: Option<Foreseeing<'a>>,
}

/// What the menus foresee conflicts with.
#[derive(Clone, Copy)]
pub(super) struct Foreseeing<'a> {
    pub(super) foresight: &'a Foresight,
    pub(super) git: &'a egit_git::Git,
    pub(super) repository: &'a std::sync::Arc<egit_git::Repository>,
    /// The commit `HEAD` is at.
    pub(super) head: ObjectId,
}

/// An item's words for bringing commits in, and what is foreseen of it: the bringing, and
/// whether a merge commit is made anyway (no fast-forward), so a fast-forward reads as clean.
type Foreseen = (Bringing, bool);

impl Menus<'_> {
    /// What bringing in would do, as far as it is worked out.
    fn look(&self, ui: &Ui, bringing: Bringing) -> Look {
        self.foreseeing.map_or(Look::Unknown, |seeing| {
            seeing.foresight.look(
                ui.ctx(),
                (seeing.git, seeing.repository),
                seeing.head,
                bringing,
            )
        })
    }

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
    menu_entry(ui, entry, allowed, (false, None), None)
}

/// An [`item`] bringing commits in, what it would do at its right once foreseen ("✓ clean",
/// "⚠ 2 conflicts"; the files on hover).
fn foreseen_item(
    ui: &mut Ui,
    menus: &Menus<'_>,
    entry: (&str, &str),
    allowed: Result<(), &str>,
    (bringing, merge_commit): Foreseen,
) -> bool {
    // Nothing is worked out for what can't be done now.
    let hint = if allowed.is_ok() {
        hint(
            &Palette::of(ui.ctx()),
            &menus.look(ui, bringing),
            merge_commit,
        )
    } else {
        None
    };
    menu_entry(ui, entry, allowed, (false, None), hint)
}

/// What a foreseen item says at its right, and on hover.
fn hint(palette: &Palette, look: &Look, merge_commit: bool) -> Option<(RichText, String)> {
    use egit_git::Foreseen as Seen;
    // A merge commit is made even where the branch could only move forward.
    let clean = Look::Seen(Seen::Clean);
    let look = if merge_commit && matches!(look, Look::Seen(Seen::FastForward)) {
        &clean
    } else {
        look
    };
    let (words, color, hover) = match look {
        Look::Unknown | Look::Seen(Seen::TooLong(_)) => return None,
        Look::Working => (
            "checking…".to_owned(),
            palette.muted,
            "Working out whether it would conflict".to_owned(),
        ),
        Look::Seen(Seen::Clean) => (
            format!("{} clean", icon::CHECK),
            palette.ok,
            "No conflicts (worked out without touching anything)".to_owned(),
        ),
        Look::Seen(Seen::FastForward) => (
            "fast-forward".to_owned(),
            palette.muted,
            "The branch only moves forward: nothing to merge".to_owned(),
        ),
        Look::Seen(Seen::UpToDate) => (
            "up to date".to_owned(),
            palette.muted,
            "Nothing to bring in: it is in already".to_owned(),
        ),
        Look::Seen(Seen::Empty) => (
            "no changes".to_owned(),
            palette.muted,
            "Its changes are in already: it would change nothing".to_owned(),
        ),
        Look::Seen(Seen::Conflicts { files, at }) => (
            if files.len() == 1 {
                format!("{} 1 conflict", icon::WARNING)
            } else {
                format!(
                    "{} {} conflicts",
                    icon::WARNING,
                    format::count(u64::try_from(files.len()).unwrap_or(u64::MAX))
                )
            },
            palette.warn,
            conflicts_hover(files, *at),
        ),
    };
    Some((
        RichText::new(words).size(crate::theme::SMALL).color(color),
        hover,
    ))
}

/// The files a foreseen conflict names (the first ten), and where a rebase would stop.
fn conflicts_hover(files: &[BString], at: Option<ObjectId>) -> String {
    const NAMED: usize = 10;
    let mut named: Vec<String> = files
        .iter()
        .take(NAMED)
        .map(|file| file.to_str_lossy().into_owned())
        .collect();
    if files.len() > NAMED {
        named.push(format!("{} more", files.len().saturating_sub(NAMED)));
    }
    let named = named.join(", ");
    at.map_or_else(
        || format!("Would stop at conflicts in {named}"),
        |commit| {
            format!(
                "Would stop at {} with conflicts in {named}",
                commit.abbreviated(ID_DIGITS)
            )
        },
    )
}

/// An [`item`] explained by the help topic `topic` rather than by its words, where a common
/// word means something of its own ("Rename…" a remote).
pub(crate) fn item_of(
    ui: &mut Ui,
    entry: (&str, &str),
    allowed: Result<(), &str>,
    topic: &'static str,
) -> bool {
    menu_entry(ui, entry, allowed, (false, Some(topic)), None)
}

/// A menu's item whose words want more said: `about` on hover (in the danger colour when
/// `danger`: what it allows can lose work).
pub(crate) fn described_item(
    ui: &mut Ui,
    entry: (&str, &str),
    allowed: Result<(), &str>,
    (danger, about): (bool, &str),
) -> bool {
    menu_entry(
        ui,
        entry,
        allowed,
        (danger, None),
        Some((RichText::new(""), about.to_owned())),
    )
}

/// A menu's item with a keyboard shortcut (written as a Mac writes it), shown at its right.
pub(crate) fn shortcut_item(ui: &mut Ui, (glyph, label): (&str, &str), keys: &'static str) -> bool {
    menu_rows(ui);
    let palette = Palette::of(ui.ctx());
    let button = egui::Button::new((RichText::new(glyph).color(palette.muted), label))
        .shortcut_text(crate::commands::keys_text(keys));
    let response = ui.add(button);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    crate::help::note(ui, response.rect, label);
    if response.clicked() {
        ui.close();
        true
    } else {
        false
    }
}

/// A menu's item for what loses work (deleting, dropping, a hard reset): in the danger colour.
pub(crate) fn danger_item(ui: &mut Ui, entry: (&str, &str), allowed: Result<(), &str>) -> bool {
    menu_entry(ui, entry, allowed, (true, None), None)
}

/// A [`danger_item`] explained by the help topic `topic` rather than by its words.
pub(crate) fn danger_item_of(
    ui: &mut Ui,
    entry: (&str, &str),
    allowed: Result<(), &str>,
    topic: &'static str,
) -> bool {
    menu_entry(ui, entry, allowed, (true, Some(topic)), None)
}

/// Draws an item (in the danger colour, explained by a help topic of its own); returns whether
/// it was clicked.
fn menu_entry(
    ui: &mut Ui,
    (glyph, label): (&str, &str),
    allowed: Result<(), &str>,
    (danger, topic): (bool, Option<&'static str>),
    hint: Option<(RichText, String)>,
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
    let (button, hover) = match hint {
        Some((shown, said)) => (
            egui::Button::new((glyph, words)).shortcut_text(shown),
            Some(said),
        ),
        None => (egui::Button::new((glyph, words)), None),
    };
    let response = ui.add_enabled(allowed.is_ok(), button);
    // Named by its words alone: what is foreseen is said on hover.
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, allowed.is_ok(), label)
    });
    crate::help::note(ui, response.rect, label);
    if let Some(topic) = topic {
        crate::help::note_topic(ui, response.rect, topic);
    }
    let response = match (allowed, hover) {
        (Err(why), _) => response.on_disabled_hover_text(why),
        (Ok(()), Some(said)) => response.on_hover_text(said),
        (Ok(()), None) => response,
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
    crate::help::note(ui, response.rect, label);
}

/// A group's header in a menu: its name in small capitals, muted, over its items, which it
/// lines up with.
pub(crate) fn menu_header(ui: &mut Ui, title: &str) {
    menu_rows(ui);
    let palette = Palette::of(ui.ctx());
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(ui.spacing().button_padding.x);
        let response = ui.add(
            egui::Label::new(
                RichText::new(title.to_uppercase())
                    .size(crate::theme::SMALL)
                    .semibold()
                    .extra_letter_spacing(0.6)
                    .color(palette.muted),
            )
            .selectable(false),
        );
        crate::help::note(ui, response.rect, title);
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
    // Its page on the forge first: what people reach for most outside git.
    if let Some(repository) = menus.forge.repositories.first() {
        let forge = repository.host().kind().name();
        if item(
            ui,
            (icon::ARROW_SQUARE_OUT, &format!("Open on {forge}")),
            Ok(()),
        ) {
            ask(Ask::Open(repository.commit_url(id)));
        }
    }
    compare_item(ui, menus, id, &mut ask);
    ui.separator();
    here_items(ui, menus, id, other, &mut ask);
    ui.separator();
    into_items(ui, menus, id, (other, can), &mut ask);
    rewrite_items(ui, menus, id, &mut ask);
    bulk_menu(ui, menus, id, &mut ask);
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
}

/// "Compare with <the current branch>": what changes from `HEAD` to `target`, in the details'
/// place (nothing to compare where `HEAD` is already there; none before the first commit).
fn compare_item(ui: &mut Ui, menus: &Menus<'_>, target: ObjectId, ask: &mut impl FnMut(Ask)) {
    let Some(head) = menus.head else {
        return;
    };
    let allowed = if head == target {
        Err("It is where HEAD is")
    } else {
        Ok(())
    };
    let words = format!("Compare with {}", menus.current());
    if item(ui, (icon::ARROWS_LEFT_RIGHT, &words), allowed) {
        ask(Ask::Compare(target));
    }
}

/// What of a commit can be copied (a submenu): its name, its subject, a link to it on its
/// forge, and it as a patch.
pub(super) fn copy_menu(
    ui: &mut Ui,
    menus: &Menus<'_>,
    (id, subject): (ObjectId, Option<&str>),
    asked: &mut Option<Ask>,
) {
    submenu(ui, (icon::COPY, "Copy"), |ui| {
        if item(ui, (icon::COPY, "Copy commit name"), Ok(())) {
            ui.ctx().copy_text(id.to_string());
        }
        if let Some(subject) = subject
            && item(ui, (icon::COPY, "Copy subject"), Ok(()))
        {
            ui.ctx().copy_text(subject.to_owned());
        }
        if let Some(repository) = menus.forge.repositories.first() {
            let forge = repository.host().kind().name();
            if item(
                ui,
                (icon::LINK, &format!("Copy link to this commit on {forge}")),
                Ok(()),
            ) {
                ui.ctx().copy_text(repository.commit_url(id));
            }
        }
        if item(ui, (icon::COPY, "Copy as a patch"), menus.idle()) {
            *asked = Some(Ask::Task(Task::CopyPatch(id)));
        }
    });
}

/// The actions picking several commits offers (their menu's), for this one alone, and how to
/// pick more.
fn bulk_menu(ui: &mut Ui, menus: &Menus<'_>, id: ObjectId, ask: &mut impl FnMut(Ask)) {
    submenu(ui, (icon::STACK, "Bulk actions"), |ui| {
        let allowed = super::picked::signing_allowed(menus, 1);
        if item(ui, (icon::SEAL_CHECK, "Sign this commit…"), allowed) {
            ask(Ask::Dialog(Dialog::SignCommits(vec![id])));
        }
        ui.separator();
        let (range, one) = super::picked::clicks();
        ui.add(
            egui::Label::new(muted(
                ui,
                format!("{range} or {one} rows to pick several: their menu does these to all."),
            ))
            .wrap(),
        );
    });
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
    // A branch's tip is merged as the branch (a local one first, never the current one), so
    // git's message says "Merge branch 'topic'" rather than naming the commit.
    let tip = menus
        .refs
        .at(&id)
        .iter()
        .find(|reference| match reference.kind {
            RefKind::Branch => menus.branch != Some(reference.short_name()),
            RefKind::RemoteBranch => true,
            _ => false,
        });
    let (merged, merged_shown) = tip.map_or_else(
        || (what.clone(), shown.clone()),
        |branch| {
            let name = branch.short_name().to_str_lossy().into_owned();
            (branch.name.clone(), name)
        },
    );
    for (label, fast_forward) in [
        (format!("Merge into {current}"), FastForward::AsConfigured),
        (
            format!("Merge into {current} (no fast-forward)"),
            FastForward::Never,
        ),
    ] {
        let foreseen = (Bringing::Merge(id), fast_forward == FastForward::Never);
        if foreseen_item(ui, menus, (icon::GIT_MERGE, &label), other, foreseen) {
            ask(Ask::Task(Task::Merge {
                what: merged.clone(),
                shown: merged_shown.clone(),
                fast_forward,
            }));
        }
    }
    let label = format!("Rebase {current} onto this commit");
    if foreseen_item(
        ui,
        menus,
        (icon::STACK, &label),
        other,
        (Bringing::Rebase(id), false),
    ) {
        ask(Ask::Task(Task::Rebase {
            onto: what.clone(),
            shown: shown.clone(),
            keep_merges: false,
        }));
    }
    let label = format!("Rebase {current} onto this commit, keeping merges");
    if item(ui, (icon::STACK, &label), other) {
        ask(Ask::Task(Task::Rebase {
            onto: what,
            shown,
            keep_merges: true,
        }));
    }
    let label = format!("Cherry-pick onto {current}");
    if foreseen_item(
        ui,
        menus,
        (icon::CHERRIES, &label),
        other,
        (Bringing::Pick(id), false),
    ) {
        ask(Ask::Task(Task::CherryPick(id)));
    }
    let label = format!("Revert in {current}");
    if foreseen_item(
        ui,
        menus,
        (icon::ARROW_COUNTER_CLOCKWISE, &label),
        can,
        (Bringing::Revert(id), false),
    ) {
        ask(Ask::Task(Task::Revert {
            commit: id,
            anyway: false,
        }));
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
            compare_item(ui, menus, reference.target, &mut ask);
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
            compare_item(ui, menus, reference.target, &mut ask);
            ui.separator();
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
    for (glyph, label, task, allowed, foreseen) in drop_offers(menus, (dragged, onto), (&from, &to))
    {
        let allowed = allowed.as_ref().copied().map_err(String::as_str);
        let chosen = match foreseen {
            Some(foreseen) => foreseen_item(ui, menus, (glyph, &label), allowed, foreseen),
            None => item(ui, (glyph, &label), allowed),
        };
        if chosen && let Some(task) = task {
            ask(Ask::Task(task));
        }
    }
}

/// An item a drop offers: its icon, its words, its task (none: only said), whether it can be
/// done now (why not), and what is foreseen of it.
type Offer = (
    &'static str,
    String,
    Option<Task>,
    Result<(), String>,
    Option<Foreseen>,
);

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
    if current(onto) {
        let (merged, rebased) = (
            Bringing::Merge(dragged.target),
            Bringing::Rebase(dragged.target),
        );
        return vec![
            (
                icon::GIT_MERGE,
                format!("Merge {from} into {to}"),
                Some(merge_task(dragged, from, FastForward::AsConfigured)),
                allowed.clone(),
                Some((merged, false)),
            ),
            (
                icon::GIT_MERGE,
                format!("Merge {from} into {to} (no fast-forward)"),
                Some(merge_task(dragged, from, FastForward::Never)),
                allowed.clone(),
                Some((merged, true)),
            ),
            (
                icon::STACK,
                format!("Rebase {to} onto {from}"),
                Some(rebase_task(dragged, from, false)),
                allowed.clone(),
                Some((rebased, false)),
            ),
            (
                icon::FAST_FORWARD,
                format!("Fast-forward {to} to {from}"),
                Some(forward_task(dragged, from)),
                allowed,
                None,
            ),
        ];
    }
    if current(dragged) {
        return current_dropped_offers(onto, (from, to), &allowed);
    }
    elsewhere_offers(menus, (dragged, onto), (from, to))
}

/// What the current branch dropped onto `onto` offers: rebasing onto it (keeping merges or
/// not), merging it in, fast-forwarding to it.
fn current_dropped_offers(
    onto: &Reference,
    (from, to): (&str, &str),
    allowed: &Result<(), String>,
) -> Vec<Offer> {
    let (merged, rebased) = (Bringing::Merge(onto.target), Bringing::Rebase(onto.target));
    vec![
        (
            icon::STACK,
            format!("Rebase {from} onto {to}"),
            Some(rebase_task(onto, to, false)),
            allowed.clone(),
            Some((rebased, false)),
        ),
        (
            icon::STACK,
            format!("Rebase {from} onto {to}, keeping merges"),
            Some(rebase_task(onto, to, true)),
            allowed.clone(),
            None,
        ),
        (
            icon::GIT_MERGE,
            format!("Merge {to} into {from}"),
            Some(merge_task(onto, to, FastForward::AsConfigured)),
            allowed.clone(),
            Some((merged, false)),
        ),
        (
            icon::GIT_MERGE,
            format!("Merge {to} into {from} (no fast-forward)"),
            Some(merge_task(onto, to, FastForward::Never)),
            allowed.clone(),
            Some((merged, true)),
        ),
        (
            icon::FAST_FORWARD,
            format!("Fast-forward {from} to {to}"),
            Some(forward_task(onto, to)),
            allowed.clone(),
            None,
        ),
    ]
}

/// Merging `what` (named `shown`) into the current branch.
fn merge_task(what: &Reference, shown: &str, fast_forward: FastForward) -> Task {
    Task::Merge {
        what: what.name.clone(),
        shown: shown.to_owned(),
        fast_forward,
    }
}

/// Rebasing the current branch onto `onto` (named `shown`).
fn rebase_task(onto: &Reference, shown: &str, keep_merges: bool) -> Task {
    Task::Rebase {
        onto: onto.name.clone(),
        shown: shown.to_owned(),
        keep_merges,
    }
}

/// Fast-forwarding the current branch to `to` (named `shown`).
fn forward_task(to: &Reference, shown: &str) -> Task {
    Task::FastForward {
        to: to.name.clone(),
        shown: shown.to_owned(),
    }
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
            None,
        ));
    }
    offers.push((
        icon::GIT_MERGE,
        format!("Merge {from} into {to}"),
        None,
        Err(format!(
            "git merges into the branch checked out: switch to {to} first"
        )),
        None,
    ));
    if let Some(switch) = menus.switch_to(onto) {
        offers.push((
            icon::ARROW_BEND_UP_RIGHT,
            format!("Switch to {to}"),
            Some(Task::Switch(switch)),
            menus.can_change().map_err(str::to_owned),
            None,
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
    for (label, fast_forward) in [
        (
            format!("Merge {name} into {current}"),
            FastForward::AsConfigured,
        ),
        (
            format!("Merge {name} into {current} (no fast-forward)"),
            FastForward::Never,
        ),
    ] {
        let foreseen = (
            Bringing::Merge(reference.target),
            fast_forward == FastForward::Never,
        );
        if foreseen_item(ui, menus, (icon::GIT_MERGE, &label), other, foreseen) {
            ask(Ask::Task(Task::Merge {
                what: reference.name.clone(),
                shown: name.clone().into_owned(),
                fast_forward,
            }));
        }
    }
    for (label, keep_merges) in [
        (format!("Rebase {current} onto {name}"), false),
        (
            format!("Rebase {current} onto {name}, keeping merges"),
            true,
        ),
    ] {
        // A rebase keeping merges replays them too: not foreseen.
        let chosen = if keep_merges {
            item(ui, (icon::STACK, &label), other)
        } else {
            let foreseen = (Bringing::Rebase(reference.target), false);
            foreseen_item(ui, menus, (icon::STACK, &label), other, foreseen)
        };
        if chosen {
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

/// A commit's patches: saved, or saved with the commits after it up to `HEAD` (copied, in
/// [`copy_menu`]).
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
            *asked = Some(Ask::Dialog(Dialog::stash(Vec::new())));
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
        for (glyph, label, pop) in [
            (icon::TRAY_ARROW_UP, "Apply", false),
            (icon::ARROW_LINE_UP, "Pop", true),
        ] {
            if item(ui, (glyph, label), busy) {
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
    crate::help::note_topic(ui, response.rect, "stash");
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
