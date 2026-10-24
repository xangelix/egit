//! A CI/CD job's log in the pipeline page's place (a job's click there), a GitLab job's or a
//! GitHub Actions job's: a bar (back to the pipeline, the job's status and how long it has run,
//! why it failed, its runs (the latest and those retried before it) to switch between,
//! retrying, cancelling, or starting it as its forge allows (in its words: [`CiWords`]), its
//! sections' outline shown or not, wrapping, its page on the forge), then the log in the text
//! grid as the forge shows it (colours, each line's time beside its number where the runner
//! wrote them, sections' headers marked with a caret and how long they took, folded as the
//! forge folds them: `log_folds`), its outline at its left (`log_outline`). A click on a
//! section's header folds or unfolds it; Alt+↑ and Alt+↓ go to the section before or after.
//! It is read again every few seconds while the job runs, the view following its end while it
//! rests there (scrolled up, it stays, "Follow the end" a click away). GitHub gives a job's
//! log once it has ended: meanwhile its steps show as they go. A retry shows the new run as it
//! starts. Escape, or the back button, goes back to the pipeline (read again).

use std::sync::Arc;

use egit_forge::{CiAction, CiStatus, EarlierRun, ForgeKind, Job, JobRun, JobStep};
use egui::{Align, Frame, Id, Key, Layout, Modifiers, RichText, Ui};

use super::history::Shared;
use super::log_outline::{Outline, OutlineAsk, Standing};
use super::pipeline::{
    ARTIFACTS_TOPIC, Acting, CiWords, PipelineView, Reach, artifact_items, ci_look, ci_words,
    duration,
};
use crate::downloads::ArtifactWanted;
use crate::forge::RepoForge;
use crate::format;
use crate::grid::TextGrid;
use crate::job_log::{Following, JobFollower, LogSource};
use crate::log_folds::{Folding, Shown};
use crate::problem::Problem;
use crate::theme::Weighted as _;
use crate::theme::{Palette, icon};
use crate::widgets::{self, Look, muted};

/// The help topic explaining it.
const TOPIC: &str = "job-log";

/// Where whether the outline shows is kept (for every job's log).
const OUTLINE_SHOWN: &str = "job-log-outline-shown";

/// A job's log, shown.
#[derive(Debug)]
pub(super) struct JobView {
    /// The pipeline's page, gone back to.
    pub(super) pipeline: Box<PipelineView>,
    /// The job, as the pipeline's page had it (a retry here making its latest run the new one).
    job: Job,
    /// The run shown: the latest's number, or an earlier one's.
    run: u64,
    /// Its project and the account it is read with (none where they are gone).
    reach: Option<Reach>,
    /// Its forge: GitLab's jobs are retried, cancelled, and started one by one; a GitHub job is
    /// re-run once its run has ended.
    kind: ForgeKind,
    /// The run's log followed (none where its project or account is gone).
    follower: Option<JobFollower>,
    /// Why it can't be followed, when it can't.
    problem: Option<Problem>,
    grid: TextGrid,
    /// The log's version drawn last (0: none yet).
    drawn: u64,
    /// Whether the view follows the log's end (it rested there).
    follow: bool,
    /// What was last asked of the forge, and how it went.
    acting: Acting,
    /// Which sections are folded, and what is shown of the log.
    folding: Folding,
    /// What was shown last frame (to keep its top in place as folds change).
    shown: Option<Shown>,
    /// The outline's state.
    outline: Outline,
    /// A line of the whole log asked for, shown once what is shown has it.
    going: Option<u32>,
}

/// What the view asks of the history view.
pub(super) enum JobEvent {
    /// Back to the pipeline.
    Back,
    /// Open a page in the browser.
    Open(String),
    /// The forge did what was asked: the commits' marks read again.
    Acted,
    /// One of the job's artifacts, saved where the user says.
    Download(Box<ArtifactWanted>),
}

impl JobView {
    /// The log of `job`'s run `run` (its latest's number, or an earlier one's), of the
    /// pipeline `pipeline` shows, followed through `forge`'s account for its project (waking
    /// `waker` as it lands).
    pub(super) fn new(
        pipeline: Box<PipelineView>,
        (job, run): (Job, u64),
        forge: &RepoForge,
        waker: &egui::Context,
    ) -> Self {
        let reach = pipeline.reached(forge);
        let kind = forge.kind_of(pipeline.repository());
        let mut view = Self {
            pipeline,
            job,
            run,
            reach,
            kind,
            follower: None,
            problem: None,
            grid: TextGrid::new(Id::new(("job-log", run))),
            drawn: 0,
            follow: true,
            acting: Acting::new(),
            folding: Folding::default(),
            shown: None,
            outline: Outline::default(),
            going: None,
        };
        view.show_run(run, waker);
        view
    }

    /// Follows run `run`'s log from its start.
    fn show_run(&mut self, run: u64, waker: &egui::Context) {
        self.run = run;
        self.grid = TextGrid::new(Id::new(("job-log", run)));
        self.drawn = 0;
        self.follow = true;
        self.follower = None;
        self.folding = Folding::default();
        self.shown = None;
        self.outline = Outline::default();
        self.going = None;
        match self.reach.clone() {
            Some(reach) => {
                self.problem = None;
                self.follower = Some(JobFollower::start(reach, run, waker.clone()));
            }
            None => {
                self.problem = Some(Problem::new(
                    "The job's log can't be read",
                    "Its project is no longer among the remotes.",
                ));
            }
        }
    }

    /// Whether the run shown is the job's latest.
    fn is_latest(&self) -> bool {
        self.job.id == Some(self.run)
    }

    /// What the pipeline's page knew of the run shown.
    fn known(&self) -> EarlierRun {
        self.job
            .earlier
            .iter()
            .find(|earlier| earlier.id == self.run)
            .cloned()
            .unwrap_or_else(|| EarlierRun {
                id: self.run,
                status: self.job.status,
                duration: self.job.duration,
                started: self.job.started,
                url: self.job.url.clone(),
            })
    }

    /// Asks `action` of the forge (in a job).
    fn act(&mut self, action: CiAction) {
        self.acting.ask(self.reach.clone(), action);
    }

    /// Takes in what the forge said to an action, once: a retry's new run becomes the latest and
    /// shows, the run is followed again after any other; returns whether one went through.
    fn landed(&mut self, run: Option<&JobRun>, waker: &egui::Context) -> bool {
        let Some(landed) = self.acting.landed() else {
            return false;
        };
        match landed {
            (CiAction::RetryJob(_), Some(new)) if new != self.run => {
                // The run shown is now the latest's earlier one.
                let before = EarlierRun {
                    id: self.run,
                    status: run.map_or(self.job.status, |run| run.status),
                    duration: self.job.duration,
                    started: run.and_then(|run| run.started).or(self.job.started),
                    url: run
                        .and_then(|run| run.url.clone())
                        .or_else(|| self.job.url.clone()),
                };
                self.job.earlier.insert(0, before);
                self.job.id = Some(new);
                self.job.status = CiStatus::Pending;
                self.job.duration = None;
                self.job.started = None;
                self.job.url = None;
                self.show_run(new, waker);
            }
            _ => self.show_run(self.run, waker),
        }
        true
    }
}

#[cfg(test)]
impl JobView {
    /// The log shown, as text, and whether the view follows its end.
    pub(super) fn shown(&self) -> Option<(String, bool)> {
        use crate::grid::GridSource as _;
        let follower = self.follower.as_ref()?;
        let log = follower.following().log.clone()?;
        Some((
            String::from_utf8_lossy(log.text().bytes()).into_owned(),
            self.follow,
        ))
    }

    /// What the grid shows of the log, its folded sections left out.
    pub(super) fn shown_text(&self) -> Option<String> {
        let shown = self.shown.as_ref()?;
        Some(String::from_utf8_lossy(shown.source().text().bytes()).into_owned())
    }

    /// Where on screen the place before cell `cell` of shown line `line` was drawn, if it was.
    #[cfg(test)]
    pub(super) fn spot(&self, line: u32, cell: u32) -> Option<egui::Pos2> {
        self.grid.spot_position(0, line, cell)
    }

    /// The number of the run shown.
    pub(super) const fn run_shown(&self) -> u64 {
        self.run
    }
}

/// What the follower last said, taken out of its lock.
struct Seen {
    run: Option<JobRun>,
    log: Option<std::sync::Arc<LogSource>>,
    version: u64,
    missing: bool,
    problem: Option<Problem>,
    done: bool,
}

impl Seen {
    fn of(following: &Following) -> Self {
        Self {
            run: following.job.clone(),
            log: following.log.clone(),
            version: following.version,
            missing: following.missing,
            problem: following.problem.clone(),
            done: following.done,
        }
    }
}

/// Draws the job's log; returns what it asks of the history view.
pub(super) fn show(ui: &mut Ui, shared: &Shared<'_>, view: &mut JobView) -> Option<JobEvent> {
    let seen = view
        .follower
        .as_ref()
        .map(|follower| Seen::of(&follower.following()));
    let acted = view.landed(seen.as_ref().and_then(|seen| seen.run.as_ref()), ui.ctx());
    let palette = shared.palette;
    let mut event = bar(ui, palette, view, seen.as_ref());
    if acted && event.is_none() {
        event = Some(JobEvent::Acted);
    }
    egui::CentralPanel::default()
        .frame(Frame::new())
        .show(ui, |ui| {
            if view.acting.failed() {
                padded(ui, |ui| view.acting.problem(ui));
            }
            if let Some(problem) = view.problem.clone() {
                padded(ui, |ui| widgets::problem_box(ui, &problem, |_| {}));
                return;
            }
            let Some(seen) = seen else {
                return;
            };
            if let Some(problem) = &seen.problem {
                padded(ui, |ui| {
                    widgets::problem_box(ui, problem, |ui| {
                        ui.label(muted(ui, "egit tries again in a few seconds."));
                    });
                });
            }
            if let Some(log) = &seen.log {
                if log.truncated {
                    padded(ui, |ui| {
                        ui.label(muted(
                            ui,
                            format!(
                                "Only its first {} show here: the rest is on {}.",
                                format::size(
                                    u64::try_from(egit_forge::MOST_TEXT).unwrap_or(u64::MAX)
                                ),
                                CiWords::of(view.kind).forge
                            ),
                        ));
                    });
                }
                let status = seen
                    .run
                    .as_ref()
                    .map_or_else(|| view.known().status, |run| run.status);
                log_view(ui, view, (log, seen.version), status);
            } else {
                padded(ui, |ui| waiting(ui, &seen, view.known().status));
            }
        });
    // Escape goes back, once the grid had it (its find bar, a selection).
    if event.is_none()
        && !ui.ctx().text_edit_focused()
        && ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape))
    {
        event = Some(JobEvent::Back);
    }
    event
}

/// What shows inside the page's margins.
fn padded(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    Frame::new()
        .inner_margin(egui::Margin::symmetric(12, 12))
        .show(ui, add);
}

/// The bar: back to the pipeline, the job (status, name, how long it has run, why it failed),
/// then its sections, wrapping, and its page on its forge, wrapping onto a row of their own where
/// the bar is narrow.
fn bar(ui: &mut Ui, palette: Palette, view: &mut JobView, seen: Option<&Seen>) -> Option<JobEvent> {
    let run = seen.and_then(|seen| seen.run.as_ref());
    let known = view.known();
    let status = run.map_or(known.status, |run| run.status);
    let mut event = None;
    widgets::bar(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            let back = format!("{}  {}", icon::ARROW_LEFT, view.pipeline.title());
            if widgets::secondary(ui, true, &back).clicked() {
                event = Some(JobEvent::Back);
            }
            let (glyph, color) = ci_look(status, palette);
            let name = ui.label(
                RichText::new(format!("{glyph}  {}", view.job.name))
                    .color(color)
                    .semibold(),
            );
            crate::help::note_topic(ui, name.rect, TOPIC);
            let said = ci_words(status, false);
            match ran(run, &known, status) {
                // The clock in a column as wide as the longest it says: ticking (each second),
                // it moves nothing after it.
                Some(took) if status.is_moving() => {
                    ui.label(muted(ui, format!("{said} ·")));
                    let width = widgets::duration_width(ui, &egui::TextStyle::Body)
                        + widgets::text_width(ui, " so far");
                    widgets::column(ui, (width, crate::theme::CONTROL), Align::Min, |ui| {
                        ui.label(muted(ui, took));
                    });
                    ui.ctx()
                        .request_repaint_after(std::time::Duration::from_secs(1));
                }
                Some(took) => {
                    ui.label(muted(ui, format!("{said} · {took}")));
                }
                None => {
                    ui.label(muted(ui, said));
                }
            }
            if let Some(why) = run
                .filter(|run| run.status == CiStatus::Failed)
                .and_then(|run| run.failure.as_deref())
            {
                ui.label(
                    RichText::new(format!("{}  {}", icon::WARNING, failure_words(why)))
                        .color(palette.danger),
                );
            }
            widgets::toolbar_separator(ui);
            let url = known.url.or_else(|| run.and_then(|run| run.url.clone()));
            if let Some(url) = url
                && widgets::secondary(
                    ui,
                    true,
                    &format!(
                        "{}  Open on {}",
                        icon::ARROW_SQUARE_OUT,
                        CiWords::of(view.kind).forge
                    ),
                )
                .clicked()
            {
                event = Some(JobEvent::Open(url));
            }
            let mut wrap = view.grid.wraps();
            if widgets::checkbox(ui, &mut wrap, "Wrap lines").changed() {
                view.grid.set_wrap(wrap);
            }
            // What comes and goes is last, in a `Ui` of its own (on a row of its own where it
            // doesn't fit): nothing after it moves.
            let widths = runs_width(ui, view, (seen, status));
            widgets::room_for(ui, &widths);
            ui.scope_builder(
                egui::UiBuilder::new()
                    .id(Id::new("job-log-runs"))
                    .layout(Layout::left_to_right(Align::Center)),
                |ui| {
                    if let Some(asked) = runs(ui, view, (seen, status)) {
                        event = Some(asked);
                    }
                },
            );
        });
    });
    event
}

/// What follows the bar's fixed part: the runs to switch between, what can be done to the run
/// shown, its sections, and a spinner while it is followed (or what was asked of the forge, while
/// it is being done).
fn runs(
    ui: &mut Ui,
    view: &mut JobView,
    (seen, status): (Option<&Seen>, CiStatus),
) -> Option<JobEvent> {
    if !view.job.earlier.is_empty() {
        runs_menu(ui, view);
    }
    let asked = artifacts(ui, view);
    run_actions(ui, view, status);
    if let Some(log) = seen.and_then(|seen| seen.log.as_ref())
        && has_outline(log)
    {
        let shown = outline_shown(ui.ctx());
        if widgets::tool(ui, icon::SIDEBAR_SIMPLE, "Sections", shown, Look::Full)
            .on_hover_text(if shown {
                "Hide the log's sections"
            } else {
                "Show the log's sections beside it"
            })
            .clicked()
        {
            ui.ctx().data_mut(|data| {
                data.insert_persisted(Id::new(OUTLINE_SHOWN), !shown);
            });
        }
    }
    if let Some(doing) = view.acting.doing() {
        widgets::working(ui, &format!("{doing}…"));
    } else if seen.is_some_and(|seen| !seen.done && seen.problem.is_none()) {
        ui.spinner();
    }
    asked
}

/// The latest run's artifacts (a GitLab job's), a menu of them; the one chosen.
fn artifacts(ui: &mut Ui, view: &JobView) -> Option<JobEvent> {
    if !view.is_latest() || view.job.artifacts.is_empty() {
        return None;
    }
    let mut chosen = None;
    widgets::tool_menu_of(
        ui,
        (icon::DOWNLOAD_SIMPLE, "Artifacts"),
        Look::Full,
        ARTIFACTS_TOPIC,
        |ui| {
            super::actions::menu_rows(ui);
            let items = view.job.artifacts.iter().map(|artifact| (artifact, None));
            chosen = artifact_items(ui, items);
        },
    );
    chosen.map(|(artifact, _)| {
        JobEvent::Download(Box::new(ArtifactWanted {
            repository: view.pipeline.repository(),
            artifact,
            of: view.job.name.clone(),
        }))
    })
}

/// The Runs menu: the job's latest run and those retried before it (the newest first), each
/// with how it ended, how long it took, and when; the one shown ticked; a click shows its log.
fn runs_menu(ui: &mut Ui, view: &mut JobView) {
    let latest = view.job.id;
    let all = all_runs(view);
    let place = run_place(view);
    let mut chosen = None;
    let palette = Palette::of(ui.ctx());
    widgets::tool_menu_of(
        ui,
        (icon::CLOCK_COUNTER_CLOCKWISE, &place),
        Look::Full,
        TOPIC,
        |ui| {
            super::actions::menu_rows(ui);
            let now = format::now();
            for (place, run) in all.iter().enumerate() {
                let when = run
                    .started
                    .and_then(|started| u64::try_from(started).ok())
                    .map(|started| format!(" · {}", format::ago(started, now)))
                    .unwrap_or_default();
                let took = run
                    .duration
                    .map(|took| format!(" in {}", duration(took)))
                    .unwrap_or_default();
                let which = if place == 0 && latest.is_some() {
                    "Latest"
                } else {
                    "Run"
                };
                let words = format!("{which} #{} · {}{took}{when}", run.id, run.status.word());
                let glyph = if run.id == view.run {
                    icon::CHECK
                } else {
                    ci_look(run.status, palette).0
                };
                if super::actions::item_of(ui, (glyph, &words), Ok(()), TOPIC) {
                    chosen = Some(run.id);
                }
            }
        },
    );
    if let Some(run) = chosen.filter(|run| *run != view.run) {
        let waker = ui.ctx().clone();
        view.show_run(run, &waker);
    }
}

/// The job's runs: its latest, then those retried before it, the newest first.
fn all_runs(view: &JobView) -> Vec<EarlierRun> {
    view.job
        .id
        .map(|id| EarlierRun {
            id,
            status: view.job.status,
            duration: view.job.duration,
            started: view.job.started,
            url: view.job.url.clone(),
        })
        .into_iter()
        .chain(view.job.earlier.iter().cloned())
        .collect()
}

/// Which of the job's runs shows, counted from its first: "Run 2 of 3".
fn run_place(view: &JobView) -> String {
    let all = all_runs(view);
    let count = all.len();
    let at = all
        .iter()
        .position(|run| run.id == view.run)
        .map_or(count, |at| count.saturating_sub(at));
    format!("Run {at} of {count}")
}

/// Retrying, cancelling, or starting the latest run, as its status (as last read) and the
/// account's role allow.
fn run_actions(ui: &mut Ui, view: &mut JobView, status: CiStatus) {
    let allowed = view.job.can_update && !view.acting.is_pending();
    let why_not = match (view.job.can_update, view.kind) {
        (true, _) => "The forge is being asked",
        (false, ForgeKind::GitLab) => {
            "Your role on this project may not run its pipelines (Developer and above may)"
        }
        (false, ForgeKind::GitHub) => "Re-running jobs takes write access to the repository",
    };
    let mut asked = None;
    for (action, words, does) in offered(view, status) {
        let button = widgets::secondary(ui, allowed, &words)
            .on_hover_text(does)
            .on_disabled_hover_text(why_not);
        if button.clicked() {
            asked = Some(action);
        }
    }
    if let Some(action) = asked {
        view.act(action);
    }
}

/// What can be done to the latest run as it stands (as last read; the forge's own say while that
/// is as the pipeline's page read it): each action, its button's words, and what it does.
fn offered(view: &JobView, status: CiStatus) -> Vec<(CiAction, String, &'static str)> {
    let Some(id) = view.job.id.filter(|_| view.is_latest()) else {
        return Vec::new();
    };
    // GitHub's say stands: a job is re-run once its whole run has ended, which its page knows.
    let (retry, cancel, play) = if status == view.job.status || view.kind == ForgeKind::GitHub {
        (
            view.job.actions.retry,
            view.job.actions.cancel,
            view.job.actions.play,
        )
    } else {
        (
            matches!(
                status,
                CiStatus::Failed | CiStatus::Canceled | CiStatus::Success
            ),
            status.is_moving() && status != CiStatus::Canceling,
            status == CiStatus::Manual,
        )
    };
    [
        (
            play,
            CiAction::PlayJob(id),
            format!("{}  Start the job", icon::PLAY),
            "Start this manual job",
        ),
        (
            retry,
            CiAction::RetryJob(id),
            format!(
                "{}  {}",
                icon::ARROW_CLOCKWISE,
                CiWords::of(view.kind).retry_job
            ),
            "Run it again: a new run in its place, this one kept among its runs",
        ),
        (
            cancel,
            CiAction::CancelJob(id),
            format!("{}  Cancel the job", icon::PROHIBIT),
            "Stop it where it is (or before it starts)",
        ),
    ]
    .into_iter()
    .filter(|(shown, ..)| *shown)
    .map(|(_, action, words, does)| (action, words, does))
    .collect()
}

/// How wide the bar's part that comes and goes draws: the runs' menu, the actions, the
/// sections' menu, and a spinner or what is being done.
fn runs_width(ui: &Ui, view: &mut JobView, (seen, status): (Option<&Seen>, CiStatus)) -> Vec<f32> {
    let mut widths = Vec::new();
    if view.is_latest() && !view.job.artifacts.is_empty() {
        widths.push(widgets::tool_width(
            ui,
            (icon::DOWNLOAD_SIMPLE, "Artifacts"),
            Look::Full,
            true,
        ));
    }
    if !view.job.earlier.is_empty() {
        let place = run_place(view);
        widths.push(widgets::tool_width(
            ui,
            (icon::CLOCK_COUNTER_CLOCKWISE, &place),
            Look::Full,
            true,
        ));
    }
    widths.extend(
        offered(view, status)
            .iter()
            .map(|(_, words, _)| widgets::button_width(ui, words)),
    );
    if seen
        .and_then(|seen| seen.log.as_ref())
        .is_some_and(|log| has_outline(log))
    {
        widths.push(widgets::tool_width(
            ui,
            (icon::SIDEBAR_SIMPLE, "Sections"),
            Look::Full,
            false,
        ));
    }
    // The spinner's room always: the bar keeps its rows as the job ends.
    if let Some(doing) = view.acting.doing() {
        widths.push(widgets::button_width(ui, &format!("{doing}…  ")));
    } else {
        widths.push(widgets::TARGET);
    }
    widths
}

/// Why a job failed, in words: GitLab's reasons said as a sentence, others as they come.
fn failure_words(why: &str) -> String {
    let said = match why {
        "script_failure" => "Its script failed",
        "stuck_or_timeout_failure" | "job_execution_timeout" => "It timed out",
        "runner_system_failure" => "The runner failed",
        "missing_dependency_failure" => "A job it needs left no artifacts (expired, or failed)",
        "runner_unsupported" => "The runner can't run it",
        "stale_schedule" => "Its schedule went by",
        "archived_failure" => "Its project is archived",
        "unmet_prerequisites" => "What it needs wasn't there",
        "scheduler_failure" => "GitLab couldn't schedule it",
        "data_integrity_failure" => "GitLab lost track of it",
        "api_failure" => "GitLab's API failed it",
        "no_matching_runner" => "No runner can take it",
        "trace_size_exceeded" => "Its log grew too large",
        "ci_quota_exceeded" => "Out of CI/CD minutes",
        "builds_disabled" => "CI/CD is off for the project",
        "user_blocked" => "Its user is blocked",
        "environment_creation_failure" => "Its environment couldn't be made",
        "deployment_rejected" => "Its deployment was rejected",
        "protected_environment_failure" => "It may not deploy to a protected environment",
        "failed_outdated_deployment_job" | "forward_deployment_failure" => {
            "A newer deployment went first"
        }
        "downstream_pipeline_creation_failed" => "The pipeline it starts couldn't be made",
        "ip_restriction_failure" => "Its runner's address isn't allowed",
        "unknown_failure" => "It failed",
        other => return capitalized(&other.replace('_', " ")),
    };
    said.to_owned()
}

/// `text` with its first letter capitalized.
fn capitalized(text: &str) -> String {
    let mut letters = text.chars();
    letters.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(letters).collect()
    })
}

/// Whether a log has an outline: sections, or lines marked as errors or warnings.
const fn has_outline(log: &LogSource) -> bool {
    !log.sections.is_empty() || !log.marks.is_empty()
}

/// Whether the outline shows (for every job's log, kept).
fn outline_shown(ctx: &egui::Context) -> bool {
    ctx.data_mut(|data| *data.get_persisted_mut_or(Id::new(OUTLINE_SHOWN), true))
}

/// How long the run ran, or has run so far.
fn ran(run: Option<&JobRun>, job: &EarlierRun, status: CiStatus) -> Option<String> {
    if let Some(seconds) = run
        .and_then(|run| run.duration)
        .filter(|_| !status.is_moving())
    {
        let seconds = format!("{seconds:.0}").parse::<u64>().unwrap_or(0);
        return Some(duration(seconds));
    }
    let started = run.and_then(|run| run.started).or(job.started)?;
    let started = u64::try_from(started).ok()?;
    let now = format::now();
    if status.is_moving() {
        return Some(format!("{} so far", duration(now.saturating_sub(started))));
    }
    job.duration.map(duration)
}

/// The log and its outline: what is shown of it as folds change (its top kept in place), the
/// end followed while the view rests there, a click on a section's header folding it.
fn log_view(
    ui: &mut Ui,
    view: &mut JobView,
    (log, version): (&Arc<LogSource>, u64),
    status: CiStatus,
) {
    let (shown, changed) = view.folding.shown(log, version, ui.ctx());
    if changed
        && !view.follow
        && let Some(before) = &view.shown
    {
        let top = before.whole_line(view.grid.top().line);
        view.grid.scroll_to_line(shown.shown_line(top));
    }
    // New lines while at the end (or the first): the end shows.
    if version != view.drawn || changed {
        if view.follow {
            view.grid.scroll_to_end();
        }
        view.drawn = version;
    }
    // A section or line asked for, once what shows has it unfolded.
    if let Some(line) = view.going
        && view.folding.is_settled(version)
    {
        view.going = None;
        view.grid.scroll_to_line(shown.shown_line(line));
        view.follow = false;
    }
    let whole = shown.whole();
    let now = i64::try_from(format::now()).unwrap_or(i64::MAX);
    let lit = whole.section_at(shown.whole_line(view.grid.top().line));
    let mut asked = if outline_shown(ui.ctx()) && has_outline(whole) {
        let palette = Palette::of(ui.ctx());
        egui::Panel::left(Id::new("job-log-outline"))
            .resizable(true)
            .default_size(260.0)
            .size_range(200.0..=480.0)
            .frame(
                Frame::new()
                    .fill(palette.panel)
                    .inner_margin(egui::Margin::symmetric(6, 0)),
            )
            .show(ui, |ui| {
                super::log_outline::show(
                    ui,
                    (whole, &view.folding.folds),
                    (lit, Standing { status, now }),
                    &mut view.outline,
                )
            })
            .inner
    } else {
        None
    };
    let response = view.grid.show(ui, shown.source());
    view.follow = view.grid.is_at_end();
    // A click on a section's header folds or unfolds it; the pointer says it can.
    let header_at = |pointer: egui::Pos2| {
        view.grid
            .line_at(pointer)
            .map(|line| shown.whole_line(line))
            .and_then(|line| whole.header_place(line))
            .filter(|place| {
                whole
                    .sections
                    .get(*place)
                    .is_some_and(|section| whole.folds(section))
            })
    };
    if let Some(pointer) = response.hover_pos()
        && header_at(pointer).is_some()
    {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if response.clicked()
        && let Some(place) = response.interact_pointer_pos().and_then(header_at)
    {
        asked = Some(OutlineAsk::Fold(place));
    }
    // Alt+↑ and Alt+↓: the section before or after the top.
    let step = ui.input_mut(|input| {
        if input.consume_key(Modifiers::ALT, Key::ArrowDown) {
            Some(true)
        } else if input.consume_key(Modifiers::ALT, Key::ArrowUp) {
            Some(false)
        } else {
            None
        }
    });
    if let Some(down) = step
        && let Some(line) = neighbour(&shown, shown.whole_line(view.grid.top().line), down)
    {
        asked = Some(OutlineAsk::Go(line));
    }
    log_menu(&response, view, &shown);
    follow_button(ui, view, response.rect, status.is_moving());
    if let Some(asked) = asked {
        carry_out(view, whole, asked);
    }
    view.shown = Some(shown);
}

/// The log's menu, on a right click: what is selected copied (the line under the pointer
/// selected first, where the click isn't on a selection), everything shown selected, or the
/// whole log copied, every section unfolded.
fn log_menu(response: &egui::Response, view: &mut JobView, shown: &Shown) {
    if response.secondary_clicked()
        && let Some(pointer) = response.interact_pointer_pos()
    {
        let on_selection = view
            .grid
            .selected_lines()
            .zip(view.grid.line_at(pointer))
            .is_some_and(|((_, lines), line)| lines.contains(&line));
        if !on_selection {
            view.grid.select_line_at(pointer);
        }
    }
    let source = shown.source();
    let grid = &mut view.grid;
    response.context_menu(|ui| {
        use super::actions::item;
        let selected = grid.selected_text(&[source]);
        let allowed = if selected.is_some() {
            Ok(())
        } else {
            Err("Nothing is selected")
        };
        if item(ui, (icon::COPY, "Copy"), allowed)
            && let Some(text) = selected
        {
            ui.ctx().copy_text(text);
        }
        if item(ui, (icon::SELECTION_ALL, "Select all"), Ok(())) {
            grid.select_all(0, source.measure());
        }
        ui.separator();
        if item(ui, (icon::COPY_SIMPLE, "Copy the whole log"), Ok(())) {
            let whole = crate::grid::GridSource::text(shown.whole()).bytes();
            ui.ctx()
                .copy_text(String::from_utf8_lossy(whole).into_owned());
        }
    });
}

/// The header of the section shown before (or after: `down`) whole line `top`, the first
/// shown one.
fn neighbour(shown: &Shown, top: u32, down: bool) -> Option<u32> {
    let whole = shown.whole();
    let visible = |section: &&egit_forge::Section| {
        shown.whole_line(shown.shown_line(section.line)) == section.line
    };
    if down {
        whole
            .sections
            .iter()
            .filter(|section| section.line > top)
            .find(visible)
            .map(|section| section.line)
    } else {
        whole
            .sections
            .iter()
            .rev()
            .filter(|section| section.line < top)
            .find(visible)
            .map(|section| section.line)
    }
}

/// Does what the outline (or a header's click, or a key) asked.
fn carry_out(view: &mut JobView, whole: &LogSource, asked: OutlineAsk) {
    match asked {
        OutlineAsk::Fold(place) => {
            if let Some(section) = whole.sections.get(place) {
                view.folding.folds.toggle(whole, section);
            }
        }
        OutlineAsk::Every(folded) => view.folding.folds.every(whole, folded),
        OutlineAsk::Go(line) => {
            view.folding.folds.reveal(whole, line);
            view.going = Some(line);
        }
    }
}

/// While the view isn't at the log's end, a button at the log's bottom right going there (and
/// following it, while the job runs).
fn follow_button(ui: &Ui, view: &mut JobView, log: egui::Rect, running: bool) {
    if view.follow {
        return;
    }
    let words = if running {
        format!("{}  Follow the end", icon::ARROW_DOWN)
    } else {
        format!("{}  Go to the end", icon::ARROW_DOWN)
    };
    let width = widgets::button_width(ui, &words);
    let at = egui::pos2(
        log.right() - width - 28.0,
        log.bottom() - widgets::TARGET - 20.0,
    );
    let clicked = egui::Area::new(Id::new("job-log-follow"))
        .order(egui::Order::Middle)
        .fixed_pos(at)
        .show(ui.ctx(), |ui| {
            widgets::secondary(ui, true, &words).clicked()
        })
        .inner;
    if clicked {
        view.grid.scroll_to_end();
        view.follow = true;
    }
}

/// What shows before the job has a log: why it has none yet, by how it stands; a GitHub job's
/// steps as they go (GitHub gives its log once it has ended).
fn waiting(ui: &mut Ui, seen: &Seen, known: CiStatus) {
    let status = seen.run.as_ref().map_or(known, |run| run.status);
    if !seen.missing && seen.run.is_none() {
        widgets::working(ui, "Reading the job's log…");
        return;
    }
    if let Some(run) = seen.run.as_ref().filter(|run| !run.steps.is_empty()) {
        if status.is_moving() {
            // The bar's spinner says it is followed: one is enough.
            ui.label(muted(
                ui,
                "Running: GitHub gives its log once it has ended.",
            ));
        } else if !seen.done {
            widgets::working(ui, "Its log is on its way…");
        } else {
            ui.label(muted(ui, "GitHub has no log of it (it may have expired)."));
        }
        ui.add_space(8.0);
        steps(ui, &run.steps);
        return;
    }
    let words = match status {
        CiStatus::Created
        | CiStatus::Pending
        | CiStatus::Preparing
        | CiStatus::WaitingForResource
        | CiStatus::WaitingForCallback => {
            widgets::working(ui, "Waiting for a runner to take it up…");
            return;
        }
        CiStatus::Manual => "A manual job: it runs once someone starts it.",
        CiStatus::Scheduled => "Scheduled: it starts at its time.",
        CiStatus::Skipped => "Skipped: it never ran.",
        CiStatus::Canceled | CiStatus::Canceling => "Cancelled before it wrote anything.",
        CiStatus::Running => {
            widgets::working(ui, "Running: its log comes as it writes it…");
            return;
        }
        _ => "This job has no log (it may have been erased).",
    };
    ui.label(muted(ui, words));
}

/// A GitHub job's steps: each one's status, number, name, and how long it took (or has, so far).
fn steps(ui: &mut Ui, steps: &[JobStep]) {
    let palette = Palette::of(ui.ctx());
    let now = format::now();
    Frame::new()
        .fill(palette.surface)
        .stroke(egui::Stroke::new(1.0, palette.border))
        .corner_radius(egui::CornerRadius::same(crate::theme::RADIUS_BOX))
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            crate::help::note_topic(ui, ui.max_rect(), TOPIC);
            // Numbers and times in columns of one width: they line up, and a time growing
            // moves nothing.
            let number = widgets::text_width(ui, "99");
            let took_width = widgets::duration_width(ui, &egui::TextStyle::Body)
                + widgets::text_width(ui, " so far");
            let running = steps.iter().any(|step| step.status.is_moving());
            if running {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_secs(1));
            }
            for step in steps {
                let (glyph, color) = ci_look(step.status, palette);
                let took = match (step.started, step.finished) {
                    (Some(started), Some(finished)) => {
                        duration(u64::try_from(finished.saturating_sub(started)).unwrap_or(0))
                    }
                    (Some(started), None) if step.status.is_moving() => format!(
                        "{} so far",
                        duration(now.saturating_sub(u64::try_from(started).unwrap_or(now)))
                    ),
                    _ => String::new(),
                };
                ui.horizontal(|ui| {
                    ui.set_min_height(crate::widgets::TARGET);
                    ui.label(RichText::new(glyph).color(color));
                    widgets::column(ui, (number, widgets::TARGET), Align::Max, |ui| {
                        ui.label(muted(ui, step.number.to_string()));
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        widgets::column(ui, (took_width, widgets::TARGET), Align::Max, |ui| {
                            ui.label(muted(ui, took));
                        });
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            ui.add(egui::Label::new(&step.name).truncate());
                        });
                    });
                });
            }
        });
}
