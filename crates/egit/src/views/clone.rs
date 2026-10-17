//! Cloning a repository, from Home: its address, the folder it goes into (typed, or picked in
//! the desktop's dialog), the name of the folder made there (the address's last part until
//! changed), whether submodules come too, and whether files' contents wait until they are
//! needed (a partial clone). While git works its progress shows, with Cancel; once done the
//! repository opens, and the next clone goes into the same folder.

use std::path::{Path, PathBuf};

use egit_git::{CancelToken, CloneOptions, Git, GitError, Progress, clone};
use egui::{Align, Key, Layout, RichText, TextEdit, Ui, vec2};
use egui_async::Bind;

use crate::app::Action;
use crate::format::{self, display_path};
use crate::jobs;
use crate::problem::Problem;
use crate::theme::{Palette, icon};
use crate::widgets::{self, muted};

/// A clone asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CloneRequest {
    /// Where the repository is.
    pub(crate) url: String,
    /// The folder made for it (absolute).
    pub(crate) destination: PathBuf,
    pub(crate) options: CloneOptions,
}

/// The clone card, and the clone running.
pub(crate) struct CloneView {
    url: String,
    /// The name of the folder made, as the address gives it until edited.
    name: String,
    name_edited: bool,
    /// The folder it goes into, as typed (`~` is the home folder; a relative path is in it)…
    folder_text: String,
    /// …or as picked in the dialog or remembered, exactly (a path needn't be UTF-8), until the
    /// text is edited.
    folder: Option<PathBuf>,
    options: CloneOptions,
    /// The folder dialog, while it is open.
    pub(crate) picking: Bind<Option<PathBuf>, Problem>,
    /// The clone running: the folder made, or `None` once cancelled.
    pub(crate) cloning: Bind<Option<PathBuf>, Problem>,
    progress: Progress,
    cancel: CancelToken,
    /// Why the last clone failed, until dismissed.
    pub(crate) problem: Option<Problem>,
    /// Whether the last clone was cancelled (said until the next starts).
    pub(crate) cancelled: bool,
}

impl CloneView {
    /// The card, its folder the one the last clone went into (or the home folder).
    pub(crate) fn new(folder: Option<PathBuf>, home: Option<&Path>) -> Self {
        let folder_text = match (&folder, home) {
            (Some(folder), home) => display_path(folder, home),
            (None, Some(_)) => "~".to_owned(),
            (None, None) => String::new(),
        };
        Self {
            url: String::new(),
            name: String::new(),
            name_edited: false,
            folder_text,
            folder,
            options: CloneOptions::default(),
            picking: Bind::new(true),
            cloning: Bind::new(true),
            progress: Progress::new(),
            cancel: CancelToken::new(),
            problem: None,
            cancelled: false,
        }
    }

    /// Opens the desktop's folder dialog.
    pub(crate) fn pick_folder(&mut self) {
        if self.picking.is_pending() {
            return;
        }
        self.picking.request(async {
            let folder = rfd::AsyncFileDialog::new()
                .set_title("Clone into")
                .pick_folder()
                .await;
            Ok(folder.map(|folder| folder.path().to_owned()))
        });
    }

    /// Takes the folder picked in the dialog.
    pub(crate) fn picked(&mut self, folder: PathBuf, home: Option<&Path>) {
        self.folder_text = display_path(&folder, home);
        self.folder = Some(folder);
    }

    /// Starts cloning (one clone at a time).
    pub(crate) fn start(&mut self, git: &Git, request: CloneRequest) {
        if self.cloning.is_pending() {
            return;
        }
        self.problem = None;
        self.cancelled = false;
        self.progress = Progress::new();
        self.cancel = CancelToken::new();
        let (git, progress, cancel) = (git.clone(), self.progress.clone(), self.cancel.clone());
        self.cloning.request(jobs::blocking(move || {
            let CloneRequest {
                url,
                destination,
                options,
            } = request;
            match clone(&git, &url, &destination, options, &progress, &cancel) {
                Ok(()) => Ok(Some(destination)),
                // Git removed what it had made.
                Err(GitError::Cancelled { .. }) => Ok(None),
                Err(error) => Err(Problem::git("The repository couldn't be cloned", &error)),
            }
        }));
    }

    /// Stops the clone running.
    pub(crate) fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Clears the address and name once a clone is done (the folder stays for the next).
    pub(crate) fn done(&mut self) {
        self.url.clear();
        self.name.clear();
        self.name_edited = false;
    }

    /// Where the clone goes: the folder (a relative one is in the home folder) and the name.
    fn destination(&self, home: Option<&Path>) -> Option<PathBuf> {
        let name = self.name.trim();
        if !is_folder_name(name) {
            return None;
        }
        let folder = if let Some(folder) = &self.folder {
            folder.clone()
        } else {
            let typed = self.folder_text.trim();
            if typed.is_empty() {
                return None;
            }
            let folder = super::home::expand_home(typed, home);
            if folder.is_absolute() {
                folder
            } else {
                home?.join(folder)
            }
        };
        Some(folder.join(name))
    }
}

impl Drop for CloneView {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// The folder a clone of `url` makes, as git names it: the address's last part without `.git`
/// (or `.bundle`), whether it is a URL, `host:path`, or a path; empty when there is none.
pub(crate) fn name_from_url(url: &str) -> String {
    let url = url.trim().trim_end_matches('/');
    // `repo/.git` names the repository around it.
    let url = url
        .strip_suffix("/.git")
        .unwrap_or(url)
        .trim_end_matches('/');
    let last = url.rsplit(['/', ':']).next().unwrap_or(url);
    let name = last
        .strip_suffix(".git")
        .or_else(|| last.strip_suffix(".bundle"))
        .unwrap_or(last);
    name.trim().to_owned()
}

/// Whether `name` can be one folder's name.
fn is_folder_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\0'])
}

/// Draws the clone card.
pub(crate) fn card(
    ui: &mut Ui,
    view: &mut CloneView,
    ready: bool,
    home: Option<&Path>,
    actions: &mut Vec<Action>,
) {
    widgets::card(ui, |ui| {
        widgets::heading(ui, "Clone a repository");
        ui.label(muted(
            ui,
            "From an address (https:// or ssh), or a path on this computer.",
        ));
        ui.add_space(8.0);
        let busy = view.cloning.is_pending();
        let submitted = ui
            .add_enabled_ui(!busy, |ui| fields(ui, view, actions))
            .inner;
        ui.add_space(8.0);
        let destination = view.destination(home);
        if busy {
            // The progress moves on its own, without input.
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(muted(
                    ui,
                    format::progress("Cloning", &view.progress.report()),
                ));
                if widgets::secondary(ui, true, "Cancel").clicked() {
                    actions.push(Action::CancelClone);
                }
            });
        } else {
            ui.horizontal(|ui| {
                let can = ready && !view.url.trim().is_empty() && destination.is_some();
                let clicked =
                    widgets::primary(ui, can, &format!("{}  Clone", icon::DOWNLOAD_SIMPLE))
                        .clicked();
                if (clicked || submitted)
                    && can
                    && let Some(destination) = &destination
                {
                    actions.push(Action::Clone(CloneRequest {
                        url: view.url.trim().to_owned(),
                        destination: destination.clone(),
                        options: view.options,
                    }));
                }
                match &destination {
                    Some(destination) => {
                        ui.label(muted(
                            ui,
                            format!("Into {}", display_path(destination, home)),
                        ));
                    }
                    None if !view.url.trim().is_empty() => {
                        ui.label(muted(ui, "Name the folder it goes into."));
                    }
                    None => {}
                }
            });
        }
        if view.cancelled {
            ui.label(muted(ui, "Cancelled: git removed what it had made."));
        }
        if let Some(problem) = view.problem.clone() {
            ui.add_space(8.0);
            widgets::problem_box(ui, &problem, |ui| {
                if widgets::secondary(ui, true, "Dismiss").clicked() {
                    view.problem = None;
                }
            });
        }
    });
}

/// The address, folder, name, and options; returns whether Enter was pressed in the address.
fn fields(ui: &mut Ui, view: &mut CloneView, actions: &mut Vec<Action>) -> bool {
    let palette = Palette::of(ui.ctx());
    let width = (ui.available_width() - 90.0).max(200.0);
    let mut submitted = false;
    egui::Grid::new("clone-fields")
        .num_columns(2)
        .spacing([12.0, 8.0])
        .show(ui, |ui| {
            let label = ui.label(muted(ui, "Address"));
            let field = ui
                .add_sized(
                    [width, 20.0],
                    TextEdit::singleline(&mut view.url)
                        .hint_text("https://github.com/owner/repository.git"),
                )
                .labelled_by(label.id);
            if field.changed() && !view.name_edited {
                view.name = name_from_url(&view.url);
            }
            submitted = field.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter));
            ui.end_row();

            let label = ui.label(muted(ui, "Into"));
            // From the right, so the field ends where the address does.
            let row = Layout::right_to_left(Align::Center);
            ui.allocate_ui_with_layout(vec2(width, 20.0), row, |ui| {
                let picking = view.picking.is_pending();
                if widgets::secondary(ui, !picking, &format!("{}  Choose…", icon::FOLDER_OPEN))
                    .clicked()
                {
                    actions.push(Action::PickCloneFolder);
                }
                let field = ui
                    .add_sized(
                        [ui.available_width(), 20.0],
                        TextEdit::singleline(&mut view.folder_text).hint_text("~/src"),
                    )
                    .labelled_by(label.id);
                if field.changed() {
                    view.folder = None;
                }
            });
            ui.end_row();

            let label = ui.label(muted(ui, "Named"));
            let field = ui
                .add_sized(
                    [width.min(320.0), 20.0],
                    TextEdit::singleline(&mut view.name).hint_text("repository"),
                )
                .labelled_by(label.id);
            if field.changed() {
                view.name_edited = !view.name.is_empty();
            }
            ui.end_row();
        });
    ui.add_space(4.0);
    ui.checkbox(&mut view.options.submodules, "Its submodules too");
    ui.checkbox(
        &mut view.options.partial,
        "Partial clone: files' contents are fetched when first needed",
    );
    if view.options.partial {
        ui.label(
            RichText::new(
                "Faster to clone a large history; looking at old files then needs the remote.",
            )
            .small()
            .color(palette.muted),
        );
    }
    submitted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clone_is_named_as_git_names_it() {
        for (url, name) in [
            ("https://github.com/owner/egit.git", "egit"),
            ("https://github.com/owner/egit", "egit"),
            ("https://github.com/owner/egit/", "egit"),
            ("git@github.com:owner/egit.git", "egit"),
            ("host:egit.git", "egit"),
            ("ssh://me@host:2222/~me/egit.git", "egit"),
            ("file:///srv/git/origin.git", "origin"),
            ("/srv/git/project/.git", "project"),
            ("/srv/git/project.git/", "project"),
            ("../work.bundle", "work"),
            ("  https://host/a/b  ", "b"),
            ("", ""),
        ] {
            assert_eq!(name_from_url(url), name, "{url}");
        }
    }

    #[test]
    fn it_goes_into_the_folder_under_its_name() {
        let home = Some(Path::new("/home/me"));
        let mut view = CloneView::new(None, home);
        view.name = "egit".to_owned();
        assert_eq!(view.destination(home), Some(PathBuf::from("/home/me/egit")));
        view.folder_text = "~/src".to_owned();
        assert_eq!(
            view.destination(home),
            Some(PathBuf::from("/home/me/src/egit"))
        );
        // A relative folder is in the home folder (egit's own working folder means nothing).
        view.folder_text = "src".to_owned();
        assert_eq!(
            view.destination(home),
            Some(PathBuf::from("/home/me/src/egit"))
        );
        assert_eq!(view.destination(None), None);
        // A folder picked is used exactly.
        view.picked(PathBuf::from("/srv"), home);
        assert_eq!(view.folder_text, "/srv");
        assert_eq!(view.destination(home), Some(PathBuf::from("/srv/egit")));
        for name in ["", ".", "..", "a/b"] {
            view.name = name.to_owned();
            assert_eq!(view.destination(home), None, "{name:?}");
        }
        // The folder remembered from the last clone.
        let view = CloneView::new(Some(PathBuf::from("/home/me/src")), home);
        assert_eq!(view.folder_text, "~/src");
    }
}
