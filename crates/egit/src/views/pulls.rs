//! A repository's pull requests (GitHub's) or merge requests (GitLab's), in the commit table's
//! place, each named its forge's way: the open ones as rows
//! (each repository's most recently updated first), the selected one's details below (read when
//! it is selected), with ways to show its head commit, check it out, and open its page; or why
//! they can't be read (signing in would help, a rate limit, …).

use std::fmt::Write as _;
use std::sync::Arc;

use bstr::{BString, ByteSlice as _};
use egit_forge::{
    CheckState, Mergeable, PullRequestDetails, ReviewDecision, ReviewState, pull_request_details,
};
use egit_git::{CancelToken, ObjectId, SwitchTo, check_ref_name};
use egui::{Color32, Frame, Id, Key, Margin, Modifiers, RichText, Sense, Ui};
use egui_async::Bind;

use super::history::Shared;
use crate::forge::{ForgeProblem, Listed, RepoForge, fork_head, request_noun, request_number};
use crate::format;
use crate::history::HistoryState;
use crate::jobs;
use crate::problem::Problem;
use crate::tasks::Task;
use crate::theme::Weighted as _;
use crate::theme::{Palette, icon};
use crate::widgets::{self, muted};

/// A row's height in the list.
const ROW_HEIGHT: f32 = 26.0;

/// The pull request view's state.
#[derive(Debug)]
pub(super) struct PullRequestView {
    /// The selected pull request: its repository's place in the forge's, and its number.
    selected: Option<(usize, u32)>,
    /// Scroll the selected row into view next frame.
    reveal: bool,
    details: Bind<Arc<PullRequestDetails>, Problem>,
    /// Which pull request the details are of (or being read for).
    details_of: Option<(usize, u32)>,
    /// The selected pull request's head commit's row in the history, once looked for (with the
    /// history's length then: a longer history is looked through again).
    head_row: Option<((usize, u32), u64, Option<u64>)>,
    cancel: CancelToken,
}

impl Drop for PullRequestView {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// What the view asks of the history view.
pub(super) enum PullRequestEvent {
    Close,
    ShowCommit(ObjectId),
    Task(Task),
    Open(String),
    SignIn(String),
    Refresh,
}

impl PullRequestView {
    /// The view, with `selected` (a repository's place, and a number) selected if given.
    pub(super) fn new(selected: Option<(usize, u32)>) -> Self {
        Self {
            selected,
            reveal: selected.is_some(),
            details: Bind::new(true),
            details_of: None,
            head_row: None,
            cancel: CancelToken::new(),
        }
    }

    /// Reads the selected pull request's details, unless they are read or being read.
    fn read_details(&mut self, forge: &RepoForge) {
        let Some(selected) = self.selected else {
            return;
        };
        if self.details_of == Some(selected) {
            return;
        }
        let Some(repository) = forge.repositories.get(selected.0).cloned() else {
            return;
        };
        let Some(account) = forge.account(repository.host()) else {
            return;
        };
        self.cancel.cancel();
        self.cancel = CancelToken::new();
        self.details_of = Some(selected);
        let cancel = self.cancel.clone();
        let title = format!(
            "The {}'s details couldn't be read",
            request_noun(repository.host().kind())
        );
        self.details.request(jobs::blocking(move || {
            pull_request_details(&account, &repository, selected.1, &cancel)
                .map(Arc::new)
                .map_err(|error| Problem::new(title, error.to_string()))
        }));
    }
}

/// Draws the pull requests; returns what they ask of the history view.
pub(super) fn show(
    ui: &mut Ui,
    shared: &Shared<'_>,
    history: &HistoryState,
    view: &mut PullRequestView,
) -> Option<PullRequestEvent> {
    let forge = shared.forge;
    let escape = !ui.ctx().text_edit_focused()
        && ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape));
    let mut event = escape.then_some(PullRequestEvent::Close);
    let found = forge.found();
    let count = found.pulls.len();
    keys(ui, view, &found.pulls);
    if view
        .selected
        .is_none_or(|selected| index_of(&found.pulls, selected).is_none())
        && let Some(first) = found.pulls.first()
    {
        view.selected = Some((first.repository, first.pull.number));
    }
    let problem = found.problem.clone();
    let totals: u64 = found
        .totals
        .iter()
        .flatten()
        .map(|&total| u64::from(total))
        .sum();
    drop(found);
    view.read_details(forge);
    if let Some(asked) = bar(ui, shared, count, totals, problem.as_ref()) {
        event = Some(asked);
    }
    if count == 0 {
        egui::CentralPanel::default()
            .frame(Frame::new().inner_margin(Margin::same(16)))
            .show(ui, |ui| {
                if let Some(asked) = empty(ui, shared, problem.as_ref()) {
                    event = Some(asked);
                }
            });
        return event;
    }
    egui::Panel::top(Id::new(("pull-request-list", shared.id)))
        .resizable(true)
        .default_size(260.0)
        .size_range(96.0..=700.0)
        .frame(Frame::new())
        .show(ui, |ui| list(ui, shared, view));
    egui::CentralPanel::default()
        .frame(Frame::new().inner_margin(Margin::same(12)))
        .show(ui, |ui| {
            if let Some(asked) = details(ui, shared, history, view) {
                event = Some(asked);
            }
        });
    event
}

/// The place in `pulls` of the pull request `selected` names.
fn index_of(pulls: &[Listed], selected: (usize, u32)) -> Option<usize> {
    pulls
        .iter()
        .position(|listed| (listed.repository, listed.pull.number) == selected)
}

/// ↑ and ↓ step through the pull requests, while no text field has the focus.
fn keys(ui: &Ui, view: &mut PullRequestView, pulls: &[Listed]) {
    if ui.ctx().text_edit_focused() || pulls.is_empty() {
        return;
    }
    let pressed = |key| ui.input_mut(|input| input.consume_key(Modifiers::NONE, key));
    let at = view
        .selected
        .and_then(|selected| index_of(pulls, selected))
        .unwrap_or(0);
    let to = if pressed(Key::ArrowUp) {
        at.saturating_sub(1)
    } else if pressed(Key::ArrowDown) {
        at.saturating_add(1).min(pulls.len().saturating_sub(1))
    } else {
        return;
    };
    if let Some(listed) = pulls.get(to) {
        view.selected = Some((listed.repository, listed.pull.number));
        view.reveal = true;
    }
}

/// The bar: back, which repositories, how many are open, reading, Refresh, and what went wrong.
fn bar(
    ui: &mut Ui,
    shared: &Shared<'_>,
    count: usize,
    totals: u64,
    problem: Option<&ForgeProblem>,
) -> Option<PullRequestEvent> {
    let forge = shared.forge;
    let palette = shared.palette;
    let mut event = None;
    widgets::bar(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            if widgets::secondary(ui, true, &format!("{}  History", icon::ARROW_LEFT)).clicked() {
                event = Some(PullRequestEvent::Close);
            }
            ui.label(RichText::new(capitalized(forge.nouns())).color(palette.muted));
            let names: Vec<String> = forge
                .repositories
                .iter()
                .map(egit_forge::ForgeRepository::full_name)
                .collect();
            ui.add(egui::Label::new(RichText::new(names.join(", ")).semibold()).truncate());
            let shown = u64::try_from(count).unwrap_or(u64::MAX);
            let open = if totals > shown {
                format!(
                    "{} of {} open: the most recently updated",
                    format::count(shown),
                    format::count(totals)
                )
            } else {
                format!("{} open", format::count(shown))
            };
            // Nothing read for a reason isn't none open.
            if count > 0 || problem.is_none() {
                ui.label(muted(ui, open));
            }
            // What comes and goes follows the buttons.
            if widgets::secondary(ui, !forge.is_reading(), "Refresh").clicked() {
                event = Some(PullRequestEvent::Refresh);
            }
            if forge.is_reading() {
                ui.spinner();
            }
            if count > 0
                && let Some(problem) = problem
            {
                ui.label(
                    RichText::new(format!("{}  {}", icon::WARNING_CIRCLE, said(problem)))
                        .color(palette.danger),
                );
            }
        });
    });
    event
}

/// No pull requests: none open, still reading, or why they couldn't be read (and the way to
/// sign in, when that would help).
fn empty(
    ui: &mut Ui,
    shared: &Shared<'_>,
    problem: Option<&ForgeProblem>,
) -> Option<PullRequestEvent> {
    let mut event = None;
    let nouns = shared.forge.nouns();
    match problem {
        Some(ForgeProblem::SignIn { host, refused }) => {
            let account = shared
                .forge
                .accounts()
                .iter()
                .find(|account| account.host().name() == host);
            let kind = account.map_or_else(|| shared.forge.kind(), |account| account.host().kind());
            widgets::card(ui, |ui| {
                widgets::heading(ui, &format!("{}  Sign in to {host}", icon::SIGN_IN));
                ui.add_space(4.0);
                if *refused {
                    ui.label(format!(
                        "{host} refused the credentials git gave it. Sign in with a token that \
                         can read this repository."
                    ));
                } else {
                    ui.label(format!(
                        "Sign in to {host} to see its {}s.",
                        request_noun(kind)
                    ));
                }
                ui.label(muted(
                    ui,
                    format!(
                        "egit asks git for the credentials it uses for {host}: your credential \
                         helper's, or a personal access token you give as the password. egit \
                         keeps none of its own."
                    ),
                ));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let signing = shared.forge.signing;
                    if widgets::primary(ui, !signing, "Sign in").clicked() {
                        event = Some(PullRequestEvent::SignIn(host.clone()));
                    }
                    if let Some(account) = account
                        && widgets::secondary(ui, true, "Create a token…")
                            .on_hover_text(format!(
                                "Its page on the host, with the scope {}s need",
                                request_noun(kind)
                            ))
                            .clicked()
                    {
                        event = Some(PullRequestEvent::Open(account.host().new_token_url()));
                    }
                });
                if shared.forge.signing {
                    ui.add_space(6.0);
                    widgets::working(ui, "Signing in…");
                } else if let Some(problem) = &shared.forge.sign_in_problem {
                    ui.add_space(6.0);
                    widgets::problem_box(ui, problem, |_| {});
                }
            });
        }
        Some(problem) => {
            let problem = Problem::new(format!("The {nouns} couldn't be read"), said(problem));
            widgets::problem_box(ui, &problem, |ui| {
                let again = widgets::secondary(ui, true, "Try again");
                crate::help::note_topic(ui, again.rect, "pull-request-list");
                if again.clicked() {
                    event = Some(PullRequestEvent::Refresh);
                }
            });
        }
        None if shared.forge.is_reading() => {
            widgets::working(ui, &format!("Reading the {nouns}…"));
        }
        None => {
            ui.label(muted(ui, format!("No {nouns} are open.")));
        }
    }
    event
}

/// A problem, in words.
fn said(problem: &ForgeProblem) -> String {
    match problem {
        ForgeProblem::SignIn {
            host,
            refused: false,
        } => format!("Not signed in to {host}"),
        ForgeProblem::SignIn {
            host,
            refused: true,
        } => {
            format!("{host} refused the credentials")
        }
        ForgeProblem::RateLimited { host, reset } => {
            let until = reset
                .and_then(|reset| u64::try_from(reset).ok())
                .map_or_else(String::new, |reset| {
                    let left = reset.saturating_sub(format::now());
                    let minutes = left.div_ceil(60).max(1);
                    format!(
                        " for {minutes} more minute{}",
                        if minutes == 1 { "" } else { "s" }
                    )
                });
            format!("{host}'s rate limit is used up{until}")
        }
        ForgeProblem::Other(problem) => problem.detail.clone(),
    }
}

/// The pull requests, drawn as far as they show.
fn list(ui: &mut Ui, shared: &Shared<'_>, view: &mut PullRequestView) {
    let now = format::now();
    let found = shared.forge.found();
    let pitch = ROW_HEIGHT + ui.spacing().item_spacing.y;
    let mut scroll = egui::ScrollArea::vertical()
        .id_salt(("pull-request-rows", shared.id))
        .auto_shrink([false, false]);
    if std::mem::take(&mut view.reveal)
        && let Some(at) = view
            .selected
            .and_then(|selected| index_of(&found.pulls, selected))
    {
        let row = f32::from(u16::try_from(at).unwrap_or(u16::MAX));
        let offset = ui
            .ctx()
            .data(|data| data.get_temp::<f32>(Id::new(("pull-request-offset", shared.id))))
            .unwrap_or(0.0);
        let top = (row - 1.0).max(0.0) * pitch;
        let bottom = (row + 2.0) * pitch - ui.available_height().max(pitch);
        if top < offset {
            scroll = scroll.vertical_scroll_offset(top);
        } else if bottom > offset {
            scroll = scroll.vertical_scroll_offset(bottom);
        }
    }
    let several = shared.forge.repositories.len() > 1;
    let shown = scroll.show_rows(ui, ROW_HEIGHT, found.pulls.len(), |ui, visible| {
        for listed in found.pulls.get(visible).unwrap_or_default() {
            let key = (listed.repository, listed.pull.number);
            let selected = view.selected == Some(key);
            if row(ui, shared, listed, selected, several, now) {
                view.selected = Some(key);
            }
        }
    });
    drop(found);
    ui.ctx().data_mut(|data| {
        data.insert_temp(
            Id::new(("pull-request-offset", shared.id)),
            shown.state.offset.y,
        );
    });
}

/// One pull request's row; returns whether it was clicked.
fn row(
    ui: &mut Ui,
    shared: &Shared<'_>,
    listed: &Listed,
    selected: bool,
    several: bool,
    now: u64,
) -> bool {
    let palette = shared.palette;
    let pull = &listed.pull;
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_HEIGHT), Sense::hover());
    let response = ui.interact(
        rect,
        Id::new(("pull-request", listed.repository, pull.number)),
        Sense::click(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            true,
            selected,
            &pull.title,
        )
    });
    crate::widgets::row_fill(ui, rect, 0.0, selected, response.hovered());
    crate::help::note_topic(ui, rect, "pull-request-list");
    let mut row = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(egui::vec2(8.0, 0.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    let mut number = request_number(shared.forge.kind_of(listed.repository), pull.number);
    if several && let Some(repository) = shared.forge.repositories.get(listed.repository) {
        number = format!("{} {number}", repository.name());
    }
    row.add_sized(
        [56.0, ROW_HEIGHT],
        egui::Label::new(RichText::new(number).monospace().color(palette.muted))
            .selectable(false)
            .truncate(),
    );
    let (glyph, color, meaning) = check_look(pull.checks, palette);
    row.add(egui::Label::new(RichText::new(glyph).color(color)).selectable(false))
        .on_hover_text(meaning);
    row.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let age = u64::try_from(pull.updated).map_or_else(
            |_| String::new(),
            |updated| crate::blame::compact_age(updated, now),
        );
        ui.add_sized(
            [32.0, ROW_HEIGHT],
            egui::Label::new(muted(ui, age)).selectable(false),
        );
        if let Some((glyph, color, meaning)) = review_look(pull.review, palette) {
            ui.add(egui::Label::new(RichText::new(glyph).color(color)).selectable(false))
                .on_hover_text(meaning);
        }
        let mut whose = format!(
            "{} · {} {} {}",
            pull.author,
            pull.head_branch,
            icon::ARROW_RIGHT,
            pull.base_branch
        );
        if pull.cross_repository
            && let Some(fork) = &pull.head_repository
        {
            let _ = write!(whose, " (from {fork})");
        }
        ui.add(
            egui::Label::new(muted(ui, whose))
                .selectable(false)
                .truncate(),
        );
        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
            if pull.draft {
                chip(ui, "Draft", palette.muted);
            }
            ui.add(
                egui::Label::new(RichText::new(&pull.title).color(palette.text))
                    .selectable(false)
                    .truncate(),
            );
        });
    });
    response.clicked()
}

/// A small label in a frame.
fn chip(ui: &mut Ui, text: &str, color: Color32) {
    Frame::new()
        .stroke(egui::Stroke::new(1.0, color))
        .corner_radius(egui::CornerRadius::same(crate::theme::RADIUS_INNER))
        .inner_margin(Margin::symmetric(4, 0))
        .show(ui, |ui| {
            ui.add(egui::Label::new(RichText::new(text).size(11.0).color(color)).selectable(false));
        });
}

/// A checks state's icon, colour, and meaning.
pub(super) const fn check_look(
    state: CheckState,
    palette: Palette,
) -> (&'static str, Color32, &'static str) {
    match state {
        CheckState::Success => (icon::CHECK_CIRCLE, palette.ok, "Checks passed"),
        CheckState::Failure => (icon::X_CIRCLE, palette.danger, "Checks failed"),
        CheckState::Pending => (icon::CIRCLE_DASHED, palette.warn, "Checks are running"),
        CheckState::Neutral => (
            icon::MINUS_CIRCLE,
            palette.muted,
            "Checks neither passed nor failed",
        ),
        CheckState::None => (icon::DOT_OUTLINE, palette.muted, "No checks"),
    }
}

/// A review decision's icon, colour, and meaning (none without a decision).
const fn review_look(
    review: ReviewDecision,
    palette: Palette,
) -> Option<(&'static str, Color32, &'static str)> {
    match review {
        ReviewDecision::Approved => Some((icon::CHECK, palette.ok, "Approved")),
        ReviewDecision::ChangesRequested => {
            Some((icon::CHAT_CIRCLE, palette.danger, "Changes requested"))
        }
        ReviewDecision::Required => Some((icon::EYE, palette.muted, "Review required")),
        ReviewDecision::None => None,
    }
}

/// The selected pull request: what it is, its details, and what can be done with it.
fn details(
    ui: &mut Ui,
    shared: &Shared<'_>,
    history: &HistoryState,
    view: &mut PullRequestView,
) -> Option<PullRequestEvent> {
    let palette = shared.palette;
    let selected = view.selected?;
    let listed = {
        let found = shared.forge.found();
        found.pulls.get(index_of(&found.pulls, selected)?)?.clone()
    };
    let pull = &listed.pull;
    let repository = shared.forge.repositories.get(listed.repository)?.clone();
    let kind = repository.host().kind();
    let head_row = head_row(history, view, selected, pull.head_commit);
    let mut event = None;
    egui::ScrollArea::vertical()
        .id_salt(("pull-request-details", shared.id))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                widgets::panel_title(ui, pull.title.as_str());
                ui.label(muted(ui, request_number(kind, pull.number)));
                if pull.draft {
                    chip(ui, "Draft", palette.muted);
                }
            });
            let mut whose = format!(
                "{} wants to merge {} into {}",
                pull.author, pull.head_branch, pull.base_branch
            );
            if pull.cross_repository {
                let _ = write!(
                    whose,
                    ", from {}",
                    pull.head_repository.as_deref().unwrap_or("a deleted fork")
                );
            }
            ui.label(muted(ui, whose));
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                if widgets::secondary(ui, head_row.is_some(), "Show the commit")
                    .on_disabled_hover_text(
                        "Its head commit isn't in this history: fetch, or check it out",
                    )
                    .clicked()
                    && let Some(head) = pull.head_commit
                {
                    event = Some(PullRequestEvent::ShowCommit(head));
                }
                let check_out = check_out(shared, history, &listed);
                let (allowed, hover) = match &check_out {
                    Ok((task, hover)) => (Some(task.clone()), hover.clone()),
                    Err(why) => (None, why.clone()),
                };
                let button = widgets::secondary(ui, allowed.is_some(), "Check out");
                crate::help::note_topic(ui, button.rect, "pull-request-check-out");
                if button
                    .on_hover_text(&hover)
                    .on_disabled_hover_text(&hover)
                    .clicked()
                    && let Some(task) = allowed
                {
                    event = Some(PullRequestEvent::Task(task));
                }
                if widgets::secondary(
                    ui,
                    true,
                    &format!("{}  Open on {}", icon::ARROW_SQUARE_OUT, kind.name()),
                )
                .clicked()
                {
                    event = Some(PullRequestEvent::Open(
                        repository.pull_request_url(pull.number),
                    ));
                }
            });
            ui.add_space(8.0);
            match view.details.read_as_ref() {
                None => widgets::working(ui, "Reading its details…"),
                Some(Err(problem)) => {
                    let problem = problem.clone();
                    widgets::problem_box(ui, &problem, |_| {});
                }
                Some(Ok(details)) => {
                    let details = Arc::clone(details);
                    if let Some(asked) = described(ui, palette, &details) {
                        event = Some(asked);
                    }
                }
            }
        });
    event
}

/// The head commit's row in the history, looked for when the selection (or the history's
/// length) changes: a scan, so not every frame.
fn head_row(
    history: &HistoryState,
    view: &mut PullRequestView,
    selected: (usize, u32),
    head: Option<ObjectId>,
) -> Option<u64> {
    let rows = history.shown.rows();
    match view.head_row {
        Some((of, length, row)) if of == selected && length == rows => row,
        _ => {
            let row = head.and_then(|head| history.shown.row_of(&head, 0));
            view.head_row = Some((selected, rows, row));
            row
        }
    }
}

/// How checking a pull request out goes (its task, and what it does), or why it can't.
fn check_out(
    shared: &Shared<'_>,
    history: &HistoryState,
    listed: &Listed,
) -> Result<(Task, String), String> {
    let pull = &listed.pull;
    if shared.busy {
        return Err("Another task is running".to_owned());
    }
    if shared.operation.is_some() {
        return Err("Finish or abort the operation in progress first".to_owned());
    }
    let remote = shared
        .forge
        .remote_of(listed.repository)
        .ok_or_else(|| "None of the remotes is its repository".to_owned())?
        .to_owned();
    let current = shared.branch();
    let kind = shared.forge.kind_of(listed.repository);
    if pull.cross_repository {
        let (source, branch) = fork_head(kind, pull.number);
        if current == Some(branch.as_bytes().as_bstr()) {
            return Err(format!("Checked out already, as {branch}"));
        }
        return Ok((
            Task::CheckOutPullRequest {
                remote: remote.clone(),
                refspec: format!("{source}:refs/heads/{branch}").into(),
                kind,
                number: pull.number,
                to: SwitchTo::Branch(branch.clone().into()),
            },
            format!(
                "Fetches it from {} into {branch}, and switches to it",
                remote.to_str_lossy()
            ),
        ));
    }
    let head = pull.head_branch.as_str();
    check_ref_name(&format!("refs/heads/{head}"))
        .map_err(|_| format!("{head} isn't a branch name git takes"))?;
    let remote_name = remote.to_str_lossy().into_owned();
    let tracking = format!("{remote_name}/{head}");
    let refspec: BString = format!("+refs/heads/{head}:refs/remotes/{tracking}").into();
    let (to, hover) = if history.shown.refs().has_branch(head.as_bytes().as_bstr()) {
        (
            SwitchTo::Branch(head.into()),
            format!("Fetches {tracking}, and switches to {head}"),
        )
    } else {
        (
            SwitchTo::Track {
                remote_branch: tracking.clone().into(),
                local: head.into(),
            },
            format!("Fetches {tracking}, and switches to {head}, tracking it"),
        )
    };
    Ok((
        Task::CheckOutPullRequest {
            remote,
            refspec,
            kind,
            number: pull.number,
            to,
        },
        hover,
    ))
}

/// A pull request's details: its size, labels, checks, reviews, and description.
fn described(
    ui: &mut Ui,
    palette: Palette,
    details: &PullRequestDetails,
) -> Option<PullRequestEvent> {
    let mut event = None;
    let mut size = format!(
        "+{} −{} in {} file{}, {} commit{}",
        details.additions,
        details.deletions,
        details.changed_files,
        if details.changed_files == 1 { "" } else { "s" },
        details.commits,
        if details.commits == 1 { "" } else { "s" },
    );
    match details.mergeable {
        Mergeable::Conflicting => size.push_str(" · conflicts with its base"),
        Mergeable::Yes => size.push_str(" · merges cleanly"),
        Mergeable::Unknown => {}
    }
    ui.label(size);
    if !details.labels.is_empty() {
        ui.horizontal_wrapped(|ui| {
            for label in &details.labels {
                chip(ui, label, palette.accent);
            }
        });
    }
    if !details.checks.is_empty() {
        ui.add_space(8.0);
        ui.label(RichText::new("Checks").semibold());
        for check in &details.checks {
            ui.horizontal(|ui| {
                let (glyph, color, meaning) = check_look(check.state, palette);
                ui.label(RichText::new(glyph).color(color))
                    .on_hover_text(meaning);
                ui.add(egui::Label::new(check.name.as_str()).truncate());
                if let Some(url) = &check.url
                    && {
                        let details = ui.add(crate::widgets::small(muted(ui, "Details")));
                        crate::help::note_topic(ui, details.rect, "pull-request-checks");
                        details.on_hover_text(url).clicked()
                    }
                {
                    event = Some(PullRequestEvent::Open(url.clone()));
                }
            });
        }
    }
    if !details.reviews.is_empty() {
        ui.add_space(8.0);
        ui.label(RichText::new("Reviews").semibold());
        for review in &details.reviews {
            let (glyph, color, said) = match review.state {
                ReviewState::Approved => (icon::CHECK, palette.ok, "approved"),
                ReviewState::ChangesRequested => {
                    (icon::CHAT_CIRCLE, palette.danger, "asked for changes")
                }
                ReviewState::Commented => (icon::CHAT_CIRCLE, palette.muted, "commented"),
                ReviewState::Dismissed => (icon::MINUS_CIRCLE, palette.muted, "was dismissed"),
                ReviewState::Pending => (icon::CLOCK, palette.muted, "hasn't submitted"),
            };
            ui.horizontal(|ui| {
                ui.label(RichText::new(glyph).color(color));
                ui.label(format!("{} {said}", review.author));
            });
        }
    }
    ui.add_space(8.0);
    ui.label(RichText::new("Description").semibold());
    let body = details.body.trim();
    if body.is_empty() {
        ui.label(muted(ui, "No description."));
    } else {
        Frame::new()
            .fill(palette.surface)
            .corner_radius(egui::CornerRadius::same(crate::theme::RADIUS_BOX))
            .inner_margin(Margin::same(8))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.add(egui::Label::new(body).wrap());
            });
    }
    event
}

/// `text` with its first letter capitalized.
fn capitalized(text: &str) -> String {
    let mut letters = text.chars();
    letters.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(letters).collect()
    })
}
