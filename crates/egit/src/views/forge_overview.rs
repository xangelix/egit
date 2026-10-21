//! A repository's side on GitHub and GitLab, on its Overview: the hosts egit knows, then each
//! remote's address and what egit took it for (a forge's repository, or nothing: its host isn't
//! one egit knows; an ssh host of none of them offered as the ssh host of each listed one,
//! which a self-managed GitLab's often is), the account it is read with, its open pull or
//! merge requests, and, on
//! GitLab, its pipelines (how many were read and the newest; or why none: GitLab lists none to
//! the account, refused, out of reach); last, how many of the commits they ran for this
//! history shows. Where a request or a pipeline's mark doesn't show, this says why.

use std::fmt::Write as _;

use bstr::ByteSlice as _;
use egit_forge::{ForgeHost, ForgeKind, ForgeRepository};
use egui::{Label, RichText, Ui};

use super::pipeline::ci_words;
use crate::app::{Action, ConfigChange};
use crate::ci::{CiSeen, LIMIT};
use crate::forge::{RepoForge, Resolved, SeenRemote, ssh_host_key};
use crate::format;
use crate::history::History;
use crate::repo::RepoId;
use crate::theme::Weighted as _;
use crate::theme::{Palette, icon};
use crate::widgets::{self, muted};

/// The help topic explaining it.
const TOPIC: &str = "forge-overview";

/// The card: `forge`'s remotes and what was read from them; `history`, the one shown.
pub(super) fn card(
    ui: &mut Ui,
    (forge, history): (&mut RepoForge, &History),
    id: RepoId,
    actions: &mut Vec<Action>,
) {
    widgets::card(ui, |ui| {
        ui.horizontal(|ui| {
            widgets::heading(ui, "GitHub and GitLab");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let reading = forge.is_reading() || forge.pipelines().is_reading();
                let any = !forge.repositories.is_empty();
                if widgets::secondary(
                    ui,
                    any && !reading,
                    &format!("{}  Read again", icon::ARROWS_CLOCKWISE),
                )
                .on_hover_text("Read the requests and pipelines again")
                .clicked()
                {
                    actions.push(Action::RefreshPullRequests(id));
                }
                if reading && any {
                    ui.spinner();
                }
            });
        });
        if let Some(problem) = forge.resolve_problem() {
            let problem = problem.clone();
            widgets::problem_box(ui, &problem, |_| {});
        }
        let Some(resolved) = forge.resolved().cloned() else {
            widgets::working(ui, "Working out the remotes…");
            return;
        };
        hosts(ui, &resolved);
        if resolved.seen.is_empty() {
            ui.label(muted(ui, "This repository has no remotes."));
            return;
        }
        for seen in resolved.seen.iter() {
            ui.add_space(10.0);
            remote(ui, (forge, seen, &resolved.hosts), id, actions);
        }
        on_history(ui, forge, history);
    });
}

/// The hosts egit knew as the remotes were worked out.
fn hosts(ui: &mut Ui, resolved: &Resolved) {
    let hosts: Vec<String> = resolved
        .hosts
        .iter()
        .map(|host| format!("{} ({})", host.name(), host.kind().name()))
        .collect();
    ui.add(
        Label::new(muted(
            ui,
            format!(
                "egit reads pull requests from GitHub, and merge requests and pipelines from \
                 GitLab, for the remotes on these hosts: {}. Profile lists more.",
                hosts.join(", ")
            ),
        ))
        .wrap(),
    );
}

/// A remote: its address, what egit took it for, and what was read from it; `hosts`, those
/// egit knew.
fn remote(
    ui: &mut Ui,
    (forge, seen, hosts): (&mut RepoForge, &SeenRemote, &[ForgeHost]),
    id: RepoId,
    actions: &mut Vec<Action>,
) {
    let width = widgets::FACT_LABEL;
    ui.label(RichText::new(seen.remote.to_str_lossy()).semibold());
    widgets::fact_top_of(ui, width, ("Address", TOPIC), |ui| {
        let written = seen.written.as_ref().unwrap_or(&seen.url);
        ui.add(Label::new(written.to_str_lossy()).wrap());
        if seen.written.is_some() {
            ui.add(
                Label::new(muted(
                    ui,
                    format!("git reaches it as {}", seen.url.to_str_lossy()),
                ))
                .wrap(),
            );
        }
    });
    let Some(repository) = &seen.repository else {
        unknown(ui, seen, hosts, actions);
        return;
    };
    let kind = repository.host().kind();
    widgets::fact_top_of(ui, width, ("Recognized as", TOPIC), |ui| {
        let what = match kind {
            ForgeKind::GitHub => "GitHub repository",
            ForgeKind::GitLab => "GitLab project",
        };
        ui.add(
            Label::new(format!(
                "{what} {} on {}",
                repository.path(),
                repository.host().name()
            ))
            .wrap(),
        );
    });
    let index = forge.index_of(repository);
    account(ui, (forge, repository, index), id, actions);
    requests(ui, forge, (kind, index));
    if kind == ForgeKind::GitLab {
        pipelines(ui, forge, index);
    }
}

/// A remote on no host egit knows: why, and what to do. An ssh host is offered as the ssh
/// host of each host Profile lists (the likeliest first: the most of its name's end shared), as
/// a self-managed GitLab's ssh is often on a name of its own (`gitlab_shell_ssh_host`).
fn unknown(ui: &mut Ui, seen: &SeenRemote, hosts: &[ForgeHost], actions: &mut Vec<Action>) {
    let palette = Palette::of(ui.ctx());
    let ssh = seen.ssh_host();
    let mut listed: Vec<&ForgeHost> = hosts.iter().filter(|host| !host.is_public()).collect();
    if let Some(ssh) = &ssh {
        listed.sort_by_key(|host| std::cmp::Reverse(shared_end(ssh, host.name())));
    }
    widgets::fact_top_of(ui, widgets::FACT_LABEL, ("Recognized as", TOPIC), |ui| {
        let words = match (&ssh, listed.is_empty()) {
            (Some(ssh), false) => format!(
                "{}  Nothing: {ssh} isn't a host egit knows. If it is the ssh host of a \
                 GitLab or GitHub Enterprise host Profile lists (its web pages on another \
                 name), say which:",
                icon::WARNING
            ),
            (Some(ssh), true) => format!(
                "{}  Nothing: {ssh} isn't a host egit knows. List your GitLab or GitHub \
                 Enterprise host in Profile (the name its web pages are on); then, if its ssh \
                 is on another name, as this one may be, say so here.",
                icon::WARNING
            ),
            (None, _) => format!(
                "{}  Nothing: its host isn't one egit knows. A GitHub Enterprise or \
                 self-managed GitLab host is listed in Profile, under GitHub and GitLab.",
                icon::WARNING
            ),
        };
        ui.add(Label::new(RichText::new(words).color(palette.warn)).wrap());
        match ssh {
            Some(ssh) if !listed.is_empty() => {
                ui.horizontal_wrapped(|ui| {
                    for host in listed.iter().take(4) {
                        let words = format!("The ssh host of {}", host.name());
                        if widgets::secondary(ui, true, &words)
                            .on_hover_text(format!(
                                "Read {ssh}'s repositories from {} (egit.{}.sshHost in your \
                                 git settings)",
                                host.name(),
                                host.name()
                            ))
                            .clicked()
                        {
                            actions.push(Action::SaveProfile(vec![ConfigChange::Add {
                                key: ssh_host_key(host.name()).into(),
                                value: ssh.clone(),
                            }]));
                        }
                    }
                });
            }
            _ => {
                if widgets::secondary(ui, true, "Open Profile").clicked() {
                    actions.push(Action::ShowProfile);
                }
            }
        }
    });
}

/// How many of their names' last labels two hosts share (`git-ssh.example.org` and
/// `git.example.org:8443`: two).
fn shared_end(ssh: &str, host: &str) -> usize {
    let host = host.rsplit_once(':').map_or(host, |(name, _)| name);
    ssh.rsplit('.')
        .zip(host.rsplit('.'))
        .take_while(|(a, b)| a.eq_ignore_ascii_case(b))
        .count()
}

/// Who it is read as: signed in, not (and Sign in…), or the credential git's helper keeps.
fn account(
    ui: &mut Ui,
    (forge, repository, index): (&RepoForge, &ForgeRepository, Option<usize>),
    id: RepoId,
    actions: &mut Vec<Action>,
) {
    let palette = Palette::of(ui.ctx());
    let host = repository.host();
    let login = forge.account(host).and_then(|account| account.login());
    let wanted = forge.needs_sign_in().filter(|(wants, _)| wants == host);
    let read =
        index.is_some_and(|index| forge.found().totals.get(index).is_some_and(Option::is_some));
    widgets::fact_top_of(ui, widgets::FACT_LABEL, ("Account", TOPIC), |ui| {
        if let Some(login) = login {
            ui.label(
                RichText::new(format!("{}  Signed in as {login}", icon::CHECK_CIRCLE))
                    .color(palette.ok),
            );
        } else if let Some((host, refused)) = wanted {
            let words = if refused {
                format!("{} refused the credentials git keeps", host.name())
            } else {
                format!("Not signed in to {}", host.name())
            };
            ui.label(RichText::new(words).color(palette.warn));
            if widgets::secondary(ui, true, "Sign in…").clicked() {
                actions.push(Action::SignIn {
                    repo: Some(id),
                    host,
                });
            }
        } else if read {
            ui.label("Signed in with the credential git's helper keeps");
        } else {
            ui.label(muted(ui, "Not asked yet"));
        }
    });
}

/// Its open pull or merge requests: how many, or why none were read.
fn requests(ui: &mut Ui, forge: &RepoForge, (kind, index): (ForgeKind, Option<usize>)) {
    let palette = Palette::of(ui.ctx());
    let reading = forge.is_reading();
    let (open, problem) = {
        let found = forge.found();
        let open = index.and_then(|index| found.totals.get(index).copied().flatten());
        (open, found.problem.clone())
    };
    let label = match kind {
        ForgeKind::GitHub => "Pull requests",
        ForgeKind::GitLab => "Merge requests",
    };
    widgets::fact_top_of(ui, widgets::FACT_LABEL, (label, TOPIC), |ui| {
        if let Some(open) = open {
            ui.label(format!("{} open", format::count(u64::from(open))));
        } else if let Some(problem) = &problem {
            ui.add(
                Label::new(RichText::new(super::pulls::said(problem)).color(palette.danger)).wrap(),
            );
        } else if reading {
            ui.label(muted(ui, "Reading…"));
        } else {
            ui.label(muted(ui, "None read"));
        }
    });
}

/// A GitLab project's pipelines: how many were read and the newest, or why none were.
fn pipelines(ui: &mut Ui, forge: &mut RepoForge, index: Option<usize>) {
    let palette = Palette::of(ui.ctx());
    let ci = forge.pipelines();
    let reading = ci.is_reading();
    let seen = index.and_then(|index| ci.seen().get(index).cloned());
    widgets::fact_top_of(ui, widgets::FACT_LABEL, ("Pipelines", TOPIC), |ui| {
        let (words, color) = match seen {
            None | Some(CiSeen::NotRead) if reading => ("Reading…".to_owned(), palette.muted),
            None | Some(CiSeen::NotRead) => ("Not read yet".to_owned(), palette.muted),
            Some(CiSeen::Read { count: 0, .. }) => (
                "None: the project hasn't run any.".to_owned(),
                palette.muted,
            ),
            Some(CiSeen::Read { count, newest }) => (read(count, newest.as_deref()), palette.text),
            Some(CiSeen::Hidden) => (
                format!(
                    "{}  GitLab lists none to this account: CI/CD may be turned off for the \
                     project, or your role in it may not see its pipelines.",
                    icon::WARNING
                ),
                palette.warn,
            ),
            Some(CiSeen::Skipped) => (
                "Not asked: its host failed for another project first.".to_owned(),
                palette.muted,
            ),
            Some(CiSeen::Failed(why)) => (why, palette.danger),
        };
        ui.add(Label::new(RichText::new(words).color(color)).wrap());
    });
}

/// How many were read, and the newest: its number, what it ran for, how it went, and when.
fn read(count: usize, newest: Option<&egit_forge::Pipeline>) -> String {
    let limit = usize::try_from(LIMIT).unwrap_or(usize::MAX);
    let many = format::count(u64::try_from(count).unwrap_or(u64::MAX));
    let mut words = if count >= limit {
        format!("The newest {many} read")
    } else if count == 1 {
        "1 read".to_owned()
    } else {
        format!("{many} read")
    };
    if let Some(newest) = newest {
        let ran_for = newest
            .git_ref
            .as_deref()
            .map_or_else(String::new, |name| format!(" for {name}"));
        let when = u64::try_from(newest.created)
            .ok()
            .filter(|created| *created > 0)
            .map_or_else(String::new, |created| {
                format!(", {}", format::ago(created, format::now()))
            });
        let _ = write!(
            words,
            "; the newest, #{}, ran{ran_for} ({}): {}{when}.",
            newest.iid,
            newest.commit.abbreviated(8),
            ci_words(newest.status, newest.warnings)
        );
    }
    words
}

/// How many of the commits the pipelines ran for this history shows (counted in a job).
fn on_history(ui: &mut Ui, forge: &mut RepoForge, history: &History) {
    let palette = Palette::of(ui.ctx());
    let ci = forge.pipelines();
    let commits = ci.commits();
    if commits == 0 {
        return;
    }
    let shown = ci.on_history(history);
    ui.add_space(10.0);
    widgets::fact_top_of(ui, widgets::FACT_LABEL, ("In this history", TOPIC), |ui| {
        let all = format::count(u64::try_from(commits).unwrap_or(u64::MAX));
        let (words, color) = match shown {
            None => ("Counting…".to_owned(), palette.muted),
            Some(0) => (
                format!(
                    "{}  None of the {all} commits the pipelines ran for: fetch, or they ran \
                     for commits GitLab made itself (a merge request's merged results).",
                    icon::WARNING
                ),
                palette.warn,
            ),
            Some(shown) => (
                format!(
                    "{} of the {all} commits the pipelines ran for: each marked beside its name.",
                    format::count(u64::try_from(shown).unwrap_or(u64::MAX))
                ),
                palette.text,
            ),
        };
        ui.add(Label::new(RichText::new(words).color(color)).wrap());
    });
}
