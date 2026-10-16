//! The application: its state, the jobs it starts, and what the views ask of it.
//!
//! Views never change application state directly: they push [`Action`]s, which are carried out
//! after the frame's views are drawn. Background results arrive through `Bind`s polled at the
//! start of each frame ([`AppState::poll`]), which turns finished jobs into actions too.

use std::path::PathBuf;
use std::sync::Arc;

use egit_git::{
    ConfigPlace, ConfigSnapshot, ConfigTarget, Git, GitOptions, Repository, set_config,
    trust_directory, unset_config,
};
use egui::{CentralPanel, Frame, Panel, Ui};
use egui_async::Bind;

use crate::format::now;
use crate::jobs::{self, WriteLock};
use crate::problem::{OpenProblem, Problem};
use crate::repo::{RepoId, RepoTab};
use crate::settings::{Settings, StoredPath};
use crate::tabs::{self, Pane};
use crate::theme::{self, Palette, ThemeChoice};
use crate::views::home::HomeView;
use crate::views::profile::ProfileView;
use crate::views::prompts::Prompts;
use crate::views::status_bar;

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
    /// Paths to open once git is ready.
    waiting: Vec<PathBuf>,
    pub(crate) actions: Vec<Action>,
    next_repo: RepoId,
}

/// A repository being opened.
struct Opening {
    job: Bind<Repository, OpenProblem>,
}

/// A configuration value to write to the user's global configuration; `None` removes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ConfigChange {
    pub(crate) key: &'static str,
    pub(crate) value: Option<String>,
}

/// What the views (and finished jobs) ask the application to do.
#[derive(Debug)]
pub(crate) enum Action {
    /// Look for git again.
    DetectGit,
    /// Ask for a folder to open.
    PickFolder,
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
    /// A repository's tab was closed.
    Closed(RepoId),
    /// Use a theme.
    SetTheme(ThemeChoice),
}

impl EgitApp {
    /// The application as eframe creates it: settings from storage, git looked for on `PATH`,
    /// and `paths` (from the command line) opened once it is found.
    pub(crate) fn new(creation: &eframe::CreationContext<'_>, paths: Vec<PathBuf>) -> Self {
        let settings = Settings::load(creation.storage);
        let home_dir = std::env::var_os("HOME").map(PathBuf::from);
        // Every git process asks its questions through this window.
        let (prompts, env) = Prompts::start(&creation.egui_ctx);
        let options = GitOptions { program: None, env };
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
        Self::start(
            ctx,
            settings,
            home_dir,
            Some(git),
            GitOptions::default(),
            prompts,
            Vec::new(),
        )
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

        // Repositories open when egit last closed come back, then those asked for now.
        let mut waiting: Vec<PathBuf> = settings.open.iter().map(StoredPath::to_path).collect();
        waiting.extend(paths);
        let mut state = AppState {
            git: Bind::new(true),
            git_options,
            user_config: Bind::new(true),
            user_writes: WriteLock::default(),
            repos: Vec::new(),
            home: HomeView::default(),
            profile: ProfileView::default(),
            prompts,
            settings,
            home_dir,
            opening: Vec::new(),
            waiting,
            actions: Vec::new(),
            next_repo: 0,
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
            Action::Open(path) => state.open(path),
            Action::Opened(repository) => self.show_repository(*repository),
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
            Action::Closed(id) => state.repos.retain(|repo| repo.id != id),
            Action::SetTheme(choice) => {
                state.settings.theme = choice;
                theme::apply(ctx, choice);
            }
        }
    }

    /// Shows a repository found by discovery: its tab, which is new unless it is open already.
    fn show_repository(&mut self, repository: Repository) {
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
        state.repos.push(RepoTab::new(id, repository, &git));
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

    /// Advances the jobs: what finished becomes state or actions.
    fn poll(&mut self) {
        self.prompts.poll();
        if let Some(git) = self.git() {
            if self.user_config.is_idle() {
                self.refresh_user_config(&git);
            }
            self.actions
                .extend(self.waiting.drain(..).map(Action::Open));
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
                let written = match &change.value {
                    Some(value) => set_config(&git, ConfigTarget::Global, change.key, value),
                    None => unset_config(&git, ConfigTarget::Global, change.key),
                };
                written.map_err(|error| Problem::git("Your settings couldn't be saved", &error))?;
            }
            Ok(())
        }));
    }
}

impl eframe::App for EgitApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.state.poll();
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
