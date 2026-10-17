//! The application: its state, the jobs it starts, and what the views ask of it.
//!
//! Views never change application state directly: they push [`Action`]s, which are carried out
//! after the frame's views are drawn. Background results arrive through `Bind`s polled at the
//! start of each frame ([`AppState::poll`]), which turns finished jobs into actions too.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egit_git::{
    ConfigPlace, ConfigSnapshot, ConfigTarget, Git, GitOptions, NewCommit, Repository,
    add_config_value, set_config, trust_directory, unset_config, unset_config_value,
};
use egui::{CentralPanel, Frame, Panel, Ui};
use egui_async::Bind;

use crate::desktop::{FileDialogs, FileQuestion, Opener};
use crate::fonts::Fallbacks;
use crate::format::now;
use crate::history::Scope;
use crate::jobs::{self, WriteLock};
use crate::problem::{OpenProblem, Problem};
use crate::repo::{RepoId, RepoTab};
use crate::settings::{Settings, StoredPath};
use crate::staging::StageRequest;
use crate::tabs::{self, Pane};
use crate::tasks::Task;
use crate::theme::{self, Palette, ThemeChoice};
use crate::views::clone::CloneRequest;
use crate::views::home::HomeView;
use crate::views::profile::ProfileView;
use crate::views::prompts::Prompts;
use crate::views::status_bar;
use crate::watch::{Taken, Watcher, Watching};

/// The application.
pub(crate) struct EgitApp {
    pub(crate) tabs: egui_tiles::Tree<Pane>,
    pub(crate) state: AppState,
}

/// Everything but the tab layout (the views borrow it while the layout is drawn).
pub(crate) struct AppState {
    /// The git binary, once found and checked.
    pub(crate) git: Bind<Git, Problem>,
    git_options: GitOptions,
    /// The user's configuration, outside any repository.
    pub(crate) user_config: Bind<Arc<ConfigSnapshot>, Problem>,
    /// Commands that write the user's configuration take it.
    user_writes: WriteLock,
    pub(crate) repos: Vec<RepoTab>,
    pub(crate) home: HomeView,
    pub(crate) profile: ProfileView,
    /// Git's and ssh's questions, waiting for the user.
    pub(crate) prompts: Prompts,
    pub(crate) settings: Settings,
    /// The user's home folder, shown as `~`.
    pub(crate) home_dir: Option<PathBuf>,
    opening: Vec<Opening>,
    /// Watches the files the user's configuration comes from (and which they are).
    user_watcher: Option<(Vec<PathBuf>, Watching)>,
    /// Paths to open once git is ready.
    waiting: Vec<PathBuf>,
    pub(crate) actions: Vec<Action>,
    next_repo: RepoId,
    /// System fonts for characters egui's fonts lack.
    fonts: Fallbacks,
    /// Whether the window had the focus last frame (coming back to it reads work trees again).
    focused: bool,
    /// Where web pages go.
    pub(crate) opener: Opener,
    /// The desktop's file dialogs.
    pub(crate) files: FileDialogs,
}

/// A repository being opened.
struct Opening {
    job: Bind<Repository, OpenProblem>,
}

/// A change to the user's global configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ConfigChange {
    /// `key` set to `value`, every value it had replaced; `None` removes it.
    Set {
        key: &'static str,
        value: Option<String>,
    },
    /// `value` added to the values of `key` (one that may have several), unless it has it.
    Add { key: &'static str, value: String },
    /// `value` removed from the values of `key`, its others kept.
    Remove { key: &'static str, value: String },
}

/// What the views (and finished jobs) ask the application to do.
#[derive(Debug)]
pub(crate) enum Action {
    /// Look for git again.
    DetectGit,
    /// Ask for a folder to open.
    PickFolder,
    /// Ask for the folder clones go into.
    PickCloneFolder,
    /// Clone a repository (then open it).
    Clone(CloneRequest),
    /// Stop the clone running.
    CancelClone,
    /// Open the repository around a path.
    Open(PathBuf),
    /// A repository was found: show it.
    Opened(Box<Repository>),
    /// Add a path to `safe.directory`, then open it.
    Trust(PathBuf),
    /// Remove a path from the recent repositories.
    Forget(PathBuf),
    /// Show the profile tab.
    ShowProfile,
    /// Write changes to the user's configuration.
    SaveProfile(Vec<ConfigChange>),
    /// Read a repository's state again.
    Refresh(RepoId),
    /// Write a commit-graph file for a repository.
    WriteCommitGraph(RepoId),
    /// Stage or unstage in a repository.
    Stage(RepoId, StageRequest),
    /// Commit what a repository's index holds.
    Commit(RepoId, NewCommit),
    /// Fetch, pull, push, or give up or carry on an operation, in a repository.
    Task(RepoId, Task),
    /// Stop a repository's running task.
    CancelTask(RepoId),
    /// A repository's tab was closed.
    Closed(RepoId),
    /// Use a theme.
    SetTheme(ThemeChoice),
    /// Sign in to a forge's host, for a repository's pull requests.
    SignIn(RepoId, String),
    /// Read a repository's pull requests again.
    RefreshPullRequests(RepoId),
    /// Open a web page in the user's browser.
    OpenUrl(String),
    /// Show this much of a repository's history.
    Scope(RepoId, Scope),
    /// Ask the desktop's file dialog `question` (`title` on it; `name`, a file offered).
    AskFiles {
        question: FileQuestion,
        title: &'static str,
        name: String,
    },
    /// Show patches to apply to a repository, read from files.
    OpenPatches(RepoId, Vec<PathBuf>),
}

impl EgitApp {
    /// The application as eframe creates it: settings from storage, git looked for on `PATH`,
    /// and `paths` (from the command line) opened once it is found.
    pub(crate) fn new(creation: &eframe::CreationContext<'_>, paths: Vec<PathBuf>) -> Self {
        let settings = Settings::load(creation.storage);
        let home_dir = std::env::var_os("HOME").map(PathBuf::from);
        // Every git process asks its questions through this window.
        let (prompts, env) = Prompts::start(&creation.egui_ctx);
        let options = GitOptions {
            program: None,
            env,
            unset: Vec::new(),
        };
        Self::start(
            &creation.egui_ctx,
            settings,
            home_dir,
            None,
            options,
            prompts,
            paths,
        )
    }

    /// An application around a git that was already found (the tests' sandbox).
    #[cfg(test)]
    pub(crate) fn with_git(
        ctx: &egui::Context,
        git: Git,
        settings: Settings,
        home_dir: Option<PathBuf>,
        prompts: Prompts,
    ) -> Self {
        let mut app = Self::start(
            ctx,
            settings,
            home_dir,
            Some(git),
            GitOptions::default(),
            prompts,
            Vec::new(),
        );
        // Tests look at the pages asked for; no browser starts.
        app.state.opener = Opener::recording();
        app
    }

    fn start(
        ctx: &egui::Context,
        settings: Settings,
        home_dir: Option<PathBuf>,
        git: Option<Git>,
        git_options: GitOptions,
        prompts: Prompts,
        paths: Vec<PathBuf>,
    ) -> Self {
        theme::install_fonts(ctx);
        theme::apply(ctx, settings.theme);
        ctx.plugin_or_default::<egui_async::EguiAsyncPlugin>();
        if let Err(error) = jobs::install_runtime() {
            tracing::error!(%error, "couldn't start egit's worker threads");
        }

        // Repositories open when egit last closed come back (not those since removed), then
        // those asked for now.
        let mut waiting: Vec<PathBuf> = settings
            .open
            .iter()
            .map(StoredPath::to_path)
            .filter(|path| path.is_dir())
            .collect();
        waiting.extend(paths);
        let mut state = AppState {
            git: Bind::new(true),
            git_options,
            user_config: Bind::new(true),
            user_writes: WriteLock::default(),
            repos: Vec::new(),
            home: HomeView::new(
                settings.clone_folder.as_ref().map(StoredPath::to_path),
                home_dir.as_deref(),
            ),
            profile: ProfileView::default(),
            prompts,
            settings,
            home_dir,
            opening: Vec::new(),
            user_watcher: None,
            waiting,
            actions: Vec::new(),
            next_repo: 0,
            focused: true,
            opener: Opener::default(),
            files: FileDialogs::default(),
            fonts: Fallbacks::default(),
        };
        match git {
            Some(git) => state.git.fill(Ok(git)),
            None => state.detect_git(),
        }
        Self {
            tabs: tabs::initial(),
            state,
        }
    }

    /// Carries out the actions the frame produced.
    fn apply_actions(&mut self, ctx: &egui::Context) {
        while !self.state.actions.is_empty() {
            let actions = std::mem::take(&mut self.state.actions);
            for action in actions {
                self.apply(ctx, action);
            }
        }
    }

    fn apply(&mut self, ctx: &egui::Context, action: Action) {
        let state = &mut self.state;
        match action {
            Action::DetectGit => state.detect_git(),
            Action::PickFolder => state.home.pick_folder(),
            Action::PickCloneFolder => state.home.clone.pick_folder(),
            Action::Clone(request) => {
                if let Some(git) = state.git() {
                    state.home.clone.start(&git, request);
                }
            }
            Action::CancelClone => state.home.clone.cancel(),
            Action::Open(path) => state.open(path),
            Action::Opened(repository) => self.show_repository(ctx, *repository),
            Action::Trust(path) => state.trust(path),
            Action::Forget(path) => state.settings.forget(&path),
            Action::ShowProfile => tabs::show(&mut self.tabs, Pane::Profile),
            Action::SaveProfile(changes) => state.save_profile(changes),
            Action::Refresh(id) => {
                if let Some(git) = state.git()
                    && let Some(repo) = state.repos.iter_mut().find(|repo| repo.id == id)
                {
                    repo.refresh(&git);
                }
            }
            Action::WriteCommitGraph(id) => {
                if let Some(git) = state.git()
                    && let Some(repo) = state.repos.iter_mut().find(|repo| repo.id == id)
                {
                    repo.write_commit_graph(&git);
                }
            }
            Action::Stage(id, request) => {
                if let Some(git) = state.git()
                    && let Some(repo) = state.repos.iter_mut().find(|repo| repo.id == id)
                {
                    repo.stage(&git, request);
                }
            }
            Action::Commit(id, commit) => {
                if let Some(git) = state.git()
                    && let Some(repo) = state.repos.iter_mut().find(|repo| repo.id == id)
                {
                    repo.commit(&git, commit);
                }
            }
            Action::Task(id, task) => {
                if let Some(git) = state.git()
                    && let Some(repo) = state.repos.iter_mut().find(|repo| repo.id == id)
                {
                    repo.run_task(&git, task);
                }
            }
            Action::CancelTask(id) => {
                if let Some(repo) = state.repos.iter_mut().find(|repo| repo.id == id) {
                    repo.tasks.cancel();
                }
            }
            Action::Closed(id) => state.repos.retain(|repo| repo.id != id),
            Action::SetTheme(choice) => {
                state.settings.theme = choice;
                theme::apply(ctx, choice);
            }
            Action::SignIn(id, host) => {
                if let Some(repo) = state.repos.iter_mut().find(|repo| repo.id == id) {
                    repo.forge.sign_in(&host);
                }
            }
            Action::RefreshPullRequests(id) => {
                if let Some(repo) = state.repos.iter_mut().find(|repo| repo.id == id) {
                    repo.forge.refresh();
                }
            }
            Action::OpenUrl(url) => state.opener.open(url),
            Action::Scope(id, scope) => {
                if let Some(git) = state.git()
                    && let Some(repo) = state.repos.iter_mut().find(|repo| repo.id == id)
                {
                    repo.set_scope(&git, scope);
                }
            }
            Action::AskFiles {
                question,
                title,
                name,
            } => state.files.ask(question, title, name),
            Action::OpenPatches(id, files) => {
                if let Some(repo) = state.repos.iter_mut().find(|repo| repo.id == id) {
                    repo.history_view.show_patches(files);
                }
            }
        }
    }

    /// Shows a repository found by discovery: its tab, which is new unless it is open already.
    fn show_repository(&mut self, ctx: &egui::Context, repository: Repository) {
        let state = &mut self.state;
        state.home.problem = None;
        state.settings.remember(repository.work_tree(), now());
        if let Some(open) = state
            .repos
            .iter()
            .find(|repo| repo.repository.work_tree() == repository.work_tree())
        {
            tabs::show(&mut self.tabs, Pane::Repo(open.id));
            return;
        }
        let Some(git) = state.git() else {
            return;
        };
        let id = state.next_repo;
        state.next_repo = state.next_repo.wrapping_add(1);
        state.repos.push(RepoTab::new(id, repository, &git, ctx));
        tabs::show(&mut self.tabs, Pane::Repo(id));
    }
}

impl AppState {
    /// Git, once found.
    pub(crate) fn git(&mut self) -> Option<Git> {
        self.git.ok_ref().cloned()
    }

    fn detect_git(&mut self) {
        let options = self.git_options.clone();
        self.git.request(jobs::blocking(move || {
            Git::new(options).map_err(|error| Problem::new("egit can't use git", error.to_string()))
        }));
    }

    /// Advances the jobs and the watchers: what finished or changed becomes state or actions.
    fn poll(&mut self, ctx: &egui::Context) {
        self.prompts.poll();
        self.fonts.poll(ctx);
        // Back from an editor or a terminal: what the user did there is what they look for.
        let focused = ctx.input(|input| input.focused);
        let regained = focused && !std::mem::replace(&mut self.focused, focused);
        for repo in &mut self.repos {
            repo.poll();
            if regained {
                repo.work_tree.refresh_now();
            }
        }
        if let Some(git) = self.git() {
            if self.user_config.is_idle() {
                self.refresh_user_config(&git);
            }
            self.actions
                .extend(self.waiting.drain(..).map(Action::Open));
            self.follow_changes(ctx, &git);
        }

        let mut finished = Vec::new();
        self.opening.retain_mut(|opening| {
            let Some(result) = opening.job.take() else {
                return true;
            };
            finished.push(result);
            false
        });
        for result in finished {
            match result {
                Ok(repository) => self.actions.push(Action::Opened(Box::new(repository))),
                Err(problem) => self.home.problem = Some(problem),
            }
        }

        if let Some(picked) = self.home.picking.take() {
            match picked {
                Ok(Some(path)) => self.actions.push(Action::Open(path)),
                Ok(None) => {}
                Err(problem) => self.home.problem = Some(OpenProblem::from_problem(problem)),
            }
        }
        self.take_clone();
        if let Some((question, chosen)) = self.files.answered() {
            self.answered(question, chosen);
        }
        if let Some(trusted) = self.home.trusting.take() {
            match trusted {
                Ok(path) => self.actions.push(Action::Open(path)),
                Err(problem) => self.home.problem = Some(OpenProblem::from_problem(problem)),
            }
        }
        if let Some(saved) = self.profile.saving.take() {
            match saved {
                Ok(()) => {
                    self.profile.problem = None;
                    if let Some(git) = self.git() {
                        self.refresh_user_config(&git);
                        for repo in &mut self.repos {
                            repo.refresh_config(&git);
                        }
                    }
                }
                Err(problem) => self.profile.problem = Some(problem),
            }
        }
    }

    /// Acts on a file dialog's answer: saves there, or shows the patches chosen.
    fn answered(&mut self, question: FileQuestion, mut chosen: Vec<PathBuf>) {
        let first = chosen.first().cloned().unwrap_or_default();
        match question {
            FileQuestion::SavePatch { repo, commit } => self.actions.push(Action::Task(
                repo,
                Task::SavePatch {
                    commit,
                    path: first,
                },
            )),
            FileQuestion::SavePatches { repo, from } => self.actions.push(Action::Task(
                repo,
                Task::SavePatches {
                    from,
                    folder: first,
                },
            )),
            FileQuestion::SaveChanges { repo, changes } => self.actions.push(Action::Task(
                repo,
                Task::SaveChanges {
                    changes,
                    path: first,
                },
            )),
            FileQuestion::ApplyPatches { repo } => {
                chosen.dedup();
                self.actions.push(Action::OpenPatches(repo, chosen));
            }
        }
    }

    /// Takes in the clone folder picked, and a clone finished: it opens, and the next clone goes
    /// into the same folder.
    fn take_clone(&mut self) {
        let clone = &mut self.home.clone;
        match clone.picking.take() {
            Some(Ok(Some(folder))) => clone.picked(folder, self.home_dir.as_deref()),
            Some(Ok(None)) | None => {}
            Some(Err(problem)) => clone.problem = Some(problem),
        }
        match clone.cloning.take() {
            Some(Ok(Some(destination))) => {
                clone.done();
                self.settings.clone_folder = destination.parent().map(StoredPath::from);
                self.actions.push(Action::Open(destination));
            }
            Some(Ok(None)) => clone.cancelled = true,
            Some(Err(problem)) => clone.problem = Some(problem),
            None => {}
        }
    }

    /// Reads again whatever changed outside egit, and keeps watching the files the user's
    /// configuration comes from.
    fn follow_changes(&mut self, ctx: &egui::Context, git: &Git) {
        let now = Instant::now();
        let mut wait = None;
        for repo in &mut self.repos {
            wait = sooner(wait, repo.follow_changes(git, now));
        }

        if let Some(snapshot) = self.user_config.ok_ref() {
            let mut files: Vec<PathBuf> = snapshot
                .entries()
                .filter_map(egit_git::ConfigEntry::file)
                .collect();
            files.sort();
            files.dedup();
            let watched = self.user_watcher.as_ref().map(|(watched, _)| watched);
            if watched != Some(&files) {
                let watching = Watching::new(now, || watch_files(files.clone(), ctx));
                self.user_watcher = Some((files, watching));
            }
        }
        let taken = self
            .user_watcher
            .as_mut()
            .map_or(Taken::Nothing, |(files, watching)| {
                watching.take(now, || watch_files(files.clone(), ctx))
            });
        match taken {
            Taken::Nothing => {}
            Taken::Wait(delay) => wait = sooner(wait, Some(delay)),
            Taken::Changed(_) => {
                self.refresh_user_config(git);
                for repo in &mut self.repos {
                    repo.refresh_config(git);
                }
            }
        }
        if let Some(delay) = wait {
            ctx.request_repaint_after(delay);
        }
    }

    /// Why changes made outside egit aren't being followed (in the user's configuration or a
    /// repository), while they aren't.
    pub(crate) fn watch_problem(&self) -> Option<String> {
        self.user_watcher
            .as_ref()
            .and_then(|(_, watching)| watching.problem())
            .map(ToOwned::to_owned)
            .or_else(|| self.repos.iter().find_map(RepoTab::watch_problem))
    }

    /// Whether repositories are being opened.
    pub(crate) const fn is_opening(&self) -> bool {
        !self.opening.is_empty() || !self.waiting.is_empty()
    }

    fn refresh_user_config(&mut self, git: &Git) {
        let git = git.clone();
        self.user_config.request(jobs::blocking(move || {
            ConfigSnapshot::read(&git, ConfigPlace::User)
                .map(Arc::new)
                .map_err(|error| Problem::git("Your git settings couldn't be read", &error))
        }));
    }

    fn open(&mut self, path: PathBuf) {
        self.home.problem = None;
        let Some(git) = self.git() else {
            self.waiting.push(path);
            return;
        };
        let mut job = Bind::new(true);
        job.request(jobs::blocking(move || {
            Repository::discover(&git, &path).map_err(|error| OpenProblem::from_discovery(&error))
        }));
        self.opening.push(Opening { job });
    }

    fn trust(&mut self, path: PathBuf) {
        let Some(git) = self.git() else {
            return;
        };
        self.home.problem = None;
        let writes = self.user_writes.clone();
        self.home.trusting.request(jobs::blocking(move || {
            let _queued = writes.hold();
            trust_directory(&git, &path)
                .map(|()| path)
                .map_err(|error| Problem::git("The folder couldn't be trusted", &error))
        }));
    }

    fn save_profile(&mut self, changes: Vec<ConfigChange>) {
        let Some(git) = self.git() else {
            return;
        };
        let writes = self.user_writes.clone();
        self.profile.saving.request(jobs::blocking(move || {
            let _queued = writes.hold();
            for change in changes {
                let target = ConfigTarget::Global;
                let written = match &change {
                    ConfigChange::Set {
                        key,
                        value: Some(value),
                    } => set_config(&git, target, key, value),
                    ConfigChange::Set { key, value: None } => unset_config(&git, target, key),
                    ConfigChange::Add { key, value } => add_config_value(&git, target, key, value),
                    ConfigChange::Remove { key, value } => {
                        unset_config_value(&git, target, key, value)
                    }
                };
                written.map_err(|error| Problem::git("Your settings couldn't be saved", &error))?;
            }
            Ok(())
        }));
    }
}

impl eframe::App for EgitApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.state.poll(ui.ctx());
        let palette = Palette::of(ui.ctx());
        Panel::bottom("egit-status")
            .frame(
                Frame::new()
                    .fill(palette.panel)
                    .inner_margin(egui::Margin::symmetric(10, 4)),
            )
            .show(ui, |ui| status_bar::show(ui, &mut self.state));
        CentralPanel::default()
            .frame(Frame::new().fill(palette.background))
            .show(ui, |ui| tabs::draw(&mut self.tabs, &mut self.state, ui));
        self.state.prompts.show(ui.ctx());
        self.apply_actions(ui.ctx());
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.state.settings.open = self
            .state
            .repos
            .iter()
            .map(|repo| repo.repository.work_tree().into())
            .collect();
        self.state.settings.save(storage);
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        visuals.panel_fill.to_normalized_gamma_f32()
    }
}

/// Watches the user's configuration `files`, waking `ctx` when one changes.
fn watch_files(files: Vec<PathBuf>, ctx: &egui::Context) -> notify::Result<Watcher> {
    let waker = ctx.clone();
    Watcher::files(files, move |delay| waker.request_repaint_after(delay))
}

/// The sooner of two optional delays.
fn sooner(a: Option<Duration>, b: Option<Duration>) -> Option<Duration> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}
