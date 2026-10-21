//! A GitLab job's log in the pipeline page's place (a job's click there): a bar (back to the
//! pipeline, the job's status and how long it has run, why it failed, its sections to jump to,
//! wrapping, its page on GitLab), then the log in the text grid as GitLab shows it (colours,
//! each line's time beside its number where the runner wrote them, sections' headers marked),
//! read again every few seconds while the job runs, the view following its end while it rests
//! there (scrolled up, it stays; back at the end, it follows again). Escape, or the back
//! button, goes back to the pipeline.

use egit_forge::{CiStatus, Job, JobRun};
use egui::{Frame, Key, Modifiers, RichText, Ui};

use super::history::Shared;
use super::pipeline::{PipelineView, ci_look, ci_words, duration};
use crate::forge::RepoForge;
use crate::format;
use crate::grid::TextGrid;
use crate::job_log::{Following, JobFollower, LogSource};
use crate::problem::Problem;
use crate::theme::Weighted as _;
use crate::theme::{Palette, icon};
use crate::widgets::{self, Look, muted};

/// The help topic explaining it.
const TOPIC: &str = "job-log";

/// The most sections the Sections menu lists.
const MOST_SECTIONS: usize = 500;

/// A job's log, shown.
#[derive(Debug)]
pub(super) struct JobView {
    /// The pipeline's page, gone back to.
    pub(super) pipeline: Box<PipelineView>,
    /// The job, as the pipeline's page had it.
    job: Job,
    /// Its log followed (none where its project or account is gone).
    follower: Option<JobFollower>,
    /// Why it can't be followed, when it can't.
    problem: Option<Problem>,
    grid: TextGrid,
    /// The log's version drawn last (0: none yet).
    drawn: u64,
    /// Whether the view follows the log's end (it rested there).
    follow: bool,
}

/// What the view asks of the history view.
pub(super) enum JobEvent {
    /// Back to the pipeline.
    Back,
    /// Open a page in the browser.
    Open(String),
}

impl JobView {
    /// `job`'s log, of the pipeline `pipeline` shows, followed through `forge`'s account for
    /// its project (waking `waker` as it lands).
    pub(super) fn new(
        pipeline: Box<PipelineView>,
        job: Job,
        forge: &RepoForge,
        waker: &egui::Context,
    ) -> Self {
        let repository = forge.repositories.get(pipeline.repository()).cloned();
        let account = repository
            .as_ref()
            .and_then(|repository| forge.account(repository.host()));
        let (follower, problem) = match (repository, account, job.id) {
            (Some(repository), Some(account), Some(id)) => (
                Some(JobFollower::start((account, repository), id, waker.clone())),
                None,
            ),
            _ => (
                None,
                Some(Problem::new(
                    "The job's log can't be read",
                    "Its project is no longer among the remotes.",
                )),
            ),
        };
        Self {
            pipeline,
            job,
            follower,
            problem,
            grid: TextGrid::new(egui::Id::new(("job-log", 0_u8))),
            drawn: 0,
            follow: true,
        }
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
    let palette = shared.palette;
    let mut event = bar(ui, palette, view, seen.as_ref());
    egui::CentralPanel::default()
        .frame(Frame::new())
        .show(ui, |ui| {
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
                                "Only its first {} show here: the rest is on GitLab.",
                                format::size(
                                    u64::try_from(egit_forge::MOST_TEXT).unwrap_or(u64::MAX)
                                )
                            ),
                        ));
                    });
                }
                // New lines while at the end (or the first): the end shows.
                if seen.version != view.drawn {
                    if view.follow {
                        view.grid.scroll_to_end();
                    }
                    view.drawn = seen.version;
                }
                view.grid.show(ui, &**log);
                view.follow = view.grid.is_at_end();
            } else {
                padded(ui, |ui| waiting(ui, &seen, &view.job));
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
        .inner_margin(egui::Margin::symmetric(16, 12))
        .show(ui, add);
}

/// The bar: back to the pipeline, the job (status, name, how long it has run, why it failed),
/// then its sections, wrapping, and its page on GitLab, wrapping onto a row of their own where
/// the bar is narrow.
fn bar(ui: &mut Ui, palette: Palette, view: &mut JobView, seen: Option<&Seen>) -> Option<JobEvent> {
    let run = seen.and_then(|seen| seen.run.as_ref());
    let status = run.map_or(view.job.status, |run| run.status);
    let mut event = None;
    widgets::bar(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            let back = format!("{}  Pipeline #{}", icon::ARROW_LEFT, view.pipeline.number());
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
            let mut said = ci_words(status, false);
            if let Some(took) = ran(run, &view.job, status) {
                said.push_str(" · ");
                said.push_str(&took);
            }
            ui.label(muted(ui, said));
            if let Some(why) = run
                .filter(|run| run.status == CiStatus::Failed)
                .and_then(|run| run.failure.as_deref())
            {
                ui.label(
                    RichText::new(format!("{}  {}", icon::WARNING, why.replace('_', " ")))
                        .color(palette.danger),
                );
            }
            widgets::toolbar_separator(ui);
            let url = view
                .job
                .url
                .clone()
                .or_else(|| run.and_then(|run| run.url.clone()));
            if let Some(url) = url
                && widgets::secondary(
                    ui,
                    true,
                    &format!("{}  Open on GitLab", icon::ARROW_SQUARE_OUT),
                )
                .clicked()
            {
                event = Some(JobEvent::Open(url));
            }
            let mut wrap = view.grid.wraps();
            if widgets::checkbox(ui, &mut wrap, "Wrap lines").changed() {
                view.grid.set_wrap(wrap);
            }
            // What comes and goes is last: nothing after it moves.
            if let Some(log) = seen.and_then(|seen| seen.log.as_ref())
                && !log.sections.is_empty()
            {
                sections(ui, view, log);
            }
            if seen.is_some_and(|seen| !seen.done && seen.problem.is_none()) {
                ui.spinner();
            }
        });
    });
    event
}

/// How long the job ran, or has run so far.
fn ran(run: Option<&JobRun>, job: &Job, status: CiStatus) -> Option<String> {
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

/// The Sections menu: each section (stepped in under those it is inside) and how long it took,
/// a click scrolling to its header.
fn sections(ui: &mut Ui, view: &mut JobView, log: &LogSource) {
    let mut chosen = None;
    widgets::tool_menu_of(
        ui,
        (icon::LIST_BULLETS, "Sections"),
        Look::Full,
        TOPIC,
        |ui| {
            super::actions::menu_rows(ui);
            egui::ScrollArea::vertical()
                .max_height(420.0)
                .show(ui, |ui| {
                    for section in log.sections.iter().take(MOST_SECTIONS) {
                        let took = section.ended.map_or_else(
                            || "under way".to_owned(),
                            |(ended, _)| {
                                duration(
                                    u64::try_from(ended.saturating_sub(section.started))
                                        .unwrap_or(0),
                                )
                            },
                        );
                        let indent = "    ".repeat(usize::from(section.depth));
                        let words = format!("{indent}{}  ·  {took}", section.name);
                        if super::actions::item_of(ui, (icon::CARET_RIGHT, &words), Ok(()), TOPIC) {
                            chosen = Some(section.line);
                        }
                    }
                });
        },
    );
    if let Some(line) = chosen {
        view.grid.scroll_to_line(line);
        view.follow = false;
    }
}

/// What shows before the job has a log: why it has none yet, by how it stands.
fn waiting(ui: &mut Ui, seen: &Seen, job: &Job) {
    let status = seen.run.as_ref().map_or(job.status, |run| run.status);
    if !seen.missing && seen.run.is_none() {
        widgets::working(ui, "Reading the job's log…");
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
        CiStatus::Manual => "A manual job: it runs once someone starts it on GitLab.",
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
