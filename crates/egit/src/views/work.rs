//! The working tree's changes beside the history: what isn't staged and what is, each list with
//! a button to move all of it and each file with its own, a file opened in the file view; and
//! the form that commits what is staged, with whoever else wrote it as co-authors.
//!
//! The lists are made once per status read (indices into it, so a tree with a hundred thousand
//! untracked files costs a few hundred kilobytes), and drawn by egui-table-kit, only the rows on
//! screen.

use std::borrow::Cow;
use std::sync::Arc;

use bstr::{BStr, BString, ByteSlice as _};
use egit_git::{
    Author, CO_AUTHORED_BY, CancelToken, Change, Committed, Direction, EntryKind, Git, NewCommit,
    ObjectId, Person, Repository, Status, StatusEntry, Uncommitted,
};
use egui::{Align, Frame, Key, Label, Layout, Margin, Modifiers, Response, RichText, Ui};
use egui_async::Bind;
use egui_table_kit::error::TableError;
use egui_table_kit::interaction::RowEventKind;
use egui_table_kit::operations::{HeaderIter, Row, RowCallback, TableCell, TableProvider};
use egui_table_kit::state::TableState;
use egui_table_kit::table::TableKit;

use super::file::WorkFile;
use crate::discarding::Discard;
use crate::format::count;
use crate::history::{Objects, read_message};
use crate::preferences::Preferences;
use crate::problem::Problem;
use crate::repo::RepoId;
use crate::staging::StageRequest;
use crate::tables::elided_start;
use crate::theme::{Palette, icon};
use crate::tree_table::{TreeMenu, TreeRow, TreeSource, TreeTable};
use crate::widgets::{self, muted};

/// What the user asked of the panel.
#[derive(Debug)]
pub(super) enum WorkEvent {
    /// Look at a file's changes.
    Open(WorkFile),
    /// Stage or unstage.
    Stage(StageRequest),
    /// Commit what is staged.
    Commit(NewCommit),
    /// Stop the commit under way (its hooks with it).
    CancelCommit,
    /// Show the output panel (what the hooks printed).
    ShowOutput,
    /// Show lists of files so (a list, or a tree).
    Preferences(Preferences),
    /// Ask whether to discard whole files' changes not staged.
    AskDiscard(Discard),
}

/// What the commit form needs of the repository.
pub(super) struct CommitSide<'a> {
    /// The last commit made, or being made.
    pub(super) committing: &'a mut Bind<Committed, Problem>,
    /// How many commits egit has made here (the form clears when it grows).
    pub(super) commits_made: u64,
    /// Whether commits are signed unless asked otherwise (`commit.gpgSign`).
    pub(super) sign_by_default: bool,
    /// `HEAD`'s commit, to amend (none before the first commit).
    pub(super) head: Option<ObjectId>,
    pub(super) git: Option<&'a Git>,
    pub(super) repository: &'a Arc<Repository>,
    pub(super) objects: &'a Objects,
    /// The user's email here (`user.email`), not offered as their own co-author.
    pub(super) me: Option<&'a BStr>,
    /// The commit under way's run, while it runs.
    pub(super) run: Option<&'a crate::output::Run>,
    /// The hooks the repository has.
    pub(super) hooks: Option<&'a egit_git::Hooks>,
}

/// Up to this many recent commits' authors are offered as co-authors.
const AUTHORS_READ: usize = 5_000;

/// At most this many offered at once.
const OFFERED: usize = 8;

/// The commit form's state.
#[derive(Debug)]
struct CommitForm {
    summary: String,
    description: String,
    amend: bool,
    sign_off: bool,
    /// Sign or not, once the user chose; until then, the configuration's default.
    sign: Option<bool>,
    /// How many commits had been made when the form was last cleared.
    made_seen: u64,
    /// The last commit's message, read to amend it.
    amended: Bind<String, Problem>,
    /// Who else wrote it: a `Co-authored-by` trailer each.
    co_authors: Vec<Person>,
    /// What is typed in the co-author picker.
    query: String,
    /// Who wrote the recent commits, offered there (read when it first opens).
    authors: Bind<Vec<Author>, Problem>,
    /// Skip the `pre-commit` and `commit-msg` hooks (`--no-verify`), this commit only.
    skip_hooks: bool,
}

impl CommitForm {
    const fn new() -> Self {
        Self {
            summary: String::new(),
            description: String::new(),
            amend: false,
            sign_off: false,
            sign: None,
            made_seen: 0,
            amended: Bind::new(true),
            co_authors: Vec::new(),
            query: String::new(),
            authors: Bind::new(true),
            skip_hooks: false,
        }
    }

    /// Each co-author's trailer, for `git commit --trailer`.
    fn trailers(&self) -> Vec<String> {
        self.co_authors
            .iter()
            .map(|person| {
                format!(
                    "{CO_AUTHORED_BY}: {} <{}>",
                    person.name.to_str_lossy(),
                    person.email.to_str_lossy()
                )
            })
            .collect()
    }

    /// Adds someone, unless they're there already (by email, in any case).
    fn add_co_author(&mut self, person: Person) {
        if !self
            .co_authors
            .iter()
            .any(|known| known.email.eq_ignore_ascii_case(&person.email))
        {
            self.co_authors.push(person);
        }
    }

    /// The message: the summary, and the description after a blank line.
    fn message(&self) -> String {
        let mut message = self.summary.trim().to_owned();
        let description = self.description.trim_end();
        if !description.trim().is_empty() {
            message.push_str("\n\n");
            message.push_str(description);
        }
        message
    }

    fn is_blank(&self) -> bool {
        self.summary.trim().is_empty() && self.description.trim().is_empty()
    }
}

/// The panel's state between frames.
#[derive(Debug)]
pub(super) struct WorkPanel {
    unstaged: List,
    staged: List,
    /// The status the lists were made from.
    made_from: Option<Arc<Status>>,
    form: CommitForm,
}

/// One of the lists: its files, its table's state, and the same files as a tree; and the menu
/// of a row right-clicked.
#[derive(Debug)]
struct List {
    files: Arc<ListFiles>,
    state: TableState,
    tree: TreeTable<ListFiles>,
    menu: Option<RowMenu>,
    /// Whether the menu opens this frame.
    open_menu: bool,
}

/// A row's menu: the list's files when it opened (a status read since doesn't change what it
/// names), and the file or folder right-clicked.
#[derive(Debug)]
struct RowMenu {
    files: Arc<ListFiles>,
    target: TreeMenu,
}

/// A list's files: entries of a status (indices into it), shared with the job arranging them
/// in a tree.
#[derive(Debug)]
struct ListFiles {
    status: Arc<Status>,
    entries: Vec<u32>,
    side: Side,
}

impl ListFiles {
    fn entry(&self, row: usize) -> Option<StatusEntry<'_>> {
        let index = usize::try_from(*self.entries.get(row)?).ok()?;
        self.status.entry(index)
    }
}

impl TreeSource for ListFiles {
    fn files(&self) -> usize {
        self.entries.len()
    }

    fn path(&self, entry: usize) -> &[u8] {
        self.entry(entry)
            .map_or(&[][..], |entry| entry.path.as_bytes())
    }

    fn change(&self, entry: usize) -> &'static str {
        self.entry(entry).map_or("Changed", |entry| {
            change_name(entry, self.side.which(entry))
        })
    }
}

impl List {
    fn new(id: String, side: Side) -> Self {
        Self {
            files: Arc::new(ListFiles {
                status: Arc::default(),
                entries: Vec::new(),
                side,
            }),
            tree: TreeTable::new(format!("{id}-tree")),
            state: TableState::new(id, 0),
            menu: None,
            open_menu: false,
        }
    }

    /// Takes `entries` of `status`, keeping the table's place (its scrolling is egui's, by its
    /// id).
    fn set(&mut self, status: &Arc<Status>, entries: Vec<u32>) {
        self.state.active_rows = (0..entries.len()).collect();
        self.state.selected_rows.clear();
        self.files = Arc::new(ListFiles {
            status: Arc::clone(status),
            entries,
            side: self.files.side,
        });
    }
}

impl WorkPanel {
    pub(super) fn new(repo: RepoId) -> Self {
        Self {
            unstaged: List::new(format!("work-unstaged-{repo}"), Side::Unstaged),
            staged: List::new(format!("work-staged-{repo}"), Side::Staged),
            made_from: None,
            form: CommitForm::new(),
        }
    }

    /// Draws the lists for `status` (with `staging`'s problem if the last one failed) and the
    /// commit form; returns what the user asked for.
    pub(super) fn show(
        &mut self,
        ui: &mut Ui,
        status: &Arc<Status>,
        staging: &mut Bind<(), Problem>,
        commit: CommitSide<'_>,
        task: bool,
    ) -> Option<WorkEvent> {
        if !self
            .made_from
            .as_ref()
            .is_some_and(|made| Arc::ptr_eq(made, status))
        {
            let (mut unstaged, mut staged) = (Vec::new(), Vec::new());
            for (index, entry) in (0_u32..).zip(status.entries()) {
                if entry.is_unstaged() {
                    unstaged.push(index);
                }
                if entry.is_staged() {
                    staged.push(index);
                }
            }
            self.unstaged.set(status, unstaged);
            self.staged.set(status, staged);
            self.made_from = Some(Arc::clone(status));
        }
        let mut event = None;
        if let Some(problem) = staging.err_ref() {
            let problem = problem.clone();
            widgets::problem_box(ui, &problem, |_| {});
            ui.add_space(8.0);
        }
        let busy = staging.is_pending() || commit.committing.is_pending();
        // The form below keeps the room it needs; the lists share the rest.
        let height = (ui.available_height() - 380.0).max(160.0) / 2.0;
        for (list, side) in [
            (&mut self.unstaged, Side::Unstaged),
            (&mut self.staged, Side::Staged),
        ] {
            // Each table's scroll area is named by where it is drawn: apart, then.
            let found = ui
                .push_id(side, |ui| list_ui(ui, list, side, (busy, task), height))
                .inner;
            if found.is_some() {
                event = found;
            }
            ui.add_space(12.0);
        }
        let staged = self.staged.files.entries.len();
        let mut commit = commit;
        if let Some(asked) = commit_form(ui, &mut self.form, &mut commit, staged, busy) {
            event = Some(asked);
        }
        event
    }
}

/// The form committing what is staged: a summary, a description, amending, signing off,
/// signing; then what the last commit printed, or why it wasn't made.
fn commit_form(
    ui: &mut Ui,
    form: &mut CommitForm,
    side: &mut CommitSide<'_>,
    staged: usize,
    busy: bool,
) -> Option<WorkEvent> {
    if side.commits_made != form.made_seen {
        *form = CommitForm {
            made_seen: side.commits_made,
            ..CommitForm::new()
        };
    }
    // Amending a blank form starts from the last commit's message.
    if form.amend
        && form.is_blank()
        && let Some(Ok(message)) = form.amended.read_as_ref()
    {
        let message = message.trim_end().to_owned();
        let (summary, description) = message.split_once("\n\n").unwrap_or((&message, ""));
        summary.clone_into(&mut form.summary);
        form.description = take_co_authors(description, form);
    }
    widgets::section_header(ui, ("Commit", None, "Commit"), |_| {});
    let description = message_fields(ui, form);
    co_authors_row(ui, form, side);
    options(ui, form, side);
    hooks_row(ui, form, side);
    let ready = !busy && !form.summary.trim().is_empty() && (staged > 0 || form.amend);
    let label = if form.amend {
        "Amend the last commit".to_owned()
    } else if staged == 1 {
        "Commit 1 file".to_owned()
    } else {
        format!(
            "Commit {} files",
            count(u64::try_from(staged).unwrap_or(u64::MAX))
        )
    };
    let shortcut = description.has_focus()
        && ui.input_mut(|input| input.consume_key(Modifiers::COMMAND, Key::Enter));
    let mut asked = None;
    ui.horizontal(|ui| {
        if widgets::primary(ui, ready, &label).clicked() || (shortcut && ready) {
            asked = Some(WorkEvent::Commit(NewCommit {
                message: form.message(),
                amend: form.amend,
                sign_off: form.sign_off,
                sign: Some(form.sign.unwrap_or(side.sign_by_default)),
                trailers: form.trailers(),
                no_verify: form.skip_hooks,
            }));
        }
    });
    if side.committing.is_pending()
        && let Some(running) = committing(ui, side)
    {
        asked = Some(running);
    }
    if let Some(outcome) = outcome(ui, side.committing) {
        asked = Some(outcome);
    }
    asked
}

/// The hooks a commit runs, when the repository has `pre-commit` or `commit-msg`: which, and
/// skipping them (`--no-verify`) for this commit.
fn hooks_row(ui: &mut Ui, form: &mut CommitForm, side: &CommitSide<'_>) {
    let Some(hooks) = side.hooks else {
        return;
    };
    let checks = hooks.of(&["pre-commit", "commit-msg"]);
    if checks.is_empty() {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        widgets::checkbox(ui, &mut form.skip_hooks, "Skip hooks")
            .on_hover_text("git commit --no-verify: this commit only");
        let runs = if form.skip_hooks {
            "they won't run".to_owned()
        } else {
            format!("{} run first", checks.join(" and "))
        };
        ui.label(muted(ui, runs));
    });
}

/// The commit under way: the hooks running and for how long, the output, and Cancel.
fn committing(ui: &mut Ui, side: &CommitSide<'_>) -> Option<WorkEvent> {
    let mut asked = None;
    ui.horizontal_wrapped(|ui| {
        ui.spinner();
        let doing = match side.run {
            Some(run) if !run.hooks.is_empty() => {
                format!("Committing: running {}", run.hooks.join(", "))
            }
            _ => "Committing…".to_owned(),
        };
        ui.label(muted(ui, doing));
        if let Some(run) = side.run {
            // The seconds count on their own, in a place of their own width.
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(500));
            let palette = Palette::of(ui.ctx());
            widgets::seconds(ui, run.elapsed(std::time::Instant::now()), palette.muted);
        }
        if widgets::link(ui, "Show the output").clicked() {
            asked = Some(WorkEvent::ShowOutput);
        }
        if widgets::secondary(ui, true, "Cancel").clicked() {
            asked = Some(WorkEvent::CancelCommit);
        }
    });
    asked
}

/// The summary and the description; returns the description's field (⌘Enter there commits).
fn message_fields(ui: &mut Ui, form: &mut CommitForm) -> Response {
    let summary_label = ui.label(muted(ui, "Summary"));
    crate::help::note_topic(ui, summary_label.rect, "commit-summary");
    ui.add(
        crate::widgets::line_field(&mut form.summary)
            .hint_text("What it does, in a line")
            .desired_width(f32::INFINITY),
    )
    .labelled_by(summary_label.id);
    let length = form.summary.trim().chars().count();
    if length > 72 {
        ui.label(
            RichText::new(format!(
                "{length} characters: a summary over 72 is cut short in many places"
            ))
            .small()
            .color(Palette::of(ui.ctx()).warn),
        );
    }
    let description_label = ui.label(muted(ui, "Description"));
    crate::help::note_topic(ui, description_label.rect, "commit-description");
    crate::widgets::bounded_text_area(
        ui,
        crate::widgets::text_area(&mut form.description)
            .hint_text("Why, and anything else worth knowing")
            .desired_rows(4)
            .desired_width(f32::INFINITY),
        DESCRIPTION_HEIGHT,
        "commit-description",
    )
    .labelled_by(description_label.id)
}

/// The tallest the commit form's description gets before it scrolls: the form keeps its room.
const DESCRIPTION_HEIGHT: f32 = 180.0;

/// `description` without its `Co-authored-by` lines, whom `form` takes as its co-authors (git
/// adds them back, as trailers, when the commit is amended).
fn take_co_authors(description: &str, form: &mut CommitForm) -> String {
    let mut kept = String::with_capacity(description.len());
    for line in description.lines() {
        let person = line.split_once(':').and_then(|(key, value)| {
            key.trim()
                .eq_ignore_ascii_case(CO_AUTHORED_BY)
                .then(|| Person::parse(value.as_bytes()))
                .flatten()
        });
        if let Some(person) = person {
            form.add_co_author(person);
        } else {
            kept.push_str(line);
            kept.push('\n');
        }
    }
    kept.trim_end().to_owned()
}

/// The co-authors: each a chip with its avatar and a button removing it, then "Co-author"
/// opening the picker.
fn co_authors_row(ui: &mut Ui, form: &mut CommitForm, side: &CommitSide<'_>) {
    let palette = Palette::of(ui.ctx());
    ui.horizontal_wrapped(|ui| {
        let mut removed = None;
        for (at, person) in form.co_authors.iter().enumerate() {
            let (name, email) = (person.name.to_str_lossy(), person.email.to_str_lossy());
            ui.push_id(("co-author", at), |ui| {
                // As tall as a control: a target's height and 2 points around it.
                Frame::new()
                    .fill(palette.surface_alt)
                    .corner_radius(crate::theme::RADIUS_CONTROL)
                    .inner_margin(Margin {
                        left: 5,
                        right: 2,
                        top: 2,
                        bottom: 2,
                    })
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            widgets::avatar(ui, 18.0, (&name, &email), palette.surface_alt);
                            ui.add(Label::new(&*name).selectable(false))
                                .on_hover_text(&*email);
                            let (rect, _) = ui.allocate_exact_size(
                                egui::vec2(widgets::TARGET, widgets::TARGET),
                                egui::Sense::hover(),
                            );
                            let remove = format!("Remove {name} as a co-author");
                            let id = ui.id().with("remove");
                            if widgets::icon_button(ui, rect, id, icon::X, &remove, true).clicked()
                            {
                                removed = Some(at);
                            }
                        });
                    });
            });
        }
        if let Some(at) = removed {
            form.co_authors.remove(at);
        }
        co_author_picker(ui, form, side);
    });
}

/// "Co-author": a menu with a field, the recent authors matching what is typed, and whoever
/// it names when it is `Name <email>`.
fn co_author_picker(ui: &mut Ui, form: &mut CommitForm, side: &CommitSide<'_>) {
    let button = egui::Button::new(format!("{}  Co-author", icon::USER_PLUS))
        .min_size(egui::vec2(0.0, crate::theme::CONTROL));
    let config = egui::containers::menu::MenuConfig::new()
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
    let (response, _) = egui::containers::menu::MenuButton::from_button(button)
        .config(config)
        .ui(ui, |ui| {
            ui.set_width(360.0);
            if form.authors.is_idle()
                && let Some(git) = side.git
            {
                form.authors
                    .request(read_authors(git.clone(), Arc::clone(side.repository)));
            }
            let label = ui.label(muted(ui, "Who wrote it with you"));
            let field = ui
                .add(
                    widgets::line_field(&mut form.query)
                        .hint_text("Name, email, or Name <email>")
                        .desired_width(f32::INFINITY),
                )
                .labelled_by(label.id);
            // Typed into at once, as a dialog's field is.
            if ui.memory(|memory| memory.focused().is_none()) {
                field.request_focus();
            }
            let picked = offers(
                ui,
                (&mut form.authors, &form.co_authors, &form.query),
                side.me,
            );
            if let Some(person) = picked {
                form.add_co_author(person);
                form.query.clear();
                ui.close();
            }
        });
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Add a co-author")
    });
    crate::help::note(ui, response.rect, "Add a co-author");
    response.on_hover_text(
        "Credit someone else who wrote this commit: a Co-authored-by line at the message's end",
    );
}

/// The authors offered for what is typed, and the person typed; returns the one picked.
fn offers(
    ui: &mut Ui,
    (authors, chosen, query): (&mut Bind<Vec<Author>, Problem>, &[Person], &str),
    me: Option<&BStr>,
) -> Option<Person> {
    let palette = Palette::of(ui.ctx());
    let query = query.trim();
    let typed = Person::parse(query.as_bytes());
    let mut picked = None;
    let mut add = |ui: &mut Ui, person: &Person, detail: &str| {
        let (name, email) = (person.name.to_str_lossy(), person.email.to_str_lossy());
        let text = format!("{name}  ·  {email}{detail}");
        let row = ui.horizontal(|ui| {
            widgets::avatar(ui, 20.0, (&name, &email), palette.surface);
            ui.add(
                egui::Button::new(text)
                    .frame_when_inactive(false)
                    .truncate()
                    .min_size(egui::vec2(ui.available_width(), crate::theme::CONTROL)),
            )
        });
        let named = format!("{name} <{email}>");
        row.inner
            .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &named));
        crate::help::note_topic(ui, row.inner.rect, "co-authors");
        if row.inner.clicked() {
            picked = Some(person.clone());
        }
    };
    if let Some(person) = &typed {
        add(ui, person, "");
    }
    match authors.read_as_ref() {
        Some(Ok(authors)) => {
            let folded = query.to_lowercase();
            let mut shown = 0_usize;
            for author in authors {
                let person = &author.person;
                let taken = chosen
                    .iter()
                    .chain(&typed)
                    .any(|known| known.email.eq_ignore_ascii_case(&person.email))
                    || me.is_some_and(|me| me.eq_ignore_ascii_case(&person.email));
                let matches = folded.is_empty()
                    || person.name.to_str_lossy().to_lowercase().contains(&folded)
                    || person.email.to_str_lossy().to_lowercase().contains(&folded);
                if taken || !matches {
                    continue;
                }
                let commits = if author.commits == 1 {
                    "  ·  1 commit".to_owned()
                } else {
                    format!("  ·  {} commits", count(u64::from(author.commits)))
                };
                add(ui, person, &commits);
                shown = shown.saturating_add(1);
                if shown == OFFERED {
                    break;
                }
            }
            if shown == 0 && typed.is_none() {
                ui.label(muted(
                    ui,
                    "Nobody matches: type Name <email> to add someone new",
                ));
            }
        }
        Some(Err(problem)) => {
            ui.label(muted(ui, problem.to_string()));
        }
        None => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(muted(ui, "Reading who wrote the recent commits…"));
            });
        }
    }
    picked
}

/// Who wrote the recent commits, most commits first.
async fn read_authors(git: Git, repository: Arc<Repository>) -> Result<Vec<Author>, Problem> {
    crate::jobs::blocking(move || {
        egit_git::recent_authors(&git, &repository, AUTHORS_READ, &CancelToken::new())
            .map_err(|error| Problem::git("The recent authors couldn't be read", &error))
    })
    .await
}

/// Amending (which reads the last commit's message), signing off, signing.
fn options(ui: &mut Ui, form: &mut CommitForm, side: &CommitSide<'_>) {
    ui.horizontal_wrapped(|ui| {
        let amendable = side.head.is_some();
        if ui
            .add_enabled_ui(amendable, |ui| {
                widgets::checkbox(ui, &mut form.amend, "Amend")
            })
            .inner
            .on_hover_text("Replace the last commit: its message, with what is staged")
            .changed()
            && form.amend
            && let (Some(git), Some(head)) = (side.git, side.head)
        {
            form.amended.request(read_message(
                git.clone(),
                Arc::clone(side.repository),
                side.objects.clone(),
                head,
            ));
        }
        crate::widgets::checkbox(ui, &mut form.sign_off, "Sign off").on_hover_text(
            "Add “Signed-off-by:” with your name and email at the message's end: a statement \
             that you may submit the change, as some projects ask (a Developer Certificate of \
             Origin), not a signature",
        );
        let mut sign = form.sign.unwrap_or(side.sign_by_default);
        if crate::widgets::checkbox(ui, &mut sign, "Sign")
            .on_hover_text("With the key and program your git configuration names")
            .changed()
        {
            form.sign = Some(sign);
        }
    });
}

/// What the last commit printed (its hooks' output), or why it wasn't made (and the way to
/// what its hooks printed).
fn outcome(ui: &mut Ui, committing: &mut Bind<Committed, Problem>) -> Option<WorkEvent> {
    let mut asked = None;
    let pending = committing.is_pending();
    match committing.read_as_ref() {
        Some(Ok(committed)) if !committed.output.is_empty() && !pending => {
            let output = committed.output.clone();
            let said = ui.add(Label::new(muted(ui, output).small()).wrap());
            crate::help::note_topic(ui, said.rect, "commit-hooks");
        }
        Some(Err(problem)) if !pending => {
            let problem = problem.clone();
            widgets::problem_box(ui, &problem, |ui| {
                if widgets::secondary(ui, true, "Show the output").clicked() {
                    asked = Some(WorkEvent::ShowOutput);
                }
            });
        }
        _ => {}
    }
    asked
}

/// Which list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Side {
    Unstaged,
    Staged,
}

impl Side {
    const fn direction(self) -> Direction {
        match self {
            Self::Unstaged => Direction::Stage,
            Self::Staged => Direction::Unstage,
        }
    }

    /// What its buttons do, in a word.
    const fn verb(self) -> &'static str {
        match self {
            Self::Unstaged => "Stage",
            Self::Staged => "Unstage",
        }
    }

    /// Which changes of `entry` the list shows.
    fn which(self, entry: StatusEntry<'_>) -> Uncommitted {
        match self {
            Self::Staged => Uncommitted::Staged,
            Self::Unstaged if entry.kind == EntryKind::Untracked => Uncommitted::Untracked,
            Self::Unstaged => Uncommitted::Unstaged,
        }
    }
}

/// How `entry` changed on the side `which` shows, in a word or two.
pub(super) fn change_name(entry: StatusEntry<'_>, which: Uncommitted) -> &'static str {
    match entry.kind {
        EntryKind::Untracked => return "Untracked",
        EntryKind::Unmerged => return "Conflict",
        EntryKind::Changed | EntryKind::Renamed | EntryKind::Copied => {}
    }
    let change = match which {
        Uncommitted::Staged => entry.staged(),
        Uncommitted::Unstaged | Uncommitted::Untracked => entry.unstaged(),
    };
    match change {
        Change::Unmodified => "Unchanged",
        Change::Modified => "Modified",
        Change::TypeChanged => "Type changed",
        Change::Added => "Added",
        Change::Deleted => "Deleted",
        Change::Renamed => "Renamed",
        Change::Copied => "Copied",
        Change::Unmerged => "Conflict",
    }
}

/// The paths staging or unstaging `entry` on `side` names (a staged rename's both).
fn paths(entry: StatusEntry<'_>, side: Side) -> Vec<BString> {
    let from = (side == Side::Staged).then_some(entry.from).flatten();
    from.into_iter()
        .chain(std::iter::once(entry.path))
        .map(ToOwned::to_owned)
        .collect()
}

/// One list: its heading, its button for all (and, on the first, the choice of a list or a
/// tree), and its files as either, a row's menu on a right click; `busy` while staging or
/// committing, `task` while a task runs (discarding is one).
fn list_ui(
    ui: &mut Ui,
    list: &mut List,
    side: Side,
    (busy, task): (bool, bool),
    height: f32,
) -> Option<WorkEvent> {
    let mut event = None;
    let (title, all) = match side {
        Side::Unstaged => ("Not staged", "Stage all"),
        Side::Staged => ("Staged", "Unstage all"),
    };
    let files = Arc::clone(&list.files);
    let entries = files.entries.len();
    widgets::section_header(ui, (title, Some(entries), title), |ui| {
        if widgets::secondary(ui, !busy && entries > 0, all).clicked() {
            let paths = (0..entries)
                .filter_map(|row| files.entry(row))
                .flat_map(|entry| paths(entry, side))
                .collect();
            event = Some(WorkEvent::Stage(StageRequest::Files {
                paths,
                direction: side.direction(),
            }));
        }
        if side == Side::Unstaged
            && let Some(preferences) = widgets::tree_toggle(ui)
        {
            event = Some(WorkEvent::Preferences(preferences));
        }
    });
    if entries == 0 {
        // A row of the list's own size, its words where a row's would start.
        widgets::list_box(ui, |ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), crate::tables::ROW_HEIGHT),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.add_space(crate::tables::CELL_INSET);
                    ui.label(muted(
                        ui,
                        match side {
                            Side::Unstaged => "Nothing to stage.",
                            Side::Staged => "Nothing staged yet.",
                        },
                    ));
                },
            );
        });
        return event;
    }
    let asked = if Preferences::of(ui.ctx()).files.lists.is_tree() {
        tree_ui(ui, list, &files, busy, height)
    } else {
        table_ui(ui, list, &files, busy, height)
    };
    let chosen = row_menu(ui, list, side, (busy, task));
    chosen.or(asked).or(event)
}

/// A row's menu, while one is open: moving the file (or a folder's files) across, discarding
/// it, looking at it, copying its path; and discarding every change not staged.
fn row_menu(ui: &Ui, list: &mut List, side: Side, busy: (bool, bool)) -> Option<WorkEvent> {
    let id = ui.id().with("row-menu");
    let menu = list.menu.as_ref()?;
    let open = std::mem::take(&mut list.open_menu);
    let mut event = None;
    let shown = egui::Popup::new(
        id,
        ui.ctx().clone(),
        egui::PopupAnchor::PointerFixed,
        ui.layer_id(),
    )
    .kind(egui::PopupKind::Menu)
    .layout(Layout::top_down_justified(Align::Min))
    .style(egui::containers::menu::menu_style)
    .open_memory(open.then_some(egui::SetOpenCommand::Bool(true)))
    .show(|ui| {
        ui.set_min_width(220.0);
        event = row_menu_items(ui, menu, side, busy);
    });
    if shown.is_none() {
        list.menu = None;
    }
    event
}

/// A row's menu's items; what is chosen, its paths gathered only then (a folder may hold
/// thousands).
fn row_menu_items(
    ui: &mut Ui,
    menu: &RowMenu,
    side: Side,
    (busy, task): (bool, bool),
) -> Option<WorkEvent> {
    use super::actions::{danger_item, item};
    let files = &menu.files;
    let (entries, path, folder) = match &menu.target {
        TreeMenu::File(entry) => {
            let path = files.entry(*entry)?.path.to_owned();
            (std::slice::from_ref(entry), path, false)
        }
        TreeMenu::Folder { path, entries } => {
            (entries.as_slice(), BString::from(path.clone()), true)
        }
    };
    let each = || entries.iter().filter_map(|entry| files.entry(*entry));
    let staging = if busy {
        Err("Wait for what is under way")
    } else {
        Ok(())
    };
    let (glyph, words) = match (side, folder) {
        (Side::Unstaged, false) => (icon::PLUS, "Stage"),
        (Side::Staged, false) => (icon::MINUS, "Unstage"),
        (Side::Unstaged, true) => (icon::PLUS, "Stage the folder"),
        (Side::Staged, true) => (icon::MINUS, "Unstage the folder"),
    };
    let mut event = item(ui, (glyph, words), staging).then(|| {
        WorkEvent::Stage(StageRequest::Files {
            paths: each().flat_map(|entry| paths(entry, side)).collect(),
            direction: side.direction(),
        })
    });
    if side == Side::Unstaged {
        let refused = if folder {
            None
        } else {
            each().next().and_then(crate::discarding::refusal)
        };
        let allowed = match (task, refused) {
            (true, _) => Err("Another task is running"),
            (false, Some(why)) => Err(why),
            (false, None) => staging,
        };
        let words = if folder {
            "Discard the folder's changes…"
        } else {
            "Discard changes…"
        };
        if danger_item(ui, (icon::TRASH, words), allowed) {
            event = crate::discarding::of_entries(each()).map(WorkEvent::AskDiscard);
        }
    }
    ui.separator();
    if !folder
        && item(ui, (icon::FILE_TEXT, "Show changes"), Ok(()))
        && let Some(entry) = each().next()
    {
        event = Some(WorkEvent::Open(WorkFile::of(entry, side.which(entry))));
    }
    if item(ui, (icon::COPY, "Copy path"), Ok(())) {
        ui.ctx().copy_text(path.to_str_lossy().into_owned());
    }
    if side == Side::Unstaged {
        ui.separator();
        let allowed = if task {
            Err("Another task is running")
        } else {
            staging
        };
        if danger_item(ui, (icon::TRASH, "Discard all…"), allowed) {
            let all = (0..files.entries.len()).filter_map(|row| files.entry(row));
            event = crate::discarding::of_entries(all).map(WorkEvent::AskDiscard);
        }
    }
    event
}

/// A list's files as a tree of their folders, a button on each file and folder.
fn tree_ui(
    ui: &mut Ui,
    list: &mut List,
    files: &Arc<ListFiles>,
    busy: bool,
    height: f32,
) -> Option<WorkEvent> {
    let side = files.side;
    let shown = widgets::list_box(ui, |ui| {
        list.tree.show(ui, files, height, |ui, files, row, _| {
            let paths: Vec<BString> = match row {
                TreeRow::File(row) => {
                    let entry = files.entry(row)?;
                    let name = format!("{} {}", side.verb(), entry.path.to_str_lossy());
                    if !stage_button(ui, (&name, side), entry.path.as_bytes(), busy) {
                        return None;
                    }
                    paths(entry, side)
                }
                TreeRow::Folder(tree, node) => {
                    let folder = tree.node(node)?;
                    let path = files.path(usize::try_from(folder.entry).ok()?);
                    let folder_path = folder.folder_path(path);
                    let name = format!("{} the folder {}", side.verb(), folder_path.to_str_lossy());
                    if !stage_button(ui, (&name, side), folder_path, busy) {
                        return None;
                    }
                    tree.files_in(node)
                        .filter_map(|row| files.entry(usize::try_from(row).ok()?))
                        .flat_map(|entry| paths(entry, side))
                        .collect()
                }
            };
            Some(WorkEvent::Stage(StageRequest::Files {
                paths,
                direction: side.direction(),
            }))
        })
    });
    if let Some(target) = shown.menu {
        list.menu = Some(RowMenu {
            files: Arc::clone(files),
            target,
        });
        list.open_menu = true;
    }
    if let Some(row) = shown.opened
        && let Some(entry) = files.entry(row)
    {
        return Some(WorkEvent::Open(WorkFile::of(entry, side.which(entry))));
    }
    shown.asked
}

/// A list's files as a table, a button on each.
fn table_ui(
    ui: &mut Ui,
    list: &mut List,
    files: &Arc<ListFiles>,
    busy: bool,
    height: f32,
) -> Option<WorkEvent> {
    let side = files.side;
    let mut event = None;
    let provider = Entries {
        status: &files.status,
        indices: &files.entries,
        side,
    };
    let palette = Palette::of(ui.ctx());
    let mut button_event = None;
    let table_id = list.state.id.to_string();
    let table = TableKit::new(table_id.as_str(), &provider, &mut list.state);
    let shown = widgets::list_box(ui, |ui| {
        crate::tables::list(table, ui)
            .with_max_height(Some(height))
            .show_with_output(ui, |ui, cell, row, text_color| {
                if cell.col_nr != FILE_COLUMN {
                    return crate::tables::status_cell(ui, row, palette);
                }
                let entry = provider.entry(usize::try_from(cell.row_nr).ok()?)?;
                let (response, clicked) = file_cell(ui, entry, side, busy, text_color);
                if clicked {
                    button_event = Some(WorkEvent::Stage(StageRequest::Files {
                        paths: paths(entry, side),
                        direction: side.direction(),
                    }));
                }
                Some(response)
            })
    });
    match shown {
        Ok(output) => {
            if let Some(clicked) = output
                .events
                .iter()
                .find(|event| matches!(event.kind, RowEventKind::Clicked | RowEventKind::Activated))
                && let Some(entry) = provider.entry(clicked.row_index)
            {
                event = Some(WorkEvent::Open(WorkFile::of(entry, side.which(entry))));
            }
            if let Some(right) = output
                .events
                .iter()
                .find(|event| event.kind == RowEventKind::SecondaryClicked)
            {
                list.menu = Some(RowMenu {
                    files: Arc::clone(files),
                    target: TreeMenu::File(right.row_index),
                });
                list.open_menu = true;
            }
        }
        Err(error) => {
            ui.label(
                RichText::new(format!("The list couldn't be drawn: {error}")).color(palette.danger),
            );
        }
    }
    button_event.or(event)
}

/// A row's button staging or unstaging what it names (`name`, in words), exactly a target's
/// size at the row's right (a button egui sizes grows to a control's height, past the row);
/// returns whether it was clicked.
fn stage_button(ui: &mut Ui, (name, side): (&str, Side), key: &[u8], busy: bool) -> bool {
    let glyph = match side {
        Side::Unstaged => icon::PLUS,
        Side::Staged => icon::MINUS,
    };
    let target = egui::vec2(crate::widgets::TARGET, crate::widgets::TARGET);
    let (place, _) = ui.allocate_exact_size(target, egui::Sense::hover());
    let id = ui.id().with(("stage-file", key));
    let button =
        crate::widgets::icon_button(ui, place, id, glyph, name, !busy).on_hover_text(side.verb());
    button.clicked() && !busy
}

/// A file's cell: its button to stage or unstage it on the right, its path (elided from the
/// start) in what is left; returns the cell and whether the button was clicked.
fn file_cell(
    ui: &mut Ui,
    entry: StatusEntry<'_>,
    side: Side,
    busy: bool,
    text_color: egui::Color32,
) -> (Response, bool) {
    let path = entry.path.to_str_lossy();
    let mut clicked = false;
    // Across the cell's height, as table-kit centers its own cells' text.
    let room = ui.available_size();
    let response = ui
        .allocate_ui_with_layout(room, Layout::right_to_left(Align::Center), |ui| {
            // Named for what it does to which file: a screen reader hears more than "+".
            let name = format!("{} {path}", side.verb());
            clicked = stage_button(ui, (&name, side), entry.path.as_bytes(), busy);
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                let font = egui::TextStyle::Body.resolve(ui.style());
                let shown = elided_start(ui, &path, &font, ui.available_width());
                let elided = matches!(shown, Cow::Owned(_));
                let label = ui.add(
                    Label::new(RichText::new(shown.into_owned()).color(text_color))
                        .selectable(false)
                        .truncate(),
                );
                if elided {
                    label.on_hover_text(path.as_ref());
                }
            });
        })
        .response;
    (response, clicked)
}

/// The column with a file's path and button.
const FILE_COLUMN: usize = 1;

/// The columns of a list.
const HEADERS: [&str; 2] = ["Change", "File"];

/// A list's rows, for egui-table-kit.
struct Entries<'a> {
    status: &'a Status,
    indices: &'a [u32],
    side: Side,
}

impl<'a> Entries<'a> {
    fn entry(&self, row: usize) -> Option<StatusEntry<'a>> {
        let index = usize::try_from(*self.indices.get(row)?).ok()?;
        self.status.entry(index)
    }

    fn cell(&self, entry: StatusEntry<'a>, col_index: usize) -> Option<TableCell<'a>> {
        let text = match col_index {
            0 => Cow::Borrowed(change_name(entry, self.side.which(entry))),
            FILE_COLUMN => match (self.side, entry.from) {
                (Side::Staged, Some(from)) => Cow::Owned(format!(
                    "{} → {}",
                    from.to_str_lossy(),
                    entry.path.to_str_lossy()
                )),
                _ => entry.path.to_str_lossy(),
            },
            _ => return None,
        };
        Some((text, None))
    }
}

/// A row of a list.
struct EntryRow<'a>(&'a Entries<'a>, StatusEntry<'a>);

impl Row for EntryRow<'_> {
    fn cell(&self, col_index: usize) -> Option<TableCell<'_>> {
        self.0.cell(self.1, col_index)
    }

    fn column_count(&self) -> usize {
        HEADERS.len()
    }
}

impl TableProvider for Entries<'_> {
    fn column_count(&self) -> usize {
        HEADERS.len()
    }

    fn header(&self, index: usize) -> Option<Cow<'_, str>> {
        HEADERS.get(index).map(|&name| Cow::Borrowed(name))
    }

    fn headers(&self) -> HeaderIter<'_> {
        HeaderIter::new(self)
    }

    fn row_count(&self) -> usize {
        self.indices.len()
    }

    fn cell_at(
        &self,
        row_index: usize,
        col_index: usize,
    ) -> Result<Option<TableCell<'_>>, TableError> {
        Ok(self
            .entry(row_index)
            .and_then(|entry| self.cell(entry, col_index)))
    }

    fn for_row_at(&self, index: usize, f: &mut RowCallback<'_>) -> Result<(), TableError> {
        self.entry(index)
            .map_or(Ok(()), |entry| f(&EntryRow(self, entry)))
    }

    fn for_selected_rows(
        &self,
        state: &TableState,
        f: &mut RowCallback<'_>,
    ) -> Result<(), TableError> {
        for index in &state.selected_rows {
            self.for_row_at(usize::try_from(index).unwrap_or(usize::MAX), f)?;
        }
        Ok(())
    }

    fn for_all_rows(&self, f: &mut RowCallback<'_>) -> Result<(), TableError> {
        for row in 0..self.indices.len() {
            self.for_row_at(row, f)?;
        }
        Ok(())
    }
}
