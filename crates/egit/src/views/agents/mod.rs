//! The Agents panel at the right of a repository's tab: Claude Code working in its work tree.
//!
//! A head (the title, the conversations' menu, a new conversation, hiding the panel); Claude's
//! setup until it is done (`setup`); then the conversation shown, on the reading background,
//! only the items in view laid out (`transcript`, its tools' lines in `tools`, code and diffs
//! in `code`); and under it the composer (`composer`): what waits for an answer (`waiting`: a
//! tool's permission, Claude's question, its plan), the images attached, the prompt with its
//! slash commands, the mode, the model, and Send or Stop, and who it runs as.

mod code;
mod composer;
mod setup;
mod tools;
mod transcript;
mod waiting;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use egit_agents::accounts::Signer;
use egui::{Frame, Id, Margin, Stroke, Ui};
use egui_async::Bind;
use egui_commonmark::CommonMarkCache;

use crate::agents::{AgentAccount, AgentsSetup, RepoAgents, Starter};
use crate::app::Action;
use crate::attachments::Attachment;
use crate::problem::Problem;
use crate::repo::RepoId;
use crate::stack::Stack;
use crate::theme::{Palette, icon};
use crate::views::actions;
use crate::widgets::{self, Head, Look, muted};

pub(crate) use code::Highlights;

/// The panel's width at first, and its least and most.
pub(crate) const WIDTH: f32 = 460.0;
pub(crate) const WIDTHS: std::ops::RangeInclusive<f32> = 320.0..=1400.0;
/// Between the panel's sides and what it holds.
const INSET: i8 = 12;
/// Narrower than this, the composer's menus show their icons alone.
const NARROW: f32 = 420.0;

/// What a conversation's view keeps: the prompt being written and the images attached to it,
/// its items' heights, what is folded open, Markdown's and code's caches, and answers being
/// made.
pub(crate) struct ConversationView {
    /// The prompt being written.
    pub(crate) draft: String,
    /// Images attached to it, and those being read.
    pub(crate) attachments: Vec<Attachment>,
    attaching: Bind<Vec<Attachment>, Problem>,
    /// Why the last images couldn't be attached.
    attach_problem: Option<Problem>,
    stack: Stack,
    /// Tool calls (by id) and thinking (by item) folded open.
    open: HashSet<String>,
    /// Bumped by every fold, so the rows shown are made again only then.
    folds: u64,
    /// The rows shown, as last made.
    rows: transcript::Rows,
    markdown: CommonMarkCache,
    highlights: Highlights,
    /// An answer being made: why a refusal refuses (`refusing`: the request), what a plan
    /// should change, a question's choices and own words (by the question).
    refusing: Option<String>,
    reason: String,
    /// Whether the reason's field takes the keys (once, as it opens).
    focus_reason: bool,
    chosen: HashMap<String, Vec<String>>,
    others: HashMap<String, String>,
    /// The slash commands offered: the one picked, and the draft they were dismissed for.
    completion: composer::Completion,
}

impl Default for ConversationView {
    fn default() -> Self {
        Self {
            draft: String::new(),
            attachments: Vec::new(),
            attaching: Bind::new(true),
            attach_problem: None,
            stack: Stack::default(),
            open: HashSet::new(),
            folds: 0,
            rows: transcript::Rows::default(),
            markdown: CommonMarkCache::default(),
            highlights: Highlights::default(),
            refusing: None,
            reason: String::new(),
            focus_reason: false,
            chosen: HashMap::new(),
            others: HashMap::new(),
            completion: composer::Completion::default(),
        }
    }
}

impl ConversationView {
    /// Follows the conversation to its end.
    pub(crate) const fn follow(&mut self) {
        self.stack.scroll_to_end();
    }

    /// Opens what `keys` name (tool calls' ids), as clicking their lines would.
    #[cfg(test)]
    pub(crate) fn open(&mut self, keys: &[&str]) {
        self.open.extend(keys.iter().map(|key| (*key).to_owned()));
        self.folds = self.folds.wrapping_add(1);
    }

    /// Opens or folds `key`.
    fn toggle(&mut self, key: &str) {
        if !self.open.remove(key) {
            self.open.insert(key.to_owned());
        }
        self.folds = self.folds.wrapping_add(1);
    }

    /// Attaches the images `paths` name to the prompt, read in a job (at most
    /// [`crate::attachments::MOST`] in all).
    pub(crate) fn attach(&mut self, paths: Vec<PathBuf>) {
        let room = crate::attachments::MOST.saturating_sub(self.attachments.len());
        if room == 0 || paths.is_empty() {
            return;
        }
        let paths: Vec<PathBuf> = paths.into_iter().take(room).collect();
        self.attach_problem = None;
        self.attaching.request(crate::jobs::blocking(async move {
            crate::attachments::read(&paths)
        }));
    }

    /// Whether images are being read to attach.
    pub(crate) fn is_attaching(&mut self) -> bool {
        self.attaching.is_pending()
    }

    /// Takes in the images read.
    pub(crate) fn poll(&mut self) {
        self.attaching.poll();
        match self.attaching.take() {
            Some(Ok(read)) => {
                let room = crate::attachments::MOST.saturating_sub(self.attachments.len());
                self.attachments.extend(read.into_iter().take(room));
            }
            Some(Err(problem)) => self.attach_problem = Some(problem),
            None => {}
        }
    }
}

/// What the panel works with: the repository's tab and its work tree, git (an API key is
/// filled through it), and the accounts besides Claude Code's own.
#[derive(Clone, Copy)]
pub(crate) struct Around<'a> {
    pub(crate) repo: RepoId,
    pub(crate) work_tree: &'a Path,
    /// The other computer the repository is on (as ssh reaches it), if it is.
    pub(crate) elsewhere: Option<&'a str>,
    pub(crate) git: Option<&'a egit_git::Git>,
    pub(crate) accounts: &'a [AgentAccount],
}

/// The panel.
pub(crate) fn panel(
    ui: &mut Ui,
    (setup, agents): (&mut AgentsSetup, &mut RepoAgents),
    around: Around<'_>,
    actions: &mut Vec<Action>,
) {
    let work_tree = around.work_tree;
    setup.look();
    head(ui, setup, agents, work_tree);
    if !setup.is_ready() {
        Frame::new()
            .inner_margin(Margin::same(INSET))
            .show(ui, |ui| setup::show(ui, setup, actions));
        return;
    }
    // Images dropped on the window go to the prompt (patches to the history's view).
    let dropped: Vec<PathBuf> = crate::desktop::dropped_files(ui.ctx())
        .into_iter()
        .filter(|path| crate::attachments::is_image(path))
        .collect();
    if !dropped.is_empty()
        && let Some(conversation) = agents.current()
    {
        conversation.view.attach(dropped);
    }
    let homes = setup.homes();
    let starter = setup.launch(&Signer::Default).map(|launch| Starter {
        launch,
        git: around.git.cloned(),
        homes,
    });
    // Started ahead of its first prompt: its models, commands, and account known at once.
    agents.warm(starter.clone(), work_tree);
    let palette = Palette::of(ui.ctx());
    let sent = egui::Panel::bottom(Id::new(("agents-composer", work_tree)))
        .resizable(false)
        .frame(
            Frame::new()
                .fill(palette.panel)
                .inner_margin(Margin::symmetric(INSET, 10)),
        )
        .show(ui, |ui| {
            let top = ui.max_rect().top() - 10.0;
            ui.painter().hline(
                ui.max_rect().x_range().expand(f32::from(INSET)),
                top + 0.5,
                Stroke::new(1.0, palette.border),
            );
            composer::show(ui, (setup, agents), around, actions)
        })
        .inner;
    if let Some(sent) = sent {
        agents.send(starter, work_tree, sent);
    }
    let Some(conversation) = agents.current() else {
        return;
    };
    egui::CentralPanel::default()
        .frame(Frame::new().fill(palette.background))
        .show(ui, |ui| transcript::show(ui, conversation, around, actions));
}

/// The head: the title, the conversations' menu, a new conversation, and hiding the panel, on
/// the band every panel's first row shares, a hairline under it.
fn head(ui: &mut Ui, setup: &mut AgentsSetup, agents: &mut RepoAgents, work_tree: &Path) {
    let palette = Palette::of(ui.ctx());
    let shown = Frame::new()
        .inner_margin(Margin::symmetric(INSET, 0))
        .show(ui, |ui| {
            widgets::panel_head(ui, Head::Control, |ui| {
                widgets::panel_title(ui, "Agents");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::icon_tool(ui, true, icon::SIDEBAR_SIMPLE, "Hide the agents")
                        .on_hover_text("Hide the agents")
                        .clicked()
                    {
                        agents.shown = false;
                    }
                    if !setup.is_ready() {
                        return;
                    }
                    let blank = agents
                        .current()
                        .is_some_and(|conversation| conversation.is_blank());
                    if widgets::icon_tool(ui, !blank, icon::PLUS, "New conversation")
                        .on_hover_text("New conversation")
                        .on_disabled_hover_text("This one is new")
                        .clicked()
                    {
                        agents.begin();
                    }
                    conversations(ui, agents, setup, work_tree);
                });
            });
        });
    let rect = shown.response.rect;
    ui.painter().hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(1.0, palette.border),
    );
}

/// The conversations' menu, named by the one shown, its words cut to the room left.
fn conversations(ui: &mut Ui, agents: &mut RepoAgents, setup: &mut AgentsSetup, work_tree: &Path) {
    let title = agents
        .current()
        .map(|conversation| conversation.title.clone())
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| "New conversation".to_owned());
    // The room left of the buttons, less the menu's icon, caret, and padding.
    let room = (ui.available_width() - 64.0).max(48.0);
    let font = egui::TextStyle::Button.resolve(ui.style());
    let title = fitted(ui, &title, &font, room);
    let launch = setup.launch(&agents.folder_signer());
    let (response, _) = widgets::tool_menu_of(
        ui,
        (icon::CHATS, &title),
        Look::Full,
        "agents-conversations",
        |ui| conversations_menu(ui, agents, launch.as_ref(), work_tree),
    );
    if response.clicked() {
        agents.list_past(launch, work_tree);
    }
}

/// The conversations' menu: a new one, those open (each closed by its ×), the earlier ones
/// Claude Code keeps for the work tree.
fn conversations_menu(
    ui: &mut Ui,
    agents: &mut RepoAgents,
    launch: Option<&egit_agents::Launch>,
    work_tree: &Path,
) {
    actions::menu_rows(ui);
    if actions::item(ui, (icon::PLUS, "New conversation"), Ok(())) {
        agents.begin();
        ui.close();
    }
    actions::menu_header(ui, "Open");
    let mut shown = None;
    let mut closed = None;
    for (index, conversation) in agents.conversations.iter().enumerate() {
        let title = if conversation.title.is_empty() {
            "New conversation".to_owned()
        } else {
            elided(&conversation.title, 48)
        };
        let glyph = if index == agents.current {
            icon::CHECK
        } else {
            icon::CHAT_CIRCLE_DOTS
        };
        ui.horizontal(|ui| {
            if actions::item_of(ui, (glyph, &title), Ok(()), "agents-conversations") {
                shown = Some(index);
            }
            if widgets::icon_tool(ui, true, icon::X, "Close the conversation")
                .on_hover_text("Close the conversation (it is kept: open it again from Earlier)")
                .clicked()
            {
                closed = Some(index);
            }
        });
    }
    if let Some(index) = shown {
        agents.current = index;
        ui.close();
    }
    if let Some(index) = closed {
        agents.close(index);
    }
    actions::menu_header(ui, "Earlier");
    let past = match agents.past.read_as_ref() {
        Some(Ok(past)) => std::sync::Arc::clone(past),
        Some(Err(problem)) => {
            ui.label(muted(ui, problem.detail.clone()));
            return;
        }
        None => {
            widgets::working(ui, "Reading…");
            return;
        }
    };
    if past.is_empty() {
        ui.label(muted(ui, "None kept for this folder"));
    }
    egui::ScrollArea::vertical()
        .max_height(320.0)
        .show(ui, |ui| {
            let now = crate::format::now();
            for session in past.iter() {
                let seconds = session.last_modified.checked_div(1000).unwrap_or(0);
                let modified = u64::try_from(seconds).unwrap_or(0);
                let title = format!(
                    "{} · {}",
                    elided(&session.summary, 48),
                    crate::format::ago(modified, now)
                );
                let glyph = icon::CLOCK_COUNTER_CLOCKWISE;
                if actions::item_of(ui, (glyph, &title), Ok(()), "agents-conversations") {
                    agents.resume(launch.cloned(), work_tree, session);
                    ui.close();
                }
            }
        });
}

/// `text`'s first line as fits in `width` points of `font`: whole, or cut with an ellipsis.
fn fitted(ui: &Ui, text: &str, font: &egui::FontId, width: f32) -> String {
    let first = first_line(text);
    ui.fonts_mut(|fonts| {
        let whole: f32 = first.chars().map(|c| fonts.glyph_width(font, c)).sum();
        if whole <= width {
            return first.to_owned();
        }
        let budget = width - fonts.glyph_width(font, '…');
        let mut used = 0.0;
        let mut end = 0;
        for (index, character) in first.char_indices() {
            used += fonts.glyph_width(font, character);
            if used > budget {
                break;
            }
            end = index.saturating_add(character.len_utf8());
        }
        format!("{}…", first.get(..end).unwrap_or_default().trim_end())
    })
}

/// A button of one icon a control's height square, named with someone's words (`name`:
/// "Remove shot.png") and explained by the help topic `topic` rather than by them.
fn icon_button_of(ui: &mut Ui, glyph: &str, name: &str, topic: &'static str) -> egui::Response {
    let size = egui::vec2(crate::widgets::TARGET, crate::widgets::TARGET);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, name));
    crate::help::note_topic(ui, rect, topic);
    let visuals = ui.style().interact(&response);
    if response.hovered() || response.is_pointer_button_down_on() {
        ui.painter()
            .rect_filled(rect, visuals.corner_radius, visuals.weak_bg_fill);
    }
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        glyph,
        egui::TextStyle::Button.resolve(ui.style()),
        visuals.text_color(),
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// `text`'s first line.
fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or_default()
}

/// `text`'s first line, at most `most` characters, an ellipsis ending it when cut.
fn elided(text: &str, most: usize) -> String {
    let first = first_line(text);
    if first.chars().count() <= most {
        return first.to_owned();
    }
    let mut cut: String = first.chars().take(most.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

/// `path` as the user reads it in a conversation: relative to the work tree when inside it.
fn shown_path<'p>(path: &'p str, work_tree: &Path) -> &'p str {
    Path::new(path)
        .strip_prefix(work_tree)
        .ok()
        .and_then(Path::to_str)
        .filter(|relative| !relative.is_empty())
        .unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_shown_from_the_work_tree() {
        let work_tree = Path::new("/home/ada/repo");
        assert_eq!(shown_path("/home/ada/repo/src/a.rs", work_tree), "src/a.rs");
        assert_eq!(
            shown_path("/home/ada/other/b.rs", work_tree),
            "/home/ada/other/b.rs"
        );
        assert_eq!(shown_path("/home/ada/repo", work_tree), "/home/ada/repo");
        assert_eq!(elided("A long title\nsecond", 6), "A lon…");
        assert_eq!(elided("Short", 6), "Short");
    }
}
