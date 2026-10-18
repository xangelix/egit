//! The dialog that opens a pull request (GitHub's; a merge request, GitLab's) from a pushed
//! branch: into its own repository, or the
//! one it was forked from (the default when it is a fork), against a base branch (that
//! repository's default first), titled and described from the branch's commit when it has one
//! (its name otherwise), as a draft or not. What it starts is a task.

use std::sync::Arc;

use bstr::{BString, ByteSlice as _};
use egit_forge::{ForgeRepository, NewPullRequest, repository_info};

use crate::forge::request_noun;
use egit_git::{CancelToken, Commit, ObjectKind, commits_between};
use egui::{Id, Modal, Ui};
use egui_async::Bind;

use super::history::Shared;
use crate::history::Objects;
use crate::jobs;
use crate::problem::Problem;
use crate::tasks::{PullRequestToOpen, Task};
use crate::theme::Palette;
use crate::widgets::{self, muted};

/// What the dialog works out before it is filled in.
#[derive(Debug)]
struct Prepared {
    /// The repositories it may go into: the branch's own, then the one it was forked from.
    targets: Vec<(ForgeRepository, Option<String>)>,
    /// The title and description the branch's commits suggest.
    title: String,
    body: String,
}

/// The dialog's state.
#[derive(Debug)]
pub(super) struct PullRequestForm {
    /// The local branch.
    branch: BString,
    /// The remote it is pushed to, its branch there, and the repository that remote is.
    remote: BString,
    head_branch: String,
    head: ForgeRepository,
    prepared: Bind<Arc<Prepared>, Problem>,
    started: bool,
    /// Which target, and what is typed.
    target: usize,
    base: String,
    title: String,
    body: String,
    draft: bool,
    /// Whether the suggestions were taken in yet.
    filled: bool,
}

impl PullRequestForm {
    /// The dialog for `branch`, pushed to `remote`'s `head_branch`, which is `head`.
    pub(super) const fn new(
        branch: BString,
        remote: BString,
        head_branch: String,
        head: ForgeRepository,
    ) -> Self {
        Self {
            branch,
            remote,
            head_branch,
            head,
            prepared: Bind::new(true),
            started: false,
            target: 0,
            base: String::new(),
            title: String::new(),
            body: String::new(),
            draft: false,
            filled: false,
        }
    }

    /// The title as it stands.
    #[cfg(test)]
    pub(super) fn title(&self) -> String {
        self.title.clone()
    }

    /// Works out the targets and suggestions, once.
    fn prepare(&mut self, shared: &Shared<'_>, objects: &Objects) {
        if self.started {
            return;
        }
        self.started = true;
        let (Some(git), Some(account)) = (shared.git, shared.forge.account(self.head.host()))
        else {
            return;
        };
        let (git, repository, objects) =
            (git.clone(), Arc::clone(shared.repository), objects.clone());
        let (head, remote, branch) = (self.head.clone(), self.remote.clone(), self.branch.clone());
        self.prepared.request(jobs::blocking(move || {
            let info = repository_info(&account, &head, &CancelToken::new()).map_err(|error| {
                Problem::new(
                    "The repository's details couldn't be read",
                    error.to_string(),
                )
            })?;
            let mut targets = vec![(head.clone(), info.default_branch.clone())];
            if let Some((parent, default)) = info.parent {
                // A fork's requests usually go to the repository it came from.
                targets.insert(0, (parent, default));
            }
            let base = targets
                .first()
                .and_then(|(_, default)| default.clone())
                .unwrap_or_else(|| "main".to_owned());
            let (title, body) = suggested(&git, &repository, &objects, &remote, &base, &branch);
            Ok(Arc::new(Prepared {
                targets,
                title,
                body,
            }))
        }));
    }
}

/// The title and description a branch's commits suggest: its one commit's subject and body
/// when it brings in one, its name otherwise.
fn suggested(
    git: &egit_git::Git,
    repository: &egit_git::Repository,
    objects: &Objects,
    remote: &BString,
    base: &str,
    branch: &BString,
) -> (String, String) {
    let named = || (humanized(&branch.to_str_lossy()), String::new());
    let base_ref = format!("refs/remotes/{}/{base}", remote.to_str_lossy());
    let mut tip = BString::from("refs/heads/");
    tip.extend_from_slice(branch);
    let Ok(commits) = commits_between(
        git,
        repository,
        base_ref.as_bytes().as_bstr(),
        tip.as_bstr(),
        2,
        &CancelToken::new(),
    ) else {
        return named();
    };
    let [only] = commits.as_slice() else {
        return named();
    };
    let mut message = None;
    let read = objects.read(git, repository.work_tree(), &[*only], |_, object| {
        message = object
            .filter(|object| object.kind == ObjectKind::Commit)
            .and_then(|object| Commit::parse(object.data).ok())
            .map(|commit| commit.message.to_str_lossy().into_owned());
    });
    match (read, message) {
        (Ok(()), Some(message)) => {
            let (subject, body) = message.split_once("\n\n").unwrap_or((&message, ""));
            (subject.trim().replace('\n', " "), body.trim().to_owned())
        }
        _ => named(),
    }
}

/// A branch's name as a title: `fix-login_bug` → `Fix login bug`.
fn humanized(branch: &str) -> String {
    let last = branch.rsplit('/').next().unwrap_or(branch);
    let words = last.replace(['-', '_'], " ");
    let mut characters = words.trim().chars();
    characters.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(characters).collect()
    })
}

/// Shows the dialog while there is one; returns the task it starts.
pub(super) fn show(
    ctx: &egui::Context,
    shared: &Shared<'_>,
    objects: &Objects,
    form: &mut Option<PullRequestForm>,
) -> Option<Task> {
    let open = form.as_mut()?;
    open.prepare(shared, objects);
    let palette = Palette::of(ctx);
    let frame = egui::Frame::new()
        .fill(palette.surface)
        .stroke(egui::Stroke::new(1.0, palette.border))
        .corner_radius(egui::CornerRadius::same(crate::theme::RADIUS_CARD))
        .inner_margin(egui::Margin::same(20));
    let shown = Modal::new(Id::new(("pull-request-form", shared.id)))
        .frame(frame)
        .show(ctx, |ui| {
            ui.set_width(560.0);
            fill(ui, shared, open)
        });
    let answer = if shown.should_close() {
        Some(Answer::Cancel)
    } else {
        shown.inner
    };
    let answer = answer?;
    *form = None;
    match answer {
        Answer::Create(task) => Some(task),
        Answer::Cancel => None,
    }
}

/// How the form was answered.
enum Answer {
    Create(Task),
    Cancel,
}

/// The repositories it may go into (the branch's own and the one it was forked from), each
/// picking its default branch as the base.
fn targets(ui: &mut Ui, targets: &[(ForgeRepository, Option<String>)], form: &mut PullRequestForm) {
    ui.label(muted(ui, "Into"));
    for (index, (target, default)) in targets.iter().enumerate() {
        if crate::widgets::radio_of(
            ui,
            form.target == index,
            target.full_name(),
            "pull-request-target",
        )
        .clicked()
        {
            form.target = index;
            if let Some(default) = default {
                form.base.clone_from(default);
            }
        }
    }
    ui.add_space(6.0);
}

/// The form; its answer, once given.
fn fill(ui: &mut Ui, shared: &Shared<'_>, form: &mut PullRequestForm) -> Option<Answer> {
    let noun = request_noun(form.head.host().kind());
    widgets::panel_title(ui, format!("Create a {noun}"));
    ui.add_space(8.0);
    ui.add(
        egui::Label::new(format!(
            "From {} (as pushed to {}/{}).",
            form.branch.to_str_lossy(),
            form.remote.to_str_lossy(),
            form.head_branch
        ))
        .wrap(),
    );
    ui.add_space(8.0);
    let prepared = match form.prepared.read_as_ref() {
        None => {
            widgets::working(ui, "Reading the repository…");
            None
        }
        Some(Err(problem)) => {
            let problem = problem.clone();
            widgets::problem_box(ui, &problem, |_| {});
            None
        }
        Some(Ok(prepared)) => Some(Arc::clone(prepared)),
    };
    if let Some(prepared) = &prepared
        && !form.filled
    {
        form.filled = true;
        form.title.clone_from(&prepared.title);
        form.body.clone_from(&prepared.body);
        form.base = prepared
            .targets
            .first()
            .and_then(|(_, default)| default.clone())
            .unwrap_or_default();
    }
    let mut answer = None;
    if let Some(prepared) = &prepared {
        if prepared.targets.len() > 1 {
            targets(ui, &prepared.targets, form);
        }
        field(ui, "Base branch", &mut form.base, false);
        field(ui, "Title", &mut form.title, true);
        let label = ui.label(muted(ui, "Description"));
        crate::widgets::bounded_text_area(
            ui,
            crate::widgets::text_area(&mut form.body)
                .desired_rows(6)
                .desired_width(f32::INFINITY),
            260.0,
            "pull-request-body",
        )
        .labelled_by(label.id);
        crate::widgets::checkbox(ui, &mut form.draft, "Draft");
        ui.add_space(12.0);
    }
    let into = prepared
        .as_ref()
        .and_then(|prepared| prepared.targets.get(form.target))
        .map(|(target, _)| target.clone());
    let ready = into.is_some() && !form.title.trim().is_empty() && !form.base.trim().is_empty();
    widgets::buttons_row(ui, |ui| {
        if widgets::primary(ui, ready && !shared.busy, &format!("Create {noun}")).clicked()
            && let Some(into) = &into
        {
            let head = (!into.is(&form.head)).then(|| form.head.clone());
            answer = Some(Answer::Create(Task::CreatePullRequest(Box::new(
                PullRequestToOpen {
                    repository: into.clone(),
                    new: NewPullRequest {
                        title: form.title.trim().to_owned(),
                        body: form.body.trim().to_owned(),
                        head_branch: form.head_branch.clone(),
                        head,
                        base_branch: form.base.trim().to_owned(),
                        draft: form.draft,
                    },
                },
            ))));
        }
        if widgets::secondary(ui, true, "Cancel").clicked() {
            answer = Some(Answer::Cancel);
        }
    });
    answer
}

/// A labelled one-line field.
fn field(ui: &mut Ui, label: &str, text: &mut String, focus: bool) {
    let label = ui.label(muted(ui, label));
    let response = ui
        .add(crate::widgets::line_field(text).desired_width(f32::INFINITY))
        .labelled_by(label.id);
    if focus && ui.memory(|memory| memory.focused().is_none()) {
        response.request_focus();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_branch_s_name_reads_as_a_title() {
        assert_eq!(humanized("fix-login_bug"), "Fix login bug");
        assert_eq!(humanized("ada/better-errors"), "Better errors");
        assert_eq!(humanized(""), "");
    }
}
