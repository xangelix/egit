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

use crate::commands::{self, Command, RepoCommand};
use crate::desktop::{FileDialogs, FileQuestion, Opener};
use crate::exporting::Export;
use crate::fonts::Fallbacks;
use crate::format::now;
use crate::history::Scope;
use crate::jobs::{self, WriteLock};
use crate::preferences::Preferences;
use crate::problem::{OpenProblem, Problem};
use crate::repo::{RepoId, RepoPanel, RepoTab};
use crate::settings::{Settings, StoredPath};
use crate::staging::StageRequest;
use crate::tabs::{self, Pane};
use crate::tasks::Task;
use crate::theme::{self, Palette, ThemeChoice};
use crate::views::clone::CloneRequest;
use crate::views::home::HomeView;
use crate::views::palette::{self, Offer, Offers, PaletteView};
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
    /// The command palette (⌘K).
    pub(crate) palette: PaletteView,
    /// The help window, while open.
    pub(crate) help: crate::help::HelpWindow,
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

/// What is asked of a repository's output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OutputAsk {
    /// Stop the commit under way (its hooks with it).
    CancelCommit,
    /// Open the panel, on the latest run.
    Show,
    /// Open or close the panel.
    Toggle,
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
    /// Show a page of a repository's tab (or its history).
    ShowPanel(RepoId, RepoPanel),
    /// Write a commit-graph file for a repository.
    WriteCommitGraph(RepoId),
    /// Stage or unstage in a repository.
    Stage(RepoId, StageRequest),
    /// Commit what a repository's index holds.
    Commit(RepoId, NewCommit),
    /// Something of a repository's output: its commit stopped, its panel opened or toggled.
    Output(RepoId, OutputAsk),
    /// Fetch, pull, push, or give up or carry on an operation, in a repository.
    Task(RepoId, Task),
    /// Stop a repository's running task.
    CancelTask(RepoId),
    /// Put these preferences in effect, and keep them.
    SetPreferences(Preferences),
    /// Show the preferences' tab.
    ShowPreferences,
    /// Open the command palette.
    ShowPalette,
    /// Help, every topic.
    ShowHelp,
    /// Take a repository's last action back (`true`: do the last one taken back again).
    Undo(RepoId, bool),
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
        // People's pictures, fetched while the preferences ask (the tests fetch none).
        crate::avatars::install(
            &creation.egui_ctx,
            &egit_forge::PictureSources::public(),
            crate::avatars::cache_folder(home_dir.as_deref()).as_deref(),
        );
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
        settings.preferences.apply(ctx);
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
            palette: PaletteView::default(),
            help: crate::help::HelpWindow::default(),
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
    /// Runs a command from the keyboard or the palette: the application's own at once; one
    /// about a repository on the repository in front.
    fn run(&mut self, command: Command) {
        let state = &mut self.state;
        match command {
            Command::Palette => {
                if state.palette.is_open() {
                    state.palette = PaletteView::default();
                } else {
                    let front = tabs::front(&self.tabs);
                    let offers = offers(state, front);
                    state.palette.open(offers);
                }
            }
            Command::Preferences => tabs::show(&mut self.tabs, Pane::Preferences),
            Command::Help => state.help.toggle(),
            Command::OpenRepository => state.actions.push(Action::PickFolder),
            Command::CloneRepository | Command::Home => tabs::show(&mut self.tabs, Pane::Home),
            Command::Profile => tabs::show(&mut self.tabs, Pane::Profile),
            Command::CloseTab => {
                if let Some(Pane::Repo(id)) = tabs::close_front(&mut self.tabs) {
                    state.actions.push(Action::Closed(id));
                }
            }
            Command::Tab(n) => tabs::show_nth(&mut self.tabs, n),
            Command::Theme(choice) => state.actions.push(Action::SetTheme(choice)),
            Command::OpenRecent(path) => state.actions.push(Action::Open(path)),
            Command::Repo(command) => {
                let Some(Pane::Repo(id)) = tabs::front(&self.tabs) else {
                    return;
                };
                match command {
                    RepoCommand::Undo => state.actions.push(Action::Undo(id, false)),
                    RepoCommand::Redo => state.actions.push(Action::Undo(id, true)),
                    RepoCommand::Refresh => state.actions.push(Action::Refresh(id)),
                    command => {
                        if let Some(repo) = state.repos.iter_mut().find(|repo| repo.id == id) {
                            repo.history_view.queue(command);
                        }
                    }
                }
            }
        }
    }

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
            Action::Clone(request) => state.start_clone(request),
            Action::CancelClone => state.home.clone.cancel(),
            Action::Open(path) => state.open(path),
            Action::Opened(repository) => self.show_repository(ctx, *repository),
            Action::Trust(path) => state.trust(path),
            Action::Forget(path) => state.settings.forget(&path),
            Action::ShowProfile => tabs::show(&mut self.tabs, Pane::Profile),
            Action::SaveProfile(changes) => state.save_profile(changes),
            Action::ShowPanel(id, panel) => state.show_panel(id, panel),
            Action::Refresh(id) => state.with_repo(id, |git, repo| repo.refresh(git)),
            Action::WriteCommitGraph(id) => {
                state.with_repo(id, |git, repo| repo.write_commit_graph(git));
            }
            Action::Stage(id, request) => state.with_repo(id, |git, repo| repo.stage(git, request)),
            Action::Output(id, asked) => {
                if let Some(repo) = state.repo_mut(id) {
                    repo.output_asked(asked);
                }
            }
            Action::Commit(id, commit) => state.with_repo(id, |git, repo| repo.commit(git, commit)),
            Action::Task(id, task) => state.with_repo(id, |git, repo| repo.run_task(git, task)),
            Action::Undo(id, redo) => state.with_repo(id, |git, repo| repo.undo(git, redo)),
            Action::CancelTask(id) => {
                if let Some(repo) = state.repo_mut(id) {
                    repo.tasks.cancel();
                }
            }
            Action::Closed(id) => state.repos.retain(|repo| repo.id != id),
            Action::SetTheme(choice) => {
                state.settings.theme = choice;
                theme::apply(ctx, choice);
            }
            Action::SetPreferences(preferences) => state.prefer(ctx, preferences),
            Action::ShowPreferences => tabs::show(&mut self.tabs, Pane::Preferences),
            Action::ShowPalette => self.run(Command::Palette),
            Action::ShowHelp => self.run(Command::Help),
            Action::SignIn(id, host) => {
                if let Some(repo) = state.repo_mut(id) {
                    repo.forge.sign_in(&host);
                }
            }
            Action::RefreshPullRequests(id) => {
                if let Some(repo) = state.repo_mut(id) {
                    repo.forge.refresh();
                }
            }
            Action::OpenUrl(url) => state.opener.open(url),
            Action::Scope(id, scope) => state.with_repo(id, |git, repo| repo.set_scope(git, scope)),
            Action::AskFiles {
                question,
                title,
                name,
            } => state.files.ask(question, title, name),
            Action::OpenPatches(id, files) => {
                if let Some(repo) = state.repo_mut(id) {
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
        let mut tab = RepoTab::new(id, repository, &git, ctx);
        tab.prefer(state.settings.preferences, Instant::now());
        state.repos.push(tab);
        tabs::show(&mut self.tabs, Pane::Repo(id));
    }
}

impl AppState {
    /// The repository tab `id`, if open.
    pub(crate) fn repo_mut(&mut self, id: RepoId) -> Option<&mut RepoTab> {
        self.repos.iter_mut().find(|repo| repo.id == id)
    }

    /// Does `act` with git and the repository tab `id`, when both are there.
    fn with_repo(&mut self, id: RepoId, act: impl FnOnce(&Git, &mut RepoTab)) {
        if let Some(git) = self.git()
            && let Some(repo) = self.repo_mut(id)
        {
            act(&git, repo);
        }
    }

    /// Shows repository `id`'s `panel` (its history, or a page about it).
    fn show_panel(&mut self, id: RepoId, panel: RepoPanel) {
        if let Some(repo) = self.repos.iter_mut().find(|repo| repo.id == id) {
            repo.panel = panel;
        }
    }

    /// Starts cloning as `request` says (once git is found).
    fn start_clone(&mut self, request: CloneRequest) {
        if let Some(git) = self.git() {
            self.home.clone.start(&git, request);
        }
    }

    /// Puts `preferences` in effect (every repository's background fetch too), and keeps them.
    fn prefer(&mut self, ctx: &egui::Context, preferences: Preferences) {
        self.settings.preferences = preferences;
        preferences.apply(ctx);
        let now = Instant::now();
        for repo in &mut self.repos {
            repo.tasks.fetch_every(preferences.fetch.interval(), now);
        }
    }

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
        crate::avatars::poll(ctx);
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
            // Back from a terminal, a hook may have been added.
            if regained {
                for repo in &mut self.repos {
                    repo.refresh_hooks(&git);
                }
            }
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
            FileQuestion::SavePicked { repo, commits, one } => self.actions.push(Action::Task(
                repo,
                Task::Export {
                    commits: commits.to_vec(),
                    how: if one {
                        Export::OnePatch(first)
                    } else {
                        Export::Patches(first)
                    },
                },
            )),
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
            repo.follow_submodules(git);
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
        crate::help::begin_frame(ui.ctx());
        self.state.poll(ui.ctx());
        // ⌘+ and ⌘− change the text's size: kept as preferred.
        let zoom = Preferences::zoom_of(ui.ctx());
        if zoom != self.state.settings.preferences.zoom {
            self.state.settings.preferences.zoom = zoom;
            self.state.settings.preferences.apply(ui.ctx());
        }
        let palette = Palette::of(ui.ctx());
        Panel::bottom("egit-status")
            .frame(
                Frame::new()
                    .fill(palette.panel)
                    .inner_margin(egui::Margin::symmetric(10, 4)),
            )
            .show(ui, |ui| status_bar::show(ui, &mut self.state));
        if let Some(command) = commands::pressed(ui.ctx()) {
            self.run(command);
        }
        palette::take_keys(ui.ctx(), &mut self.state.palette);
        CentralPanel::default()
            .frame(Frame::new().fill(palette.background))
            .show(ui, |ui| tabs::draw(&mut self.tabs, &mut self.state, ui));
        if let Some(command) = palette::show(ui.ctx(), &mut self.state.palette) {
            self.run(command);
        }
        self.state.prompts.show(ui.ctx());
        crate::help::window(ui.ctx(), &mut self.state.help);
        self.apply_actions(ui.ctx());
        pointing_hand(ui.ctx());
        crate::widgets::focus_ring(ui.ctx());
        crate::help::show(ui.ctx());
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

/// What the palette offers: the repository in front's commands (each saying why it can't run
/// now, if it can't), the application's commands, the recent repositories not open, and the
/// repository's branches to switch to.
fn offers(state: &mut AppState, front: Option<Pane>) -> Offers {
    let offer = |command: Command, allowed: Result<(), &'static str>, nouns: &str| Offer {
        label: command.label(nouns),
        keys: command.keys().map(crate::commands::keys_text),
        command,
        allowed,
    };
    let mut offers = Offers::default();
    let repo = match front {
        Some(Pane::Repo(id)) => state.repos.iter_mut().find(|repo| repo.id == id),
        _ => None,
    };
    if let Some(repo) = repo {
        let nouns = repo.forge.nouns();
        let has_forge = !repo.forge.repositories.is_empty();
        for command in Command::listed() {
            let Command::Repo(asked) = &command else {
                continue;
            };
            if *asked == RepoCommand::PullRequests && !has_forge {
                continue;
            }
            let allowed = repo.allows(asked);
            offers.commands.push(offer(command, allowed, nouns));
        }
        let current = repo.status.ok_ref().and_then(|status| match &status.head {
            egit_git::Head::Branch(branch) => Some(branch.name.clone()),
            egit_git::Head::Detached(_) => None,
        });
        offers.switching = repo.allows(&RepoCommand::Switch(bstr::BString::default()));
        offers.refs = repo.history.shown.refs().shared();
        offers.branches = offers
            .refs
            .iter()
            .zip(0_u32..)
            .filter(|(reference, _)| {
                reference.kind == egit_git::RefKind::Branch
                    && current
                        .as_ref()
                        .is_none_or(|current| current.as_slice() != &**reference.short_name())
            })
            .map(|(_, place)| place)
            .collect();
    }
    for command in Command::listed() {
        if !command.needs_repo() {
            offers
                .commands
                .push(offer(command, Ok(()), "pull requests"));
        }
    }
    let open: Vec<PathBuf> = state
        .repos
        .iter()
        .map(|repo| repo.repository.work_tree().to_path_buf())
        .collect();
    for recent in &state.settings.recent {
        let path = recent.path.to_path();
        if !open.contains(&path) {
            offers
                .commands
                .push(offer(Command::OpenRecent(path), Ok(()), "pull requests"));
        }
    }
    offers
}

/// The pointer over anything enabled that a click acts on shows a hand, as links do: egui gives
/// its buttons one (`interact_cursor`), but not checkboxes, tabs, rows, or badges. A text field
/// keeps its text cursor, and a widget that chose another cursor keeps that.
fn pointing_hand(ctx: &egui::Context) {
    if ctx.output(|output| output.cursor_icon) != egui::CursorIcon::Default {
        return;
    }
    let hovered: Vec<egui::Id> =
        ctx.interaction_snapshot(|snapshot| snapshot.hovered.iter().copied().collect());
    let clickable = hovered.into_iter().any(|id| {
        ctx.read_response(id)
            .is_some_and(|response| response.enabled() && response.sense.senses_click())
    });
    if clickable {
        ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
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
