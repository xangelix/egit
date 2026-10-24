//! A CI/CD pipeline in the commit table's place, from the mark on its commit's row: a GitLab
//! pipeline, or a GitHub Actions workflow's run, alike. How it stands (its status, branch or tag,
//! commit, who started it and how, when, for how long, which attempt), the commit's other runs
//! (another GitHub workflow's) a menu away, what can be done to it as the forge allows the
//! account, in the forge's own words ([`CiWords`]: GitLab's failed jobs retried, GitHub's
//! re-run, or all of them; the pipeline cancelled once asked), then its jobs laid out
//! (`JobsLayout`): a card per stage listing its jobs ([`list`]), or a graph of them by stage
//! (GitLab's, whose stages run in order) or by what each waits for ([`graph`]; a GitHub run's
//! needs read from its workflow's file at its commit). A job's click shows its log
//! (`super::job`), its menu its earlier runs' logs, retrying, cancelling, or starting it, and
//! its page. Read again every few seconds while it may still change by itself, and at once
//! after anything asked of the forge.
//!
//! Where the pipelines couldn't be read, a notice under the commit table says why ([`unread`]).
//!
//! Every status shows as one of five, by colour and by icon alike (for those who don't tell
//! the colours apart): passed green, failed red, under way blue, waiting for someone (or for its
//! time) orange, cancelled or skipped grey.

use std::sync::Arc;
use std::time::{Duration, Instant};

use egit_forge::{
    Artifact, ArtifactKind, CiAction, CiGroup, CiStatus, ForgeKind, Job, PipelineDetails, Workflow,
    ci_act, pipeline_details,
};
use egit_git::{CancelToken, Git, ObjectId};
use egui::{Align, Color32, Frame, Id, Key, Layout, Margin, Modifiers, RichText, Sense, Ui};
use egui_async::Bind;

mod graph;
mod list;

use super::history::Shared;
use crate::ci::CiMark;
use crate::downloads::ArtifactWanted;
use crate::forge::RepoForge;
use crate::format;
use crate::jobs;
use crate::problem::Problem;
use crate::theme::Weighted as _;
use crate::theme::{Palette, icon};
use crate::widgets::{self, muted};

/// How soon a pipeline that may still change is read again.
const READ_AGAIN: Duration = Duration::from_secs(5);

/// The help topic artifacts are explained by.
pub(super) const ARTIFACTS_TOPIC: &str = "pipeline-artifacts";

/// The help topic the page's parts are explained by.
const TOPIC: &str = "pipeline-page";

/// A status's icon and colour: its group's colour, its own icon.
pub(super) const fn ci_look(status: CiStatus, palette: Palette) -> (&'static str, Color32) {
    let color = match status.group() {
        CiGroup::Success => palette.ok,
        CiGroup::Failed => palette.danger,
        CiGroup::Running => palette.running,
        CiGroup::Paused => palette.paused,
        CiGroup::Canceled => palette.muted,
    };
    let glyph = match status {
        CiStatus::Success => icon::CHECK_CIRCLE,
        CiStatus::Failed => icon::X_CIRCLE,
        CiStatus::Running => icon::CIRCLE_HALF,
        CiStatus::Created
        | CiStatus::WaitingForResource
        | CiStatus::Preparing
        | CiStatus::WaitingForCallback
        | CiStatus::Pending => icon::CLOCK,
        CiStatus::Manual => icon::PAUSE_CIRCLE,
        CiStatus::Scheduled => icon::CLOCK_COUNTDOWN,
        CiStatus::Canceling | CiStatus::Canceled => icon::PROHIBIT,
        CiStatus::Skipped => icon::SKIP_FORWARD_CIRCLE,
        CiStatus::Unknown => icon::MINUS_CIRCLE,
    };
    (glyph, color)
}

/// A pipeline's status in words: "passed", "passed with warnings", "waiting for someone".
pub(super) fn ci_words(status: CiStatus, warnings: bool) -> String {
    match status {
        CiStatus::Success if warnings => "passed with warnings".to_owned(),
        CiStatus::Manual => "waiting for someone to start a job".to_owned(),
        CiStatus::Scheduled => "waiting for a delayed job's time".to_owned(),
        other => other.word().to_owned(),
    }
}

/// What a forge calls what is done to its pipelines and jobs, on buttons and in menus: GitLab's
/// words, or GitHub's.
#[derive(Clone, Copy, Debug)]
pub(super) struct CiWords {
    /// The forge's name ("Open on GitHub").
    pub(super) forge: &'static str,
    /// What its pipelines are, in the singular: "pipeline", "workflow run".
    pub(super) noun: &'static str,
    /// Running a pipeline's failed jobs again: "Retry failed jobs", "Re-run failed jobs".
    pub(super) retry_failed: &'static str,
    /// Running every job again (GitHub's): "Re-run all jobs".
    pub(super) rerun_all: &'static str,
    /// Cancelling the pipeline: "Cancel the pipeline", "Cancel workflow".
    pub(super) cancel: &'static str,
    /// Asking first: "Cancel the pipeline?", "Cancel the workflow run?".
    pub(super) cancel_asked: &'static str,
    /// A job's retry: "Retry the job", "Re-run this job".
    pub(super) retry_job: &'static str,
    /// Before a job's name, on its row's button: "Retry", "Re-run".
    pub(super) retry_verb: &'static str,
}

impl CiWords {
    /// `kind`'s.
    pub(super) const fn of(kind: ForgeKind) -> Self {
        match kind {
            ForgeKind::GitLab => Self {
                forge: "GitLab",
                noun: "pipeline",
                retry_failed: "Retry failed jobs",
                rerun_all: "Retry failed jobs",
                cancel: "Cancel the pipeline",
                cancel_asked: "Cancel the pipeline?",
                retry_job: "Retry the job",
                retry_verb: "Retry",
            },
            ForgeKind::GitHub => Self {
                forge: "GitHub",
                noun: "workflow run",
                retry_failed: "Re-run failed jobs",
                rerun_all: "Re-run all jobs",
                cancel: "Cancel workflow",
                cancel_asked: "Cancel the workflow run?",
                retry_job: "Re-run this job",
                retry_verb: "Re-run",
            },
        }
    }
}

/// What the commit table's mark asked.
pub(super) enum MarkEvent {
    /// The pipeline's page, in the commit table's place.
    Show(CiMark),
    /// Its page on the forge.
    Open(String),
}

/// The width a pipeline's mark takes in a commit's row (room kept on every row while any has
/// one, so the commits' names line up).
pub(super) const MARK_WIDTH: f32 = 24.0;

/// A commit's pipeline as a mark in its row: an icon in its status's colour (its most telling
/// pipeline's), each pipeline's title and status on hover; a click shows the pipeline, its menu
/// opens it on its forge. Room is kept where the commit has none.
pub(super) fn mark(
    ui: &mut Ui,
    forge: &RepoForge,
    commit: ObjectId,
    palette: Palette,
) -> Option<MarkEvent> {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(MARK_WIDTH, MARK_WIDTH), Sense::hover());
    let mark = forge.pipeline_of(&commit)?;
    let pipeline = &mark.pipeline;
    let words = CiWords::of(forge.kind_of(mark.repository));
    let response = ui.interact(rect, Id::new(("pipeline-mark", commit)), Sense::click());
    let said = format!(
        "{}: {}",
        pipeline.title(),
        ci_words(pipeline.status, pipeline.warnings)
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &said));
    crate::help::note_topic(ui, rect, "pipeline-mark");
    if response.hovered() || response.has_focus() {
        ui.painter().rect_filled(
            rect,
            crate::theme::RADIUS_INNER,
            ui.style().interact(&response).weak_bg_fill,
        );
    }
    let (glyph, color) = ci_look(pipeline.status, palette);
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        glyph,
        egui::FontId::proportional(15.0),
        color,
    );
    let mut event = None;
    let response = response.on_hover_ui(|ui| {
        ui.label(&said);
        if let Some(git_ref) = &pipeline.git_ref {
            ui.label(muted(ui, git_ref));
        }
        // The commit's other runs (another GitHub workflow's), each as it stands.
        for (_, other) in mark.others.iter().take(8) {
            let (glyph, color) = ci_look(other.status, palette);
            ui.label(
                RichText::new(format!(
                    "{glyph}  {}: {}",
                    other.title(),
                    ci_words(other.status, other.warnings)
                ))
                .color(color),
            );
        }
        if mark.others.len() > 8 {
            ui.label(muted(
                ui,
                format!("and {} more", mark.others.len().saturating_sub(8)),
            ));
        }
        ui.label(muted(ui, "Click to see its jobs"));
    });
    if response.clicked() {
        event = Some(MarkEvent::Show(mark.clone()));
    }
    response.context_menu(|ui| {
        super::actions::menu_rows(ui);
        if super::actions::item(ui, (icon::FLOW_ARROW, "Show the pipeline"), Ok(())) {
            event = Some(MarkEvent::Show(mark.clone()));
        }
        let open = format!("Open on {}", words.forge);
        if super::actions::item(ui, (icon::ARROW_SQUARE_OUT, &open), Ok(())) {
            event = Some(MarkEvent::Open(pipeline.url.clone()));
        }
    });
    event
}

/// How the page lays a pipeline's jobs out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) enum JobsLayout {
    /// Each stage a card of its jobs, in the order they run.
    #[default]
    List,
    /// Each stage a column of its jobs, lines drawn to what each needs.
    Stages,
    /// Columns by what each job waits for (its `needs`, else the stages before it), as
    /// GitLab's "Job dependencies" view lays them out.
    Dependencies,
}

impl JobsLayout {
    /// The one chosen last (kept with egui's memory, for every pipeline).
    fn chosen(ctx: &egui::Context) -> Self {
        ctx.data_mut(|data| data.get_persisted(Id::new("pipeline-layout")))
            .unwrap_or_default()
    }

    fn choose(self, ctx: &egui::Context) {
        ctx.data_mut(|data| data.insert_persisted(Id::new("pipeline-layout"), self));
    }
}

/// What a job's row, card, or menu asked.
pub(super) enum JobAsk {
    /// Its log: of its latest run, or of an earlier one (its number).
    Log(Job, u64),
    /// Something of GitLab: run it again, cancel it, start it.
    Act(CiAction),
    /// Its page on GitLab.
    Open(String),
    /// One of its artifacts downloaded (a GitLab job's), and the job's name.
    Download(Artifact, String),
}

/// The pipeline page's state.
#[derive(Debug)]
pub(super) struct PipelineView {
    /// The pipeline, as its mark had it (shown until its details land), and its repository.
    mark: CiMark,
    details: Bind<Arc<PipelineDetails>, Problem>,
    /// When it is read again (while it may still change).
    next: Option<Instant>,
    /// Whether a read is due at once (the first, Refresh, or after an action).
    due: bool,
    /// Whether the last read's end was taken in (when the next is due worked out).
    landed: bool,
    cancel: CancelToken,
    /// What was last asked of GitLab, and how it went.
    acting: Acting,
    /// Whether cancelling the pipeline is asked first.
    confirming: bool,
    /// The jobs' graph, as last laid out, and what the pointer lights in it.
    graph: graph::Graph,
    /// A GitHub run's workflow, read from its file at the run's commit: the file and what it
    /// says, kept for the reads after.
    workflow: KeptWorkflow,
}

/// A GitHub run's workflow, once read: its file, and what it says.
type KeptWorkflow = Arc<parking_lot::Mutex<Option<(String, Arc<Workflow>)>>>;

impl Drop for PipelineView {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// What the page asks of the history view.
pub(super) enum PipelineEvent {
    Close,
    ShowCommit(ObjectId),
    Open(String),
    /// A job's log: of its latest run, or of an earlier one (its number).
    Job(Box<Job>, u64),
    /// The forge did what was asked: the commits' marks read again.
    Acted,
    /// Another of the commit's pipelines (another GitHub workflow's run), in this one's place.
    Other(Box<CiMark>),
    /// An artifact saved where the user says.
    Download(Box<ArtifactWanted>),
}

impl PipelineView {
    /// The page of `mark`'s pipeline, read at once.
    pub(super) fn new(mark: CiMark) -> Self {
        Self {
            mark,
            details: Bind::new(true),
            next: None,
            due: true,
            landed: false,
            cancel: CancelToken::new(),
            acting: Acting::new(),
            confirming: false,
            graph: graph::Graph::default(),
            workflow: Arc::default(),
        }
    }

    /// What its page is titled ("Pipeline #123456", "CI #57").
    pub(super) fn title(&self) -> String {
        self.mark.pipeline.title()
    }

    /// Its repository's place among the forge's.
    pub(super) const fn repository(&self) -> usize {
        self.mark.repository
    }

    /// Reads it again at once (back from a job's log, say).
    pub(super) const fn read_again(&mut self) {
        self.due = true;
    }

    /// Reads it (again) when due; schedules the next read while it may still change. Returns
    /// whether an action asked of the forge went through, just now.
    fn follow(&mut self, ui: &Ui, shared: &Shared<'_>) -> bool {
        let acted = self.acting.landed().is_some();
        self.due = self.due || acted;
        let now = Instant::now();
        if self.details.is_pending() {
            return acted;
        }
        if !self.landed
            && let Some(read) = self.details.read_as_ref()
        {
            self.landed = true;
            // One that failed is tried again in a while: a network gone may come back.
            let moving = read.map_or(true, |details| {
                details.pipeline.status.is_moving()
                    || details
                        .stages
                        .iter()
                        .flat_map(|stage| &stage.jobs)
                        .any(|job| job.status.is_moving())
            });
            self.next = moving.then(|| now.checked_add(READ_AGAIN)).flatten();
        }
        let due = std::mem::take(&mut self.due) || self.next.is_some_and(|next| now >= next);
        if due {
            self.next = None;
            self.read(shared);
        } else if let Some(next) = self.next {
            ui.ctx()
                .request_repaint_after(next.saturating_duration_since(now));
        }
        acted
    }

    /// Its repository and the account it is read with, where they are still there.
    pub(super) fn reached(&self, forge: &RepoForge) -> Option<Reach> {
        let repository = forge.repositories.get(self.mark.repository).cloned()?;
        let account = forge.account(repository.host())?;
        Some((account, repository))
    }

    /// Starts reading it: its details, and a GitHub run's workflow's file at its commit (read
    /// once, then kept), laid over them.
    fn read(&mut self, shared: &Shared<'_>) {
        self.landed = false;
        let Some((account, repository)) = self.reached(shared.forge) else {
            self.details.fill(Err(Problem::new(
                "The pipeline couldn't be read",
                "Its project is no longer among the remotes.",
            )));
            return;
        };
        self.cancel.cancel();
        self.cancel = CancelToken::new();
        let cancel = self.cancel.clone();
        let iid = self.mark.pipeline.iid;
        let commit = self.mark.pipeline.commit;
        let git = shared.git.cloned();
        let work_tree = shared.repository.work_tree().to_path_buf();
        let workflow = Arc::clone(&self.workflow);
        self.details.request(jobs::blocking(move || {
            let mut details =
                pipeline_details(&account, &repository, iid, &cancel).map_err(|error| {
                    Problem::new("The pipeline couldn't be read", error.to_string())
                })?;
            if let (Some(path), Some(git)) = (details.workflow.clone(), git.as_ref()) {
                let read = workflow_at(git, (&work_tree, commit), &path, &workflow);
                details.with_workflow(&read);
            }
            Ok(Arc::new(details))
        }));
    }

    /// Asks `action` of the forge (in a job); the pipeline is read again once it went through.
    fn act(&mut self, forge: &RepoForge, action: CiAction) {
        self.confirming = false;
        let reach = self.reached(forge);
        self.acting.ask(reach, action);
    }
}

/// A GitLab project and the account it is reached with.
pub(super) type Reach = (Arc<egit_forge::Account>, egit_forge::ForgeRepository);

/// What was last asked of GitLab (retrying, cancelling, starting), and how it went: the
/// pipeline's page's and a job's log's alike.
#[derive(Debug)]
pub(super) struct Acting {
    action: Option<CiAction>,
    /// GitLab's answer: the run a job's retry made, or the job asked of.
    answer: Bind<Option<u64>, Problem>,
    /// Whether the answer was taken in.
    taken: bool,
}

impl Acting {
    /// Nothing asked yet.
    pub(super) const fn new() -> Self {
        Self {
            action: None,
            answer: Bind::new(true),
            taken: true,
        }
    }

    /// Asks `action` of the project `reach` names, in a job (or says it can't be, none).
    pub(super) fn ask(&mut self, reach: Option<Reach>, action: CiAction) {
        self.action = Some(action);
        self.taken = false;
        let Some((account, repository)) = reach else {
            self.answer.fill(Err(Problem::new(
                "GitLab couldn't be asked",
                "The project is no longer among the remotes.",
            )));
            return;
        };
        self.answer.request(jobs::blocking(move || {
            ci_act(&account, &repository, action, &CancelToken::new())
                .map_err(|error| refusal(action, &error))
        }));
    }

    /// Whether GitLab is being asked.
    pub(super) fn is_pending(&mut self) -> bool {
        self.answer.is_pending()
    }

    /// What is being done, while it is: "Retrying the job".
    pub(super) fn doing(&mut self) -> Option<&'static str> {
        let action = self.action?;
        self.is_pending().then(|| doing(action))
    }

    /// Once, as GitLab's answer lands, when it did what was asked: the action, and the run a
    /// job's retry made.
    pub(super) fn landed(&mut self) -> Option<(CiAction, Option<u64>)> {
        if self.taken || !self.answer.is_finished() {
            return None;
        }
        self.taken = true;
        let made = *self.answer.read_as_ref()?.ok()?;
        Some((self.action?, made))
    }

    /// Whether GitLab didn't do what was last asked (and that wasn't dismissed).
    pub(super) fn failed(&mut self) -> bool {
        matches!(self.answer.read_as_ref(), Some(Err(_)))
    }

    /// Why GitLab didn't, in a problem box with Dismiss, where it didn't.
    pub(super) fn problem(&mut self, ui: &mut Ui) {
        let Some(Err(problem)) = self.answer.read_as_ref() else {
            return;
        };
        let problem = problem.clone();
        let mut dismissed = false;
        widgets::problem_box(ui, &problem, |ui| {
            dismissed = widgets::secondary(ui, true, "Dismiss").clicked();
        });
        ui.add_space(8.0);
        if dismissed {
            self.answer = Bind::new(true);
        }
    }
}

/// The workflow in `path` as the repository holds it at `commit` (`git cat-file blob`, in its
/// work tree `work_tree`), or as `kept` holds it already; none where it can't be read (the
/// commit isn't here: a fork's), which leaves its jobs without needs.
fn workflow_at(
    git: &Git,
    (work_tree, commit): (&std::path::Path, ObjectId),
    path: &str,
    kept: &KeptWorkflow,
) -> Arc<Workflow> {
    if let Some((held, workflow)) = kept.lock().as_ref()
        && held == path
    {
        return Arc::clone(workflow);
    }
    let read = git
        .read("cat-file")
        .args(["blob".to_owned(), format!("{commit}:{path}")])
        .current_dir(work_tree)
        .output();
    let workflow = Arc::new(match read {
        Ok(done) => Workflow::parse(&String::from_utf8_lossy(&done.stdout)),
        Err(error) => {
            tracing::debug!(%error, path, "the workflow's file couldn't be read");
            Workflow::default()
        }
    });
    *kept.lock() = Some((path.to_owned(), Arc::clone(&workflow)));
    workflow
}

/// What `action` does, in words: "Retrying the failed jobs".
const fn doing(action: CiAction) -> &'static str {
    match action {
        CiAction::RetryPipeline(_) => "Running the failed jobs again",
        CiAction::RerunPipeline(_) => "Running every job again",
        CiAction::CancelPipeline(_) => "Cancelling",
        CiAction::RetryJob(_) => "Running the job again",
        CiAction::CancelJob(_) => "Cancelling the job",
        CiAction::PlayJob(_) => "Starting the job",
    }
}

/// Why the forge didn't do `action`, in words to act on.
fn refusal(action: CiAction, error: &egit_forge::ForgeError) -> Problem {
    let title = match action {
        CiAction::RetryPipeline(_) => "The failed jobs couldn't be run again",
        CiAction::RerunPipeline(_) => "The jobs couldn't be run again",
        CiAction::CancelPipeline(_) => "It couldn't be cancelled",
        CiAction::RetryJob(_) => "The job couldn't be retried",
        CiAction::CancelJob(_) => "The job couldn't be cancelled",
        CiAction::PlayJob(_) => "The job couldn't be started",
    };
    match error {
        egit_forge::ForgeError::Forbidden { host, message } => {
            let why = if message.contains("Resource not accessible")
                || message.contains("admin rights")
                || message.contains("write access")
            {
                format!(
                    "{host} refused ({message}): running and cancelling workflows takes write \
                     access to the repository, and a token that may write Actions (a classic \
                     token's repo scope; a fine-grained one's Actions: read and write)."
                )
            } else {
                format!(
                    "{host} refused ({message}): your role on the project may not run its \
                     pipelines (Developer and above may), or your token lacks the api scope."
                )
            };
            Problem::new(title, why)
        }
        other => Problem::new(title, other.to_string()),
    }
}

/// Draws the pipeline's page; returns what it asks of the history view.
pub(super) fn show(
    ui: &mut Ui,
    shared: &Shared<'_>,
    view: &mut PipelineView,
) -> Option<PipelineEvent> {
    let escape = !ui.ctx().text_edit_focused()
        && ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape));
    let acted = view.follow(ui, shared);
    let words = CiWords::of(shared.forge.kind_of(view.mark.repository));
    let mut event = if acted {
        Some(PipelineEvent::Acted)
    } else {
        escape.then_some(PipelineEvent::Close)
    };
    let details = view
        .details
        .read_as_ref()
        .map(|read| read.map(Arc::clone).map_err(Clone::clone));
    let (details, problem) = match details {
        Some(Ok(details)) => (Some(details), None),
        Some(Err(problem)) => (None, Some(problem)),
        None => (None, None),
    };
    let pipeline = details
        .as_ref()
        .map_or(&view.mark.pipeline, |details| &details.pipeline)
        .clone();
    match bar(ui, (shared, words), &pipeline, details.as_deref(), view) {
        Some(BarAsk::Refresh) => view.due = true,
        Some(BarAsk::Close) => event = Some(PipelineEvent::Close),
        Some(BarAsk::Open(url)) => event = Some(PipelineEvent::Open(url)),
        Some(BarAsk::Act(action)) => view.act(shared.forge, action),
        Some(BarAsk::Other(mark)) => event = Some(PipelineEvent::Other(mark)),
        Some(BarAsk::Download(artifact, of)) => {
            event = Some(PipelineEvent::Download(Box::new(ArtifactWanted {
                repository: view.mark.repository,
                artifact,
                of,
            })));
        }
        None => {}
    }
    let palette = shared.palette;
    let mut asked = None;
    egui::CentralPanel::default()
        .frame(Frame::new().inner_margin(Margin::symmetric(12, 16)))
        .show(ui, |ui| {
            view.acting.problem(ui);
            if let Some(summary) = summary(ui, palette, &pipeline, details.as_deref()) {
                event = Some(summary);
            }
            ui.add_space(12.0);
            match (&details, &problem) {
                (Some(details), _) => {
                    let layout = layout_choice(ui, details.ordered_stages);
                    ui.add_space(8.0);
                    asked = match layout {
                        JobsLayout::List => list::stages(ui, (palette, words), details),
                        by => graph::graph(ui, (palette, words), details, (by, &mut view.graph)),
                    };
                }
                (None, Some(problem)) => {
                    widgets::problem_box(ui, problem, |ui| {
                        let again = widgets::secondary(ui, true, "Try again");
                        if again.clicked() {
                            view.due = true;
                        }
                    });
                }
                (None, None) => widgets::working(ui, "Reading its stages and jobs…"),
            }
        });
    match asked {
        Some(JobAsk::Log(job, run)) => event = Some(PipelineEvent::Job(Box::new(job), run)),
        Some(JobAsk::Act(action)) => view.act(shared.forge, action),
        Some(JobAsk::Open(url)) => event = Some(PipelineEvent::Open(url)),
        Some(JobAsk::Download(artifact, of)) => {
            event = Some(PipelineEvent::Download(Box::new(ArtifactWanted {
                repository: view.mark.repository,
                artifact,
                of,
            })));
        }
        None => {}
    }
    event
}

/// The jobs' layout chosen: a list of stages, or a graph by stage or by dependency; by stage
/// only where stages run in order (GitLab's: a GitHub workflow's jobs run as they need).
fn layout_choice(ui: &mut Ui, ordered_stages: bool) -> JobsLayout {
    let mut chosen = JobsLayout::chosen(ui.ctx());
    if !ordered_stages && chosen == JobsLayout::Stages {
        chosen = JobsLayout::Dependencies;
    }
    let all = [
        (JobsLayout::List, "List"),
        (JobsLayout::Stages, "Stages"),
        (JobsLayout::Dependencies, "Dependencies"),
    ];
    let unordered = [
        (JobsLayout::List, "List"),
        (JobsLayout::Dependencies, "Dependencies"),
    ];
    let choices: &[(JobsLayout, &str)] = if ordered_stages { &all } else { &unordered };
    let mut picked = chosen;
    // What each layout shows, on hover: the page keeps to the jobs.
    let explained = match (picked, ordered_stages) {
        (JobsLayout::List, true) => "Each stage's jobs, in the order they run.",
        (JobsLayout::List, false) => "Each job, with its matrix's beside it.",
        (JobsLayout::Stages, _) => "Each stage a column; lines to what each job needs.",
        (JobsLayout::Dependencies, true) => {
            "Columns by what each job waits for: its needs, else the stages before it."
        }
        (JobsLayout::Dependencies, false) => {
            "Columns by what each job needs, as its workflow's file says."
        }
    };
    ui.horizontal(|ui| widgets::segmented(ui, &mut picked, choices))
        .response
        .on_hover_text(explained);
    if picked != chosen {
        picked.choose(ui.ctx());
    }
    picked
}

/// What the bar asked.
enum BarAsk {
    Close,
    Refresh,
    Open(String),
    Act(CiAction),
    /// Another of the commit's pipelines.
    Other(Box<CiMark>),
    /// One of its artifacts, and what it is of.
    Download(Artifact, String),
}

/// The bar: back, the pipeline's number and status, Refresh (a spinner while reading), its
/// page on GitLab; then, as GitLab allows, retrying its failed jobs and cancelling it (asked
/// first, in place), or what is under way.
fn bar(
    ui: &mut Ui,
    (shared, words): (&Shared<'_>, CiWords),
    pipeline: &egit_forge::Pipeline,
    details: Option<&PipelineDetails>,
    view: &mut PipelineView,
) -> Option<BarAsk> {
    let palette = shared.palette;
    let reading = view.details.is_pending();
    let acting = view.acting.is_pending();
    let mut event = None;
    widgets::bar(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            if widgets::secondary(ui, true, &format!("{}  History", icon::ARROW_LEFT)).clicked() {
                event = Some(BarAsk::Close);
            }
            // Its title; where the commit ran others (a GitHub commit's other workflows), a menu
            // of them, as GitHub's run page has it.
            if let Some(other) = title(ui, (palette, words), pipeline, &view.mark) {
                event = Some(BarAsk::Other(Box::new(other)));
            }
            let (glyph, color) = ci_look(pipeline.status, palette);
            ui.label(
                RichText::new(format!(
                    "{glyph}  {}",
                    ci_words(pipeline.status, pipeline.warnings)
                ))
                .color(color),
            );
            // Always pressable: a read under way just reads again after it.
            if widgets::secondary(ui, true, "Refresh").clicked() {
                event = Some(BarAsk::Refresh);
            }
            let open = format!("{}  Open on {}", icon::ARROW_SQUARE_OUT, words.forge);
            if widgets::secondary(ui, true, &open).clicked() {
                event = Some(BarAsk::Open(pipeline.url.clone()));
            }
            // What comes and goes follows the buttons, on a row of its own where it doesn't fit.
            let widths = action_widths(ui, (details, words), view);
            widgets::room_for(ui, &widths);
            ui.scope_builder(
                egui::UiBuilder::new()
                    .id(Id::new("pipeline-actions"))
                    .layout(Layout::left_to_right(Align::Center)),
                |ui| {
                    if let Some((artifact, of)) = details.and_then(|details| artifacts(ui, details))
                    {
                        event = Some(BarAsk::Download(artifact, of));
                    }
                    if let Some(details) = details
                        && let Some(asked) = pipeline_actions(ui, (details, words), view, acting)
                    {
                        event = Some(asked);
                    }
                    if let Some(doing) = view.acting.doing() {
                        widgets::working(ui, &format!("{doing}…"));
                    } else if reading {
                        ui.spinner();
                    }
                },
            );
        });
    });
    event
}

/// The pipeline's title; where its commit ran others (another GitHub workflow's runs), a menu of
/// them under it, each as it stands. The one chosen.
fn title(
    ui: &mut Ui,
    (palette, words): (Palette, CiWords),
    pipeline: &egit_forge::Pipeline,
    mark: &CiMark,
) -> Option<CiMark> {
    let title = pipeline.title();
    if mark.others.is_empty() {
        let shown = ui.label(RichText::new(title).semibold());
        crate::help::note_topic(ui, shown.rect, TOPIC);
        return None;
    }
    let count = mark.others.len();
    let header = format!(
        "{count} more {}{} for this commit",
        words.noun,
        if count == 1 { "" } else { "s" }
    );
    let mut chosen = None;
    widgets::tool_menu_of(
        ui,
        (icon::STACK, &title),
        widgets::Look::Full,
        TOPIC,
        |ui| {
            super::actions::menu_rows(ui);
            super::actions::menu_header(ui, &header);
            for (at, (_, other)) in mark.others.iter().enumerate() {
                let (glyph, _) = ci_look(other.status, palette);
                let words = format!(
                    "{}: {}",
                    other.title(),
                    ci_words(other.status, other.warnings)
                );
                if super::actions::item_of(ui, (glyph, &words), Ok(()), TOPIC) {
                    chosen = mark.showing(at);
                }
            }
        },
    );
    chosen
}

/// How wide the bar's actions draw, side by side (and what is being done, beside them).
fn action_widths(
    ui: &Ui,
    (details, words): (Option<&PipelineDetails>, CiWords),
    view: &mut PipelineView,
) -> Vec<f32> {
    let mut texts: Vec<String> = Vec::new();
    let mut widths = Vec::new();
    if details.is_some_and(has_artifacts) {
        widths.push(widgets::tool_width(
            ui,
            (icon::DOWNLOAD_SIMPLE, "Artifacts"),
            widgets::Look::Full,
            true,
        ));
    }
    if view.confirming {
        texts.extend([words.cancel_asked, "Cancel it", "Keep it running"].map(str::to_owned));
    } else if let Some(details) = details {
        if details.actions.retry {
            texts.push(format!("{}  {}", icon::ARROW_CLOCKWISE, words.retry_failed));
        }
        if details.actions.rerun {
            texts.push(format!("{}  {}", icon::ARROWS_CLOCKWISE, words.rerun_all));
        }
        if details.actions.cancel {
            texts.push(format!("{}  {}", icon::PROHIBIT, words.cancel));
        }
    }
    if let Some(doing) = view.acting.doing() {
        texts.push(format!("{doing}…  "));
    }
    widths.extend(texts.iter().map(|text| widgets::button_width(ui, text)));
    // The spinner's room always: a read coming and going never moves the bar's rows.
    widths.push(widgets::TARGET);
    widths
}

/// Whether a pipeline has artifacts to download: a GitHub run's, a GitLab job's.
fn has_artifacts(details: &PipelineDetails) -> bool {
    !details.artifacts.is_empty()
        || details
            .stages
            .iter()
            .flat_map(|stage| &stage.jobs)
            .any(|job| !job.artifacts.is_empty())
}

/// The Artifacts menu, where the pipeline has any: a GitHub run's, each GitLab job's (named
/// after it); the one chosen, and what it is of.
fn artifacts(ui: &mut Ui, details: &PipelineDetails) -> Option<(Artifact, String)> {
    if !has_artifacts(details) {
        return None;
    }
    let mut chosen = None;
    widgets::tool_menu_of(
        ui,
        (icon::DOWNLOAD_SIMPLE, "Artifacts"),
        widgets::Look::Full,
        ARTIFACTS_TOPIC,
        |ui| {
            super::actions::menu_rows(ui);
            let title = details.pipeline.title();
            let run = details.artifacts.iter().map(|artifact| (artifact, None));
            let jobs = details
                .stages
                .iter()
                .flat_map(|stage| &stage.jobs)
                .flat_map(|job| {
                    job.artifacts
                        .iter()
                        .map(move |artifact| (artifact, Some(job.name.as_str())))
                });
            egui::ScrollArea::vertical()
                .max_height(420.0)
                .show(ui, |ui| {
                    if let Some((artifact, of)) = artifact_items(ui, run.chain(jobs)) {
                        chosen = Some((artifact, of.unwrap_or(title)));
                    }
                });
        },
    );
    chosen
}

/// An item downloading each of `artifacts` (each with the job it is of, where said): "Download
/// artifacts.zip of build · 12.3 MB"; an expired one disabled. The one chosen, and its job.
pub(super) fn artifact_items<'a>(
    ui: &mut Ui,
    artifacts: impl Iterator<Item = (&'a Artifact, Option<&'a str>)>,
) -> Option<(Artifact, Option<String>)> {
    let mut chosen = None;
    for (artifact, of) in artifacts {
        let mut words = format!("Download {}", artifact.file_name);
        if let Some(of) = of {
            words.push_str(" of ");
            words.push_str(of);
        }
        if let Some(size) = artifact.size {
            words.push_str(" · ");
            words.push_str(&format::size(size));
        }
        let allowed = if artifact.expired {
            Err("It expired: the forge no longer keeps it")
        } else {
            Ok(())
        };
        let glyph = match artifact.kind {
            ArtifactKind::Report => icon::FILE_TEXT,
            ArtifactKind::Archive | ArtifactKind::Uploaded => icon::FILE_ZIP,
        };
        if super::actions::item_of(ui, (glyph, &words), allowed, ARTIFACTS_TOPIC) {
            chosen = Some((artifact.clone(), of.map(str::to_owned)));
        }
    }
    chosen
}

/// Retrying the failed jobs and cancelling the pipeline (asked first), where GitLab allows them;
/// disabled, saying why, where the account may not.
fn pipeline_actions(
    ui: &mut Ui,
    (details, words): (&PipelineDetails, CiWords),
    view: &mut PipelineView,
    acting: bool,
) -> Option<BarAsk> {
    let id = details.pipeline.id;
    let why_not = (!details.can_update).then_some(if details.ordered_stages {
        "Your role on this project may not run its pipelines (Developer and above may)"
    } else {
        "Running and cancelling workflows takes write access to the repository"
    });
    let mut asked = None;
    if view.confirming {
        ui.label(RichText::new(words.cancel_asked).semibold());
        if widgets::danger(ui, !acting, "Cancel it").clicked() {
            asked = Some(BarAsk::Act(CiAction::CancelPipeline(id)));
        }
        if widgets::secondary(ui, true, "Keep it running").clicked() {
            view.confirming = false;
        }
        return asked;
    }
    let offered = [
        (
            details.actions.retry,
            format!("{}  {}", icon::ARROW_CLOCKWISE, words.retry_failed),
            "Runs again every job that failed or was cancelled; those after them run as the \
             pipeline goes on",
            CiAction::RetryPipeline(id),
        ),
        (
            details.actions.rerun,
            format!("{}  {}", icon::ARROWS_CLOCKWISE, words.rerun_all),
            "Runs every job again, in a new attempt; this one's runs are kept",
            CiAction::RerunPipeline(id),
        ),
    ];
    for (shown, text, does, action) in offered {
        if !shown {
            continue;
        }
        let button =
            widgets::secondary(ui, why_not.is_none() && !acting, &text).on_hover_text(does);
        let button = match why_not {
            Some(why) => button.on_disabled_hover_text(why),
            None => button,
        };
        if button.clicked() {
            asked = Some(BarAsk::Act(action));
        }
    }
    if details.actions.cancel {
        let cancel = widgets::secondary(
            ui,
            why_not.is_none() && !acting,
            &format!("{}  {}", icon::PROHIBIT, words.cancel),
        )
        .on_hover_text("Cancels every job under way or waiting");
        let cancel = match why_not {
            Some(why) => cancel.on_disabled_hover_text(why),
            None => cancel,
        };
        if cancel.clicked() {
            view.confirming = true;
        }
    }
    asked
}

/// How it stands: its branch or tag, its commit (and the way to it in the history), who
/// started it and how, when, and how long it ran (or has run so far).
fn summary(
    ui: &mut Ui,
    palette: Palette,
    pipeline: &egit_forge::Pipeline,
    details: Option<&PipelineDetails>,
) -> Option<PipelineEvent> {
    let mut event = None;
    let now = format::now();
    let width = widgets::FACT_LABEL;
    if let Some(git_ref) = &pipeline.git_ref {
        widgets::fact_of(ui, width, ("Ran for", TOPIC), |ui| {
            ui.add(egui::Label::new(RichText::new(git_ref).monospace()).truncate());
        });
    }
    widgets::fact_of(ui, width, ("Commit", TOPIC), |ui| {
        ui.label(
            RichText::new(pipeline.commit.abbreviated(10).to_string())
                .monospace()
                .color(palette.muted),
        );
        if widgets::secondary(ui, true, "Show the commit").clicked() {
            event = Some(PipelineEvent::ShowCommit(pipeline.commit));
        }
    });
    let created = u64::try_from(pipeline.created).unwrap_or(0);
    let when = format::ago(created, now);
    let started = details
        .and_then(|details| details.user.as_deref())
        .map_or_else(|| when.clone(), |user| format!("{when} by {user}"));
    let how = details
        .and_then(|details| details.source.as_deref())
        .map(source_words);
    widgets::fact_of(ui, width, ("Started", TOPIC), |ui| {
        // The moment itself on hover, in the viewer's zone.
        ui.label(started).on_hover_ui(|ui| {
            ui.label(format::local_date_time(
                pipeline.created,
                &format::local_zone(),
            ));
        });
        if let Some(how) = how {
            ui.label(muted(ui, how));
        }
    });
    if let Some(took) = details.and_then(|details| ran_for(details, now)) {
        widgets::fact_of(ui, width, ("Ran", TOPIC), |ui| {
            ui.label(took);
        });
        // A clock under way ticks each second.
        if details.is_some_and(|details| details.duration.is_none()) && pipeline.status.is_moving()
        {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs(1));
        }
    }
    if pipeline.attempt > 1 {
        widgets::fact_of(ui, width, ("Attempt", TOPIC), |ui| {
            let again = pipeline.attempt.saturating_sub(1);
            ui.label(format!(
                "{} (run again {again} time{})",
                pipeline.attempt,
                if again == 1 { "" } else { "s" }
            ));
        });
    }
    event
}

/// What started a pipeline, in words.
fn source_words(source: &str) -> String {
    match source {
        "push" => "on a push".to_owned(),
        "merge_request_event" => "for a merge request".to_owned(),
        "schedule" => "on a schedule".to_owned(),
        "web" => "from GitLab's page".to_owned(),
        "api" | "repository_dispatch" => "through the API".to_owned(),
        "trigger" => "by a trigger".to_owned(),
        "pipeline" | "parent_pipeline" => "by another pipeline".to_owned(),
        // GitHub's events.
        "pull_request" | "pull_request_target" => "for a pull request".to_owned(),
        "workflow_dispatch" => "started by hand".to_owned(),
        "merge_group" => "for a merge queue".to_owned(),
        "release" => "for a release".to_owned(),
        "workflow_run" => "after another workflow".to_owned(),
        "dynamic" => "by GitHub".to_owned(),
        other => format!("by {}", other.replace('_', " ")),
    }
}

/// How long it ran, or has run so far ("1 min 35 s so far"); none before it starts.
fn ran_for(details: &PipelineDetails, now: u64) -> Option<String> {
    if let Some(seconds) = details.duration {
        return Some(duration(seconds));
    }
    let started = u64::try_from(details.started?).ok()?;
    if details.pipeline.status.is_moving() {
        return Some(format!("{} so far", duration(now.saturating_sub(started))));
    }
    let finished = u64::try_from(details.finished?).ok()?;
    Some(duration(finished.saturating_sub(started)))
}

pub(crate) use crate::format::duration;

/// The notice under the commit table where the pipelines couldn't be read: `why` (cut to its
/// line, whole on hover), and Dismiss; returns whether it was dismissed.
pub(super) fn unread(ui: &mut Ui, repo: crate::repo::RepoId, why: &str) -> bool {
    let palette = Palette::of(ui.ctx());
    let mut dismissed = false;
    ui.scope_builder(
        egui::UiBuilder::new().id(Id::new(("pipelines-unread", repo))),
        |ui| {
            widgets::banner(ui, widgets::Tone::Warn, |ui| {
                crate::help::note_topic(ui, ui.max_rect(), "pipeline-mark");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    dismissed = widgets::icon_tool(ui, true, icon::X, "Dismiss").clicked();
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        ui.label(RichText::new(format!("{}  ", icon::WARNING)).color(palette.warn));
                        ui.add(egui::Label::new(muted(ui, why)).truncate())
                            .on_hover_text(why);
                    });
                });
            });
        },
    );
    dismissed
}
