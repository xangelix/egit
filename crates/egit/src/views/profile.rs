//! The Profile tab: who you are to git, how you sign, how git gets your credentials, who you
//! are signed in as on GitHub and GitLab (signing in and out) and the hosts pull and merge
//! requests come from, and how pull and push behave, each with where it is set.
//! Identity, signing, and the listed GitHub and GitLab hosts can be changed here (in your global
//! configuration).

use std::path::Path;
use std::sync::Arc;

use bstr::ByteSlice as _;

use egit_forge::{ForgeHost, ForgeHosts, ForgeKind};
use egit_git::{
    ConfigEntry, ConfigScope, ConfigSnapshot, PullFastForward, PullRebase, SigningFormat,
};
use egui::{Grid, RichText, Ui};
use egui_async::Bind;
use egui_table_kit::table::TableKit;

use crate::app::{Action, AppState, ConfigChange};
use crate::forge::{GITHUB_HOSTS_KEY, GITLAB_HOSTS_KEY};
use crate::problem::Problem;
use crate::tables::{ConfigRows, ROW_HEIGHT, SnapshotTable};
use crate::theme::Weighted as _;
use crate::theme::{Palette, icon};
use crate::widgets::{self, muted};

/// The size of the profile's text fields (a grid would otherwise size them to their text).
const FIELD: [f32; 2] = [340.0, 26.0];

/// The Profile tab's own state.
pub(crate) struct ProfileView {
    /// Saving changes, while it runs.
    pub(crate) saving: Bind<(), Problem>,
    /// Why the last save failed.
    pub(crate) problem: Option<Problem>,
    form: Form,
    /// What the settings the form was filled from say.
    saved: Form,
    /// The settings the form was filled from.
    filled_from: Option<Arc<ConfigSnapshot>>,
    table: SnapshotTable,
    /// A GitHub Enterprise host being typed, to add; and a self-managed GitLab one.
    new_github_host: String,
    new_gitlab_host: String,
}

impl Default for ProfileView {
    fn default() -> Self {
        Self {
            saving: Bind::new(true),
            problem: None,
            form: Form::default(),
            saved: Form::default(),
            filled_from: None,
            table: SnapshotTable::new("profile-config".to_owned()),
            new_github_host: String::new(),
            new_gitlab_host: String::new(),
        }
    }
}

/// What can be edited, as it stands in the form.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Form {
    name: String,
    email: String,
    format: SigningFormat,
    key: String,
    sign_commits: bool,
    sign_tags: bool,
}

impl Default for Form {
    fn default() -> Self {
        Self {
            name: String::new(),
            email: String::new(),
            format: SigningFormat::OpenPgp,
            key: String::new(),
            sign_commits: false,
            sign_tags: false,
        }
    }
}

impl Form {
    fn from_snapshot(snapshot: &ConfigSnapshot) -> Self {
        let profile = snapshot.profile();
        let text = |entry: Option<ConfigEntry<'_>>| {
            entry
                .map(|entry| entry.text().to_string())
                .unwrap_or_default()
        };
        let signing = profile.signing();
        Self {
            name: text(profile.name()),
            email: text(profile.email()),
            format: signing.format.value,
            key: text(signing.key),
            sign_commits: signing.commits.value,
            sign_tags: signing.tags.value,
        }
    }

    /// What must change in the global configuration to match the form.
    fn changes(&self, saved: &Self) -> Vec<ConfigChange> {
        let mut changes = Vec::new();
        let text = |value: &str| {
            let value = value.trim();
            (!value.is_empty()).then(|| value.to_owned())
        };
        if self.name.trim() != saved.name.trim() {
            changes.push(ConfigChange::Set {
                key: "user.name".into(),
                value: text(&self.name),
            });
        }
        if self.email.trim() != saved.email.trim() {
            changes.push(ConfigChange::Set {
                key: "user.email".into(),
                value: text(&self.email),
            });
        }
        if self.format != saved.format {
            changes.push(ConfigChange::Set {
                key: "gpg.format".into(),
                value: Some(self.format.name().to_owned()),
            });
        }
        if self.key.trim() != saved.key.trim() {
            changes.push(ConfigChange::Set {
                key: "user.signingKey".into(),
                value: text(&self.key),
            });
        }
        if self.sign_commits != saved.sign_commits {
            changes.push(ConfigChange::Set {
                key: "commit.gpgSign".into(),
                value: Some(self.sign_commits.to_string()),
            });
        }
        if self.sign_tags != saved.sign_tags {
            changes.push(ConfigChange::Set {
                key: "tag.gpgSign".into(),
                value: Some(self.sign_tags.to_string()),
            });
        }
        changes
    }

    /// Why the form can't be saved as it is.
    fn problem(&self) -> Option<&'static str> {
        let has =
            |text: &str, bad: &[char]| text.chars().any(|c| bad.contains(&c) || c.is_control());
        if has(&self.name, &['<', '>']) {
            return Some("A name can't contain < or > or line breaks.");
        }
        if has(&self.email, &['<', '>', ' ']) {
            return Some("An email can't contain spaces, < or >, or line breaks.");
        }
        if has(&self.key, &[]) {
            return Some("A signing key can't contain line breaks.");
        }
        None
    }
}

/// Draws the Profile tab.
pub(crate) fn show(ui: &mut Ui, state: &mut AppState) {
    super::page(ui, "profile", |ui| {
        widgets::page_title(ui, "Your git profile");
        ui.label(muted(
            ui,
            "The settings git uses for you everywhere, read from git itself. Changes are saved to \
             your global configuration; a repository's own settings can override them.",
        ));
        ui.add_space(16.0);

        let snapshot = match state.user_config.read_as_ref() {
            None => {
                widgets::working(ui, "Reading your git settings…");
                return;
            }
            Some(Err(problem)) => {
                let problem = problem.clone();
                widgets::problem_box(ui, &problem, |_| {});
                return;
            }
            Some(Ok(snapshot)) => Arc::clone(snapshot),
        };
        let home = state.home_dir.clone();
        let view = &mut state.profile;
        let fresh = !view
            .filled_from
            .as_ref()
            .is_some_and(|filled| Arc::ptr_eq(filled, &snapshot));
        if fresh {
            let saved = Form::from_snapshot(&snapshot);
            // New settings refill the form, unless it holds edits not yet saved.
            if view.form == view.saved {
                view.form = saved.clone();
            }
            view.saved = saved;
            view.filled_from = Some(Arc::clone(&snapshot));
        }

        identity_card(ui, view, &snapshot, home.as_deref());
        ui.add_space(12.0);
        signing_card(ui, view, &snapshot, home.as_deref());
        ui.add_space(8.0);
        save_row(ui, view, &mut state.actions);
        ui.add_space(12.0);
        let git = state.git();
        forges_card(
            ui,
            (&mut state.profile, &mut state.sign_ins),
            (&snapshot, git.as_ref()),
            home.as_deref(),
            &mut state.actions,
        );
        ui.add_space(12.0);
        // Where no helper is set up, the ones installed that could be; and how setting one up
        // went, which stays once it is.
        let keeping = git
            .as_ref()
            .filter(|_| {
                snapshot.profile().credential_helpers().is_empty() || state.sign_ins.has_kept()
            })
            .map(|git| state.sign_ins.keeping(git));
        credentials_card(
            ui,
            (&snapshot, keeping.as_ref()),
            home.as_deref(),
            &mut state.actions,
        );
        ui.add_space(12.0);
        pull_push_card(ui, &snapshot, home.as_deref());
        ui.add_space(12.0);
        all_settings_card(ui, &mut state.profile.table, &snapshot, home.as_deref());
    });
}

fn identity_card(
    ui: &mut Ui,
    view: &mut ProfileView,
    snapshot: &ConfigSnapshot,
    home: Option<&Path>,
) {
    let profile = snapshot.profile();
    widgets::card(ui, |ui| {
        widgets::heading(ui, "Identity");
        ui.label(muted(
            ui,
            "Every commit you make records this name and email.",
        ));
        ui.add_space(8.0);
        Grid::new("profile-identity")
            .num_columns(3)
            .min_col_width(widgets::FACT_LABEL)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                let label = ui.label("Name");
                ui.add_sized(
                    FIELD,
                    crate::widgets::line_field(&mut view.form.name)
                        .hint_text("Your Name")
                        .vertical_align(egui::Align::Center),
                )
                .labelled_by(label.id);
                widgets::origin(ui, profile.name(), home);
                ui.end_row();

                let label = ui.label("Email");
                ui.add_sized(
                    FIELD,
                    crate::widgets::line_field(&mut view.form.email)
                        .hint_text("you@example.com")
                        .vertical_align(egui::Align::Center),
                )
                .labelled_by(label.id);
                widgets::origin(ui, profile.email(), home);
                ui.end_row();

                for entry in profile.identity_overrides() {
                    ui.label(muted(ui, entry.key.to_string()));
                    ui.label(entry.text().to_string());
                    widgets::origin(ui, Some(entry), home);
                    ui.end_row();
                }
            });
        if profile.use_config_only().value {
            ui.label(muted(
                ui,
                "git won't guess an identity you haven't set (user.useConfigOnly).",
            ));
        }
    });
}

fn signing_card(
    ui: &mut Ui,
    view: &mut ProfileView,
    snapshot: &ConfigSnapshot,
    home: Option<&Path>,
) {
    let profile = snapshot.profile();
    let signing = profile.signing();
    widgets::card(ui, |ui| {
        widgets::heading(ui, "Signing");
        ui.label(muted(
            ui,
            "git signs with your own tools: gpg for OpenPGP and X.509, ssh-keygen for SSH keys.",
        ));
        ui.add_space(8.0);
        Grid::new("profile-signing")
            .num_columns(3)
            .min_col_width(widgets::FACT_LABEL)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label("Format");
                ui.horizontal(|ui| {
                    for (format, label) in [
                        (SigningFormat::OpenPgp, "OpenPGP"),
                        (SigningFormat::Ssh, "SSH"),
                        (SigningFormat::X509, "X.509"),
                    ] {
                        crate::widgets::radio_value(ui, &mut view.form.format, format, label);
                    }
                });
                widgets::origin(ui, signing.format.entry, home);
                ui.end_row();

                let label = ui.label("Key");
                let hint = match view.form.format {
                    SigningFormat::OpenPgp => "A key id, or empty: gpg picks one by your email",
                    SigningFormat::Ssh => "~/.ssh/id_ed25519.pub, or a key itself",
                    SigningFormat::X509 => "A certificate id",
                };
                ui.add_sized(
                    FIELD,
                    crate::widgets::line_field(&mut view.form.key)
                        .hint_text(hint)
                        .vertical_align(egui::Align::Center),
                )
                .labelled_by(label.id);
                widgets::origin(ui, signing.key, home);
                ui.end_row();

                ui.label("Commits");
                crate::widgets::checkbox(ui, &mut view.form.sign_commits, "Sign every commit");
                widgets::origin(ui, signing.commits.entry, home);
                ui.end_row();

                ui.label("Tags");
                crate::widgets::checkbox(ui, &mut view.form.sign_tags, "Sign every annotated tag");
                widgets::origin(ui, signing.tags.entry, home);
                ui.end_row();

                if let Some(program) = signing.program {
                    ui.label("Program");
                    ui.label(program.text().to_string());
                    widgets::origin(ui, Some(program), home);
                    ui.end_row();
                }
                if view.form.format == SigningFormat::Ssh {
                    ui.label("Trusted signers");
                    if let Some(entry) = signing.allowed_signers {
                        ui.label(entry.text().to_string());
                        widgets::origin(ui, Some(entry), home);
                    } else {
                        ui.label(muted(ui, "Not set: SSH signatures can't be verified."));
                        ui.label("");
                    }
                    ui.end_row();
                }
            });
    });
}

fn save_row(ui: &mut Ui, view: &mut ProfileView, actions: &mut Vec<Action>) {
    let palette = Palette::of(ui.ctx());
    let changes = view.form.changes(&view.saved);
    let invalid = view.form.problem();
    let saving = view.saving.is_pending();
    ui.horizontal(|ui| {
        let can_save = !changes.is_empty() && invalid.is_none() && !saving;
        let save = widgets::primary(ui, can_save, &format!("{}  Save", icon::FLOPPY_DISK));
        crate::help::note_topic(ui, save.rect, "profile-save");
        if save.clicked() {
            actions.push(Action::SaveProfile(changes.clone()));
        }
        let revert = widgets::secondary(ui, !changes.is_empty() && !saving, "Revert");
        crate::help::note_topic(ui, revert.rect, "profile-save");
        if revert.clicked() {
            view.form = view.saved.clone();
        }
        if saving {
            ui.spinner();
        } else if let Some(problem) = invalid {
            ui.label(RichText::new(problem).color(palette.danger));
        } else if !changes.is_empty() {
            ui.label(muted(ui, "Unsaved changes."));
        }
    });
    if let Some(problem) = view.problem.clone() {
        ui.add_space(6.0);
        widgets::problem_box(ui, &problem, |ui| {
            if widgets::secondary(ui, true, "Dismiss").clicked() {
                view.problem = None;
            }
        });
    }
}

/// The credential helpers git asks (`keeping`: where none is set up, those that could be).
fn credentials_card(
    ui: &mut Ui,
    (snapshot, keeping): (&ConfigSnapshot, Option<&crate::sign_ins::Keeping>),
    home: Option<&Path>,
    actions: &mut Vec<Action>,
) {
    let profile = snapshot.profile();
    widgets::card(ui, |ui| {
        widgets::heading(ui, "Credentials");
        let helpers = profile.credential_helpers();
        if helpers.is_empty() {
            ui.add(
                egui::Label::new(muted(
                    ui,
                    "No credential helper: git asks for passwords each time, and egit relays the \
                     question; a sign-in to GitHub or GitLab lasts until egit quits. Set one up \
                     to keep them:",
                ))
                .wrap(),
            );
            if let Some(keeping) = keeping {
                ui.add_space(6.0);
                crate::views::sign_in::keep_choices(
                    ui,
                    egui::Id::new("profile-keep"),
                    keeping,
                    actions,
                );
            }
        }
        Grid::new("profile-credentials")
            .num_columns(3)
            .min_col_width(widgets::FACT_LABEL)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                for &helper in &helpers {
                    ui.label("Helper");
                    ui.label(helper.text().to_string());
                    widgets::origin(ui, Some(helper), home);
                    ui.end_row();
                }
                for helper in profile.credential_helpers_for_urls() {
                    let key = helper.key.to_string();
                    let url = key
                        .strip_prefix("credential.")
                        .and_then(|rest| rest.strip_suffix(".helper"))
                        .unwrap_or(&key)
                        .to_owned();
                    ui.label(format!("For {url}"));
                    ui.label(helper.text().to_string());
                    widgets::origin(ui, Some(helper), home);
                    ui.end_row();
                }
            });
        // One just set up: how it went.
        if !helpers.is_empty()
            && let Some(keeping) = keeping.filter(|keeping| keeping.kept.is_some())
        {
            ui.add_space(6.0);
            crate::views::sign_in::keep_choices(
                ui,
                egui::Id::new("profile-keep"),
                keeping,
                actions,
            );
        }
    });
}

/// The forges' hosts egit reads pull and merge requests from: github.com and gitlab.com, and
/// the GitHub Enterprise and self-managed GitLab hosts listed (`egit.githubHost`,
/// `egit.gitlabHost`), added and removed here.
fn forges_card(
    ui: &mut Ui,
    (view, sign_ins): (&mut ProfileView, &mut crate::sign_ins::SignIns),
    (snapshot, git): (&ConfigSnapshot, Option<&egit_git::Git>),
    home: Option<&Path>,
    actions: &mut Vec<Action>,
) {
    let saving = view.saving.is_pending();
    widgets::card(ui, |ui| {
        widgets::heading(ui, "GitHub and GitLab");
        ui.add(
            egui::Label::new(muted(
                ui,
                "egit reads the pull requests of remotes on github.com and the merge requests \
                 and pipelines of those on gitlab.com, and of those on the hosts listed here, \
                 signed in with a token git's credential helper keeps for each host.",
            ))
            .wrap(),
        );
        if let Some(git) = git {
            ui.add_space(8.0);
            accounts(ui, sign_ins, git, snapshot, actions);
        }
        for (kind, key, typed) in [
            (
                ForgeKind::GitHub,
                GITHUB_HOSTS_KEY,
                &mut view.new_github_host,
            ),
            (
                ForgeKind::GitLab,
                GITLAB_HOSTS_KEY,
                &mut view.new_gitlab_host,
            ),
        ] {
            ui.add_space(8.0);
            let listed = HostList {
                kind,
                key,
                saving,
                home,
            };
            listed.show(ui, snapshot, typed, actions);
        }
    });
}

/// Who is signed in to each host (github.com, gitlab.com, and those listed), asked once with
/// what git's helpers keep, and Sign in… or Sign out beside each.
fn accounts(
    ui: &mut Ui,
    sign_ins: &mut crate::sign_ins::SignIns,
    git: &egit_git::Git,
    snapshot: &ConfigSnapshot,
    actions: &mut Vec<Action>,
) {
    let palette = Palette::of(ui.ctx());
    let listed = |key| {
        snapshot
            .get_all(key)
            .filter_map(|entry| entry.text().to_str().ok())
    };
    let hosts = ForgeHosts::with_listed(listed(GITHUB_HOSTS_KEY), listed(GITLAB_HOSTS_KEY));
    ui.label(RichText::new("Accounts").semibold());
    Grid::new("profile-forge-accounts")
        .num_columns(3)
        .min_col_width(widgets::FACT_LABEL)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            for host in hosts.hosts() {
                ui.label(format!("{} ({})", host.name(), host.kind().name()));
                let signed_in = match sign_ins.who(git, host) {
                    None => {
                        widgets::working(ui, "Asking…");
                        None
                    }
                    Some(Ok(Some(login))) => {
                        ui.label(
                            RichText::new(format!("{}  Signed in as {login}", icon::CHECK_CIRCLE))
                                .color(palette.ok),
                        );
                        Some(true)
                    }
                    Some(Ok(None)) => {
                        ui.label(muted(ui, "Not signed in"));
                        Some(false)
                    }
                    Some(Err(problem)) => {
                        ui.label(RichText::new(problem.title.as_str()).color(palette.danger))
                            .on_hover_text(problem.detail.as_str());
                        Some(false)
                    }
                };
                let leaving = sign_ins.is_signing_out(host);
                match signed_in {
                    Some(true) => {
                        if widgets::secondary(ui, !leaving, "Sign out")
                            .on_hover_text(format!(
                                "git's credential helper forgets the token it keeps for {}: git \
                                 asks again the next time it reaches it over HTTPS",
                                host.name()
                            ))
                            .clicked()
                        {
                            actions.push(Action::SignOut(host.clone()));
                        }
                    }
                    Some(false) => {
                        if widgets::secondary(ui, true, "Sign in…").clicked() {
                            actions.push(Action::SignIn {
                                repo: None,
                                host: host.clone(),
                            });
                        }
                    }
                    None => {
                        ui.label("");
                    }
                }
                ui.end_row();
            }
        });
}

/// One forge's hosts in the profile: each listed, removable where the user's own settings list
/// it, and a field to add one.
struct HostList<'a> {
    kind: ForgeKind,
    key: &'static str,
    saving: bool,
    home: Option<&'a Path>,
}

impl HostList<'_> {
    /// Remove, for `entry` (saying what stops) where it is the user's own to change.
    fn remove(
        &self,
        ui: &mut Ui,
        (entry, what): (&egit_git::ConfigEntry<'_>, &str),
        (key, value): (std::borrow::Cow<'static, str>, &str),
        actions: &mut Vec<Action>,
    ) {
        if entry.scope == ConfigScope::Global {
            let hover = if key == self.key {
                format!("Stop reading {what}'s requests")
            } else {
                format!("Stop taking {what} for this host's ssh")
            };
            if widgets::secondary(ui, !self.saving, "Remove")
                .on_hover_text(hover)
                .clicked()
            {
                actions.push(Action::SaveProfile(vec![ConfigChange::Remove {
                    key,
                    value: value.to_owned(),
                }]));
            }
        } else {
            ui.label(muted(ui, "Set outside your global settings"));
        }
    }

    fn show(
        self,
        ui: &mut Ui,
        snapshot: &ConfigSnapshot,
        typed: &mut String,
        actions: &mut Vec<Action>,
    ) {
        let (title, hint) = match self.kind {
            ForgeKind::GitHub => ("GitHub Enterprise hosts", "github.example.com"),
            ForgeKind::GitLab => ("Self-managed GitLab hosts", "gitlab.example.com"),
        };
        ui.label(RichText::new(title).semibold());
        // None listed: no rows to lay out (an empty grid still takes its spacing).
        if snapshot.get_all(self.key).next().is_some() {
            self.listed(ui, snapshot, actions);
        }
        widgets::form_row(
            ui,
            widgets::FACT_LABEL,
            &format!("{} host", self.kind.name()),
            |ui| {
                let field = ui.add_sized(FIELD, crate::widgets::line_field(typed).hint_text(hint));
                let entry = typed.trim().to_owned();
                let host = ForgeHost::parse(self.kind, &entry).filter(|host| !host.is_public());
                let entered =
                    field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                let add = widgets::secondary(ui, host.is_some() && !self.saving, "Add");
                if (add.clicked() || entered) && host.is_some() {
                    actions.push(Action::SaveProfile(vec![ConfigChange::Add {
                        key: self.key.into(),
                        value: entry,
                    }]));
                    typed.clear();
                } else if !entry.is_empty() && host.is_none() {
                    ui.label(
                        RichText::new("Not a host egit can use")
                            .color(Palette::of(ui.ctx()).danger),
                    );
                }
                field
            },
        );
    }

    /// The hosts listed, each with where it is set, and its ssh hosts on names of their own.
    fn listed(&self, ui: &mut Ui, snapshot: &ConfigSnapshot, actions: &mut Vec<Action>) {
        Grid::new(("profile-forge-hosts", self.key))
            .num_columns(3)
            .min_col_width(widgets::FACT_LABEL)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                for entry in snapshot.get_all(self.key) {
                    let host = entry.text().to_string();
                    ui.label(&host);
                    widgets::origin(ui, Some(entry), self.home);
                    self.remove(ui, (&entry, &host), (self.key.into(), &host), actions);
                    ui.end_row();
                    // Its ssh on names of its own (a self-managed GitLab's often is).
                    let name =
                        ForgeHost::parse(self.kind, &host).map(|parsed| parsed.name().to_owned());
                    for (listed, ssh, set) in crate::forge::ssh_host_entries(snapshot) {
                        let same = ForgeHost::parse(self.kind, &listed)
                            .is_some_and(|parsed| Some(parsed.name()) == name.as_deref());
                        if !same {
                            continue;
                        }
                        ui.label(muted(ui, format!("    its ssh: {ssh}")));
                        widgets::origin(ui, Some(set), self.home);
                        let key = crate::forge::ssh_host_key(&listed);
                        self.remove(ui, (&set, &ssh), (key.into(), &ssh), actions);
                        ui.end_row();
                    }
                }
            });
    }
}

fn pull_push_card(ui: &mut Ui, snapshot: &ConfigSnapshot, home: Option<&Path>) {
    let profile = snapshot.profile();
    widgets::card(ui, |ui| {
        widgets::heading(ui, "Pull and push");
        Grid::new("profile-pull-push")
            .num_columns(3)
            .min_col_width(widgets::FACT_LABEL)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                let rebase = profile.pull_rebase();
                ui.label("Pull");
                ui.label(match rebase.map(|setting| setting.value) {
                    None => "Asks when branches have diverged",
                    Some(PullRebase::Merge) => "Merges",
                    Some(PullRebase::Rebase) => "Rebases",
                    Some(PullRebase::RebaseMerges) => "Rebases, keeping merges",
                    Some(PullRebase::Interactive) => "Rebases interactively",
                });
                widgets::origin(ui, rebase.and_then(|setting| setting.entry), home);
                ui.end_row();

                let fast_forward = profile.pull_fast_forward();
                ui.label("Fast-forward");
                ui.label(match fast_forward.map(|setting| setting.value) {
                    None | Some(PullFastForward::Allowed) => "When possible",
                    Some(PullFastForward::Never) => "Never: always merge",
                    Some(PullFastForward::Only) => "Only: refuse anything else",
                });
                widgets::origin(ui, fast_forward.and_then(|setting| setting.entry), home);
                ui.end_row();

                let auto = profile.push_auto_setup_remote();
                ui.label("New branches");
                ui.label(if auto.value {
                    "Pushing sets up tracking"
                } else {
                    "Need an upstream before pushing"
                });
                widgets::origin(ui, auto.entry, home);
                ui.end_row();

                let default = profile.push_default();
                ui.label("Push");
                ui.label(
                    default.map_or_else(|| "simple".to_owned(), |entry| entry.text().to_string()),
                );
                widgets::origin(ui, default, home);
                ui.end_row();

                let branch = profile.default_branch();
                ui.label("First branch");
                ui.label(
                    branch.map_or_else(|| "master".to_owned(), |entry| entry.text().to_string()),
                );
                widgets::origin(ui, branch, home);
                ui.end_row();
            });
    });
}

fn all_settings_card(
    ui: &mut Ui,
    table: &mut SnapshotTable,
    snapshot: &Arc<ConfigSnapshot>,
    home: Option<&Path>,
) {
    widgets::card(ui, |ui| {
        widgets::heading(ui, "Every setting");
        ui.label(muted(
            ui,
            "In the order git reads them: for a key set more than once, the last value applies.",
        ));
        ui.add_space(6.0);
        config_table(ui, "profile-config", table, snapshot, home, 360.0);
    });
}

/// A table of every entry of `snapshot`.
pub(crate) fn config_table(
    ui: &mut Ui,
    id: &str,
    table: &mut SnapshotTable,
    snapshot: &Arc<ConfigSnapshot>,
    home: Option<&Path>,
    height: f32,
) {
    table.sync(snapshot);
    let provider = ConfigRows::new(snapshot, home);
    let width = ui.available_width();
    let columns = vec![
        egui_table_kit::layout::Column::new((width * 0.28).max(140.0)).range(100.0..=f32::INFINITY),
        egui_table_kit::layout::Column::new((width * 0.32).max(140.0)).range(100.0..=f32::INFINITY),
        egui_table_kit::layout::Column::new(90.0).range(70.0..=160.0),
        egui_table_kit::layout::Column::new((width * 0.28).max(140.0))
            .range(100.0..=f32::INFINITY)
            .remainder(true),
    ];
    let shown = crate::tables::styled(TableKit::new(id, &provider, &mut table.state), ui)
        .with_columns(columns)
        .with_row_height(ROW_HEIGHT)
        .with_max_height(Some(height))
        .with_striped(true)
        .show(ui, |_, _, _, _| None);
    if let Err(error) = shown {
        ui.label(
            RichText::new(format!("The table couldn't be drawn: {error}"))
                .color(Palette::of(ui.ctx()).danger),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form() -> Form {
        Form {
            name: "Me".to_owned(),
            email: "me@example.com".to_owned(),
            ..Form::default()
        }
    }

    #[test]
    fn an_unchanged_form_changes_nothing() {
        assert_eq!(form().changes(&form()), []);
        let mut padded = form();
        padded.name = "  Me ".to_owned();
        assert_eq!(padded.changes(&form()), []);
    }

    #[test]
    fn edits_become_changes_and_emptied_fields_are_removed() {
        let mut edited = form();
        edited.name = "Someone Else".to_owned();
        edited.email = String::new();
        edited.format = SigningFormat::Ssh;
        edited.sign_commits = true;
        assert_eq!(
            edited.changes(&form()),
            [
                ConfigChange::Set {
                    key: "user.name".into(),
                    value: Some("Someone Else".to_owned())
                },
                ConfigChange::Set {
                    key: "user.email".into(),
                    value: None
                },
                ConfigChange::Set {
                    key: "gpg.format".into(),
                    value: Some("ssh".to_owned())
                },
                ConfigChange::Set {
                    key: "commit.gpgSign".into(),
                    value: Some("true".to_owned())
                },
            ]
        );
    }

    #[test]
    fn refuses_what_would_break_commits() {
        assert_eq!(form().problem(), None);
        let mut bad = form();
        bad.name = "Me <me>".to_owned();
        assert!(bad.problem().is_some());
        let mut bad = form();
        bad.email = "me@example.com\nX".to_owned();
        assert!(bad.problem().is_some());
        let mut bad = form();
        bad.email = "me @example.com".to_owned();
        assert!(bad.problem().is_some());
    }
}
