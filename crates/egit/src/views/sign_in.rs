//! The sign-in dialog, for a GitHub or GitLab host: why egit asks (pull or merge requests,
//! pipelines), then in two steps what to do: create a personal access token on the host (its
//! page opened with the scope egit needs ticked, or its link copied, for whichever browser one
//! is signed in with), and paste it. Checked with the host, which says
//! whose it is, it is kept by git's credential helper, where git finds it for the host's HTTPS
//! remotes too. Or, for those whose credential helper signs in itself (Git Credential Manager,
//! in the browser), through it. Once signed in, who it is, and whether it lasts past this run:
//! where no helper keeps it, the helpers installed to keep it with ([`keep_choices`], which
//! Profile's credentials offer too).

use std::time::Duration;

use web_time::Instant;

use egit_forge::{ForgeHost, ForgeKind};
use egit_git::Git;
use egui::{Id, Key, Modal, RichText, Ui, UiBuilder};

use crate::app::Action;
use crate::sign_ins::{Keeping, SignInDialog, SignIns};
use crate::theme::{Palette, icon};
use crate::widgets::{self, muted};

/// The scope a token needs on `kind`, as its page names it.
pub(crate) const fn scope_of(kind: ForgeKind) -> &'static str {
    match kind {
        ForgeKind::GitHub => "repo",
        ForgeKind::GitLab => "api",
    }
}

/// What egit reads from `kind`: its words.
pub(crate) const fn what_is_read(kind: ForgeKind) -> &'static str {
    match kind {
        ForgeKind::GitHub => "pull requests and their checks",
        ForgeKind::GitLab => "merge requests and CI/CD pipelines",
    }
}

/// The dialog, while it is open; `git`, the user's (for the helpers to keep a sign-in with).
pub(crate) fn show(
    ctx: &egui::Context,
    sign_ins: &mut SignIns,
    git: Option<&Git>,
    actions: &mut Vec<Action>,
) {
    // Where nothing keeps the sign-in, the helpers that could (looked for only then).
    let unkept = sign_ins
        .dialog
        .as_ref()
        .is_some_and(|dialog| dialog.signed.as_ref().is_some_and(|(_, kept)| !kept));
    let keeping = git.filter(|_| unkept).map(|git| sign_ins.keeping(git));
    let Some(dialog) = &mut sign_ins.dialog else {
        return;
    };
    let palette = Palette::of(ctx);
    let frame = egui::Frame::new()
        .fill(palette.surface)
        .stroke(egui::Stroke::new(1.0, palette.border))
        .corner_radius(egui::CornerRadius::same(crate::theme::RADIUS_CARD))
        .inner_margin(egui::Margin::same(20));
    let shown = Modal::new(Id::new("sign-in")).frame(frame).show(ctx, |ui| {
        ui.set_width(520.0);
        fill(ui, dialog, keeping.as_ref(), actions)
    });
    // Escape or a click outside closes it, but not while the host is being asked.
    let closing = shown.inner || (shown.should_close() && !dialog.signing.is_pending());
    if closing {
        sign_ins.dialog = None;
    }
}

/// The dialog's contents (`keeping`: the helpers to keep a sign-in with, where none does);
/// returns whether it should close.
fn fill(
    ui: &mut Ui,
    dialog: &mut SignInDialog,
    keeping: Option<&Keeping>,
    actions: &mut Vec<Action>,
) -> bool {
    let palette = Palette::of(ui.ctx());
    let host = dialog.host.clone();
    let kind = host.kind();
    widgets::panel_title(ui, format!("{}  Sign in to {}", icon::SIGN_IN, kind.name()));
    ui.label(muted(ui, host.name()));
    ui.add_space(8.0);
    if let Some((login, kept)) = dialog.signed.clone() {
        return signed(ui, palette, &host, (&login, kept), keeping, actions);
    }
    ui.add(
        egui::Label::new(format!(
            "egit reads {} from {} with a personal access token. Git uses the same one for \
             {}'s HTTPS remotes: your credential helper keeps it, not egit.",
            what_is_read(kind),
            host.name(),
            host.name()
        ))
        .wrap(),
    );
    ui.add_space(12.0);
    let signing = dialog.signing.is_pending();
    create_step(ui, &host, actions);
    ui.add_space(8.0);
    let mut submitted = false;
    step(ui, "2", "Paste it here", |ui| {
        let hint = match kind {
            ForgeKind::GitHub => "ghp_…",
            ForgeKind::GitLab => "glpat-…",
        };
        let field = widgets::form_row(ui, 56.0, "Token", |ui| {
            ui.add_enabled(
                !signing,
                widgets::line_field(&mut *dialog.token)
                    .password(true)
                    .hint_text(hint)
                    .desired_width(320.0),
            )
        });
        submitted = field.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter));
        // Typed into at once.
        if !signing
            && dialog.token.is_empty()
            && !field.has_focus()
            && ui.memory(|m| m.focused().is_none())
        {
            field.request_focus();
        }
    });
    ui.add_space(12.0);
    let mut close = false;
    let ready = !signing && !dialog.token.trim().is_empty();
    widgets::buttons_row(ui, |ui| {
        if widgets::primary(ui, ready, "Sign in").clicked() || (submitted && ready) {
            dialog.with_token();
        }
        close = widgets::secondary(ui, !signing, "Cancel").clicked();
    });
    ui.add_space(6.0);
    let helper = widgets::link(ui, "Sign in through git's credential helper instead")
        .on_hover_text(
            "Git Credential Manager opens the host's sign-in page in your browser; other helpers, \
         and git without one, ask for a username and password (a token as the password)",
        );
    if helper.clicked() && !signing {
        dialog.with_helper();
    }
    // What comes and goes follows the buttons.
    ui.scope_builder(egui::UiBuilder::new().id(Id::new("sign-in-state")), |ui| {
        if signing {
            ui.add_space(8.0);
            widgets::working(ui, &format!("Asking {}…", host.name()));
        } else if let Some(Err(problem)) = dialog.signing.read_as_ref() {
            ui.add_space(8.0);
            let problem = problem.clone();
            widgets::problem_box(ui, &problem, |_| {});
        }
    });
    close
}

/// The first step: what the token's page asks, and the ways to it (here, or its link copied).
fn create_step(ui: &mut Ui, host: &ForgeHost, actions: &mut Vec<Action>) {
    let kind = host.kind();
    step(ui, "1", "Create a token", |ui| {
        ui.add(
            egui::Label::new(muted(
                ui,
                format!(
                    "Its page comes with the {} scope ticked: name it, choose when it expires, \
                     create it, and copy it. Signed in to it in another browser? Copy the link \
                     and open it there.",
                    scope_of(kind)
                ),
            ))
            .wrap(),
        );
        ui.horizontal(|ui| {
            if widgets::secondary(
                ui,
                true,
                &format!("{}  Create a token…", icon::ARROW_SQUARE_OUT),
            )
            .clicked()
            {
                actions.push(Action::OpenUrl(host.new_token_url()));
            }
            // For the browser one is signed in with, whichever it is.
            copy_link(ui, &host.new_token_url());
        });
    });
}

/// How long "Copied" shows after the link is copied.
const COPIED: Duration = Duration::from_millis(1500);

/// Copy the link: the token's page's address to the clipboard (to paste into whichever
/// browser one is signed in with), "Copied" for a moment after, the button as wide either way.
fn copy_link(ui: &mut Ui, url: &str) {
    let id = Id::new("sign-in-copied");
    let copied_at: Option<Instant> = ui.data(|data| data.get_temp(id));
    let copied = copied_at.is_some_and(|at| at.elapsed() < COPIED);
    let words = if copied {
        format!("{}  Copied", icon::CHECK)
    } else {
        format!("{}  Copy the link", icon::COPY)
    };
    // As wide as its longer words, whichever it says.
    let widest = ui
        .painter()
        .layout_no_wrap(
            format!("{}  Copy the link", icon::COPY),
            egui::TextStyle::Button.resolve(ui.style()),
            egui::Color32::PLACEHOLDER,
        )
        .size()
        .x;
    let width = 2.0f32.mul_add(ui.spacing().button_padding.x, widest);
    let response = ui
        .add(
            egui::Button::new(words)
                .wrap_mode(egui::TextWrapMode::Extend)
                .min_size(egui::vec2(width, 0.0)),
        )
        .on_hover_text(url);
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Copy the link"));
    crate::help::note(ui, response.rect, "Copy the link");
    if response.clicked() {
        ui.ctx().copy_text(url.to_owned());
        ui.data_mut(|data| data.insert_temp(id, Instant::now()));
        ui.ctx().request_repaint_after(COPIED);
    }
}

/// A numbered step: its number in a circle, its title, and what `add` draws under it.
fn step(ui: &mut Ui, number: &str, title: &str, add: impl FnOnce(&mut Ui)) {
    let palette = Palette::of(ui.ctx());
    ui.horizontal_top(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::hover());
        ui.painter()
            .circle_stroke(rect.center(), 10.0, egui::Stroke::new(1.0, palette.accent));
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            number,
            egui::TextStyle::Small.resolve(ui.style()),
            palette.accent,
        );
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            ui.label(RichText::new(title).strong());
            add(ui);
        });
    });
}

/// Signed in: whose it is, whether it lasts past this run (the helpers to keep it with, where
/// it doesn't), and Done; returns whether Done was clicked.
fn signed(
    ui: &mut Ui,
    palette: Palette,
    host: &ForgeHost,
    (login, kept): (&str, bool),
    keeping: Option<&Keeping>,
    actions: &mut Vec<Action>,
) -> bool {
    ui.label(
        RichText::new(format!(
            "{}  Signed in to {} as {login}.",
            icon::CHECK_CIRCLE,
            host.name()
        ))
        .color(palette.ok),
    );
    ui.add_space(4.0);
    ui.add(
        egui::Label::new(format!(
            "egit reads {} for every open repository on {} now.",
            what_is_read(host.kind()),
            host.name()
        ))
        .wrap(),
    );
    if !kept {
        let done = keeping.is_some_and(Keeping::went_well);
        if !done {
            ui.add_space(8.0);
            ui.add(
                egui::Label::new(
                    RichText::new(format!(
                        "{}  No credential helper keeps it: egit is signed in until it quits. \
                         Keep it with one of these, and it stays signed in:",
                        icon::WARNING
                    ))
                    .color(palette.warn),
                )
                .wrap(),
            );
        }
        if let Some(keeping) = keeping {
            ui.add_space(6.0);
            keep_choices(ui, Id::new("sign-in-keep"), keeping, actions);
        }
    }
    ui.add_space(12.0);
    let setting_up = keeping.is_some_and(|keeping| keeping.setting_up.is_some());
    let mut done = false;
    widgets::buttons_row(ui, |ui| {
        done = widgets::primary(ui, !setting_up, "Done").clicked();
    });
    done
}

/// Keeping sign-ins past this run, in a `Ui` of its own `id`: a button for each credential
/// helper installed that egit can set up (the safest first, where each keeps them on hover),
/// the one being set up, and how the last went (the buttons gone once it went well).
pub(crate) fn keep_choices(ui: &mut Ui, id: Id, keeping: &Keeping, actions: &mut Vec<Action>) {
    let palette = Palette::of(ui.ctx());
    ui.scope_builder(UiBuilder::new().id(id), |ui| {
        if let Some(words) = keeping.setting_up {
            widgets::working(ui, &format!("Setting up {words}…"));
            return;
        }
        match &keeping.kept {
            Some((words, Ok((kept, of)))) => {
                let (text, color) = if *of == 0 {
                    (
                        format!(
                            "{}  {words} is set up: git keeps what you sign in with from now on.",
                            icon::CHECK_CIRCLE
                        ),
                        palette.ok,
                    )
                } else if kept == of {
                    (
                        format!(
                            "{}  {words} keeps {} now: egit stays signed in after it quits.",
                            icon::CHECK_CIRCLE,
                            if *of == 1 { "it" } else { "them" }
                        ),
                        palette.ok,
                    )
                } else {
                    (
                        format!(
                            "{}  {words} is set up, but didn't give the token back: egit stays \
                             signed in only until it quits.",
                            icon::WARNING
                        ),
                        palette.warn,
                    )
                };
                ui.add(egui::Label::new(RichText::new(text).color(color)).wrap());
                if kept == of {
                    return;
                }
                ui.add_space(6.0);
            }
            Some((_, Err(problem))) => {
                widgets::problem_box(ui, problem, |_| {});
                ui.add_space(6.0);
            }
            None => {}
        }
        let Some(keepers) = &keeping.keepers else {
            widgets::working(ui, "Looking for credential helpers…");
            return;
        };
        if keepers.is_empty() {
            ui.add(
                egui::Label::new(muted(
                    ui,
                    "None that egit can set up is installed. Git Credential Manager keeps \
                     credentials encrypted on every system; your distribution's git may package \
                     libsecret's helper apart.",
                ))
                .wrap(),
            );
            return;
        }
        ui.horizontal_wrapped(|ui| {
            for keeper in keepers {
                if widgets::secondary(ui, true, keeper.words)
                    .on_hover_text(keeper.about)
                    .clicked()
                {
                    actions.push(Action::KeepWith(*keeper));
                }
            }
        });
    });
}
