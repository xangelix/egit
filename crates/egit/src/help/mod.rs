//! Help: what each thing egit offers does at git's level, and what happens in the cases that
//! surprise people (cancelling a rebase, editing a pushed commit's message, a submodule that
//! won't sit still), as topics.
//!
//! Two ways in. Holding Alt while pointing at a control shows its topic beside it: the shared
//! controls (`widgets`, the menus' items) note where they are and what they say as they are
//! drawn, while Alt is held only (nothing is kept otherwise), and custom ones (a commit's row,
//! a badge, the graph) note their topic outright; at the frame's end the topmost one under the
//! pointer is found and its topic drawn. And F1 (or the status bar's Help) opens every topic,
//! searchable.

use std::sync::Arc;

use egit_git::sync::Mutex;
use egui::{Align2, Color32, Context, Frame, Id, LayerId, Margin, Order, Pos2, Rect, RichText, Ui};

use crate::theme::Weighted as _;
use crate::theme::{Palette, icon};

/// Topics, by kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Group {
    Everyday,
    Changes,
    Branches,
    Rewriting,
    Remotes,
    Conflicts,
    Stashes,
    Inside,
    Looking,
    Agents,
    Egit,
}

impl Group {
    /// Every group, in the order shown.
    pub(crate) const ALL: [Self; 11] = [
        Self::Everyday,
        Self::Changes,
        Self::Branches,
        Self::Remotes,
        Self::Rewriting,
        Self::Conflicts,
        Self::Stashes,
        Self::Inside,
        Self::Looking,
        Self::Agents,
        Self::Egit,
    ];

    /// Its name.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Everyday => "Every day",
            Self::Changes => "Changes and diffs",
            Self::Branches => "Branches and tags",
            Self::Rewriting => "Rewriting history",
            Self::Remotes => "Remotes",
            Self::Conflicts => "Conflicts",
            Self::Stashes => "Stashes and patches",
            Self::Inside => "Submodules, LFS, worktrees",
            Self::Looking => "Looking into history",
            Self::Agents => "Agents",
            Self::Egit => "egit itself",
        }
    }
}

/// A topic: what something does, the git that does it, and the cases that surprise.
#[derive(Debug)]
pub(crate) struct Topic {
    pub(crate) id: &'static str,
    pub(crate) group: Group,
    pub(crate) title: &'static str,
    /// The words of the controls it explains: whole, or with a `*` standing for any words
    /// there (`"Merge * into *"`, `"Stage *"`); the pattern naming more of a control's words
    /// wins.
    pub(crate) on: &'static [&'static str],
    /// What it does, at git's level.
    pub(crate) what: &'static str,
    /// The git commands egit runs for it (none for what is egit's own).
    pub(crate) git: &'static str,
    /// Questions people meet, and their answers.
    pub(crate) cases: &'static [(&'static str, &'static str)],
}

mod agents;
mod branches;
mod history;
mod inside;
mod remotes;
mod rewriting;
mod working;

/// Every area's topics.
const AREAS: [&[Topic]; 7] = [
    working::TOPICS,
    history::TOPICS,
    branches::TOPICS,
    remotes::TOPICS,
    rewriting::TOPICS,
    inside::TOPICS,
    agents::TOPICS,
];

/// Every topic.
pub(crate) fn topics() -> impl Iterator<Item = &'static Topic> {
    AREAS.into_iter().flatten()
}

/// The topic named `id`.
pub(crate) fn topic(id: &str) -> Option<&'static Topic> {
    topics().find(|topic| topic.id == id)
}

/// The topic a control's words point to: the one whose pattern fits them best (see
/// [`Topic::on`]). Looked up only for what is under the pointer while Alt is held.
pub(crate) fn for_label(label: &str) -> Option<&'static Topic> {
    let words = words_of(label);
    // A commit's name, wherever one is a control (a parent, a blamed line's commit).
    if (7..=64).contains(&words.len()) && words.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return topic("commit-name");
    }
    topics()
        .flat_map(|topic| {
            topic
                .on
                .iter()
                .filter_map(move |pattern| Some((fit(pattern, words)?, topic)))
        })
        .max_by_key(|(fit, _)| *fit)
        .map(|(_, topic)| topic)
}

/// A control's words, without the icon a tool's start with ("\u{e13a}  Push").
fn words_of(label: &str) -> &str {
    let label = label.trim();
    label.split_once("  ").map_or(label, |(icon, words)| {
        if icon.chars().count() <= 2 {
            words
        } else {
            label
        }
    })
}

/// How well `pattern` fits `words`: whole words best, else by how many of them the pattern
/// names around its `*`s (each standing for any words, in order); `None` when it doesn't.
fn fit(pattern: &str, words: &str) -> Option<usize> {
    if !pattern.contains('*') {
        return (pattern == words).then_some(usize::MAX);
    }
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or_default();
    let mut rest = words.strip_prefix(first)?;
    let mut named = first.len();
    let parts: Vec<&str> = parts.collect();
    let (last, middle) = parts.split_last()?;
    for part in middle {
        let at = rest.find(part)?;
        rest = rest.get(at.saturating_add(part.len())..)?;
        named = named.saturating_add(part.len());
    }
    rest.ends_with(last)
        .then(|| named.saturating_add(last.len()))
}

/// The help window: every topic, by group, searchable.
#[derive(Debug)]
pub(crate) struct HelpWindow {
    open: bool,
    query: String,
    selected: &'static str,
    /// The list's rows, and the query they were made for: made again only when it changes.
    listed: Vec<Listed>,
    listed_for: Option<String>,
}

/// A row of the topics' list: a group's name, or a topic.
#[derive(Clone, Copy, Debug)]
enum Listed {
    Group(Group),
    Topic(&'static Topic),
}

impl Default for HelpWindow {
    fn default() -> Self {
        Self {
            open: false,
            query: String::new(),
            selected: "start",
            listed: Vec::new(),
            listed_for: None,
        }
    }
}

impl HelpWindow {
    /// Opens it, or closes it.
    pub(crate) const fn toggle(&mut self) {
        self.open = !self.open;
    }

    /// Whether it is open.
    #[cfg(test)]
    pub(crate) const fn is_open(&self) -> bool {
        self.open
    }
}

/// Draws the help window, while open: the topics on the left (those whose words hold what is
/// typed), the one selected on the right.
pub(crate) fn window(ctx: &Context, help: &mut HelpWindow) {
    if !help.open {
        return;
    }
    let palette = Palette::of(ctx);
    let screen = ctx.content_rect();
    let size = egui::vec2(
        (screen.width() - 48.0).clamp(320.0, 900.0),
        (screen.height() - 96.0).clamp(240.0, 620.0),
    );
    // Framed as every dialog is: the surface, an edge, a card's corners, room inside.
    let frame = egui::Frame::new()
        .fill(palette.surface)
        .stroke(egui::Stroke::new(1.0, palette.border))
        .corner_radius(egui::CornerRadius::same(crate::theme::RADIUS_CARD))
        .inner_margin(egui::Margin::same(20));
    let modal = egui::Modal::new(Id::new("help-window"))
        .frame(frame)
        .show(ctx, |ui| {
            ui.set_width(size.x);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{}  Help", icon::QUESTION))
                        .size(crate::widgets::PANEL_TITLE)
                        .semibold(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if crate::widgets::icon_tool(ui, true, icon::X, "Close").clicked() {
                        help.open = false;
                    }
                    ui.label(crate::widgets::muted(
                        ui,
                        "Hold Alt over anything to see its topic",
                    ));
                });
            });
            ui.add_space(8.0);
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(240.0, size.y),
                    egui::Layout::top_down_justified(egui::Align::Min),
                    |ui| topics_list(ui, help, palette),
                );
                ui.separator();
                egui::ScrollArea::vertical()
                    .id_salt("help-topic")
                    .max_height(size.y)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.vertical(|ui| {
                            if let Some(topic) = topic(help.selected) {
                                body(ui, topic, palette);
                            }
                        });
                    });
            });
        });
    if modal.should_close() {
        help.open = false;
    }
}

/// A row of the topics' list, as tall as a control.
const LIST_ROW: f32 = crate::theme::CONTROL;

/// The topics, by group, those holding what is typed, a row at a time (only those in view
/// drawn: there are hundreds).
fn topics_list(ui: &mut Ui, help: &mut HelpWindow, palette: Palette) {
    let label = ui.label(crate::widgets::muted(ui, "Find a topic"));
    ui.add(crate::widgets::line_field(&mut help.query).hint_text("rebase, submodule, push…"))
        .labelled_by(label.id);
    let query = help.query.trim().to_lowercase();
    if help.listed_for.as_deref() != Some(query.as_str()) {
        help.listed = listed(&query);
        help.listed_for = Some(query);
    }
    ui.add_space(4.0);
    ui.spacing_mut().item_spacing.y = 0.0;
    let font = egui::TextStyle::Body.resolve(ui.style());
    egui::ScrollArea::vertical()
        .id_salt("help-topics")
        .auto_shrink([false, false])
        .show_rows(ui, LIST_ROW, help.listed.len(), |ui, rows| {
            for row in help.listed.get(rows).unwrap_or_default() {
                let (rect, response) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), LIST_ROW),
                    match row {
                        Listed::Group(_) => egui::Sense::hover(),
                        Listed::Topic(_) => egui::Sense::click(),
                    },
                );
                match row {
                    Listed::Group(group) => {
                        let words = RichText::new(group.name())
                            .small()
                            .semibold()
                            .color(palette.muted);
                        let galley = egui::WidgetText::from(words).into_galley(
                            ui,
                            Some(egui::TextWrapMode::Truncate),
                            rect.width(),
                            egui::TextStyle::Small,
                        );
                        let at =
                            Pos2::new(rect.left() + 4.0, rect.bottom() - galley.size().y - 4.0);
                        ui.painter().galley(at, galley, palette.muted);
                    }
                    Listed::Topic(topic) => {
                        let selected = help.selected == topic.id;
                        response.widget_info(|| {
                            egui::WidgetInfo::selected(
                                egui::WidgetType::SelectableLabel,
                                true,
                                selected,
                                topic.title,
                            )
                        });
                        crate::widgets::row_fill(
                            ui,
                            rect.shrink2(egui::vec2(0.0, 1.0)),
                            f32::from(crate::theme::RADIUS_INNER),
                            selected,
                            response.hovered(),
                        );
                        let galley = ui.painter().layout(
                            topic.title.to_owned(),
                            font.clone(),
                            palette.text,
                            rect.width() - 16.0,
                        );
                        let at =
                            Pos2::new(rect.left() + 8.0, rect.center().y - galley.size().y / 2.0);
                        ui.painter()
                            .with_clip_rect(rect)
                            .galley(at, galley, palette.text);
                        if response.clicked() {
                            help.selected = topic.id;
                        }
                    }
                }
            }
        });
}

/// The list's rows for `query` (lower case): each group's name, then its topics holding it.
fn listed(query: &str) -> Vec<Listed> {
    let holds = |text: &str| text.to_lowercase().contains(query);
    let matches = |topic: &Topic| {
        query.is_empty()
            || [topic.title, topic.what, topic.git].into_iter().any(holds)
            || topic.on.iter().any(|words| holds(words))
            || topic
                .cases
                .iter()
                .any(|(question, answer)| holds(question) || holds(answer))
    };
    let mut rows = Vec::new();
    for group in Group::ALL {
        let start = rows.len();
        rows.extend(
            topics()
                .filter(|topic| topic.group == group && matches(topic))
                .map(Listed::Topic),
        );
        if rows.len() > start {
            rows.insert(start, Listed::Group(group));
        }
    }
    rows
}

/// What was noted this frame: each control's place, its layer, and what it says or its topic.
#[derive(Clone, Default)]
struct Noted(Arc<Mutex<Vec<(Rect, LayerId, Said)>>>);

/// A noted control's words, or its topic.
#[derive(Clone)]
enum Said {
    Words(String),
    Topic(&'static str),
}

fn noted(ctx: &Context) -> Noted {
    ctx.data_mut(|data| {
        data.get_temp_mut_or_default::<Noted>(Id::new("help-noted"))
            .clone()
    })
}

/// Whether Alt is held: only then is anything noted.
fn asking(ui: &Ui) -> bool {
    ui.input(|input| input.modifiers.alt)
}

/// Notes a control drawn at `rect` saying `words`, while Alt is held.
pub(crate) fn note(ui: &Ui, rect: Rect, words: &str) {
    #[cfg(test)]
    {
        let seen = seen(ui.ctx());
        let mut seen = seen.lock();
        if !seen.contains(words) {
            seen.insert(words.to_owned());
        }
    }
    if asking(ui) {
        noted(ui.ctx())
            .0
            .lock()
            .push((rect, ui.layer_id(), Said::Words(words.to_owned())));
    }
}

/// Every control's words drawn so far (tests: each must find its topic).
#[cfg(test)]
pub(crate) fn seen(ctx: &Context) -> Arc<Mutex<std::collections::BTreeSet<String>>> {
    ctx.data_mut(|data| {
        data.get_temp_mut_or_default::<Arc<Mutex<std::collections::BTreeSet<String>>>>(Id::new(
            "help-seen",
        ))
        .clone()
    })
}

/// Notes something drawn at `rect` whose topic is `id`, while Alt is held.
pub(crate) fn note_topic(ui: &Ui, rect: Rect, id: &'static str) {
    if asking(ui) {
        noted(ui.ctx())
            .0
            .lock()
            .push((rect, ui.layer_id(), Said::Topic(id)));
    }
}

/// At a frame's start: what was noted goes, and tooltips wait while Alt is held (the topic
/// takes their place).
pub(crate) fn begin_frame(ctx: &Context) {
    noted(ctx).0.lock().clear();
    let alt = ctx.input(|input| input.modifiers.alt);
    let delay = if alt {
        f32::MAX
    } else {
        crate::theme::TOOLTIP_DELAY
    };
    let current = ctx.global_style().interaction.tooltip_delay;
    if (current - delay).abs() > f32::EPSILON {
        ctx.all_styles_mut(|style| style.interaction.tooltip_delay = delay);
    }
}

/// At a frame's end, with Alt held: the topic of the topmost control under the pointer (the
/// smallest among those noted on the topmost layer there), beside it.
pub(crate) fn show(ctx: &Context) {
    if !ctx.input(|input| input.modifiers.alt) {
        return;
    }
    let Some(pointer) = ctx.pointer_hover_pos() else {
        return;
    };
    let layer = ctx.layer_id_at(pointer);
    let found = {
        let noted = noted(ctx);
        let noted = noted.0.lock();
        noted
            .iter()
            .filter(|(rect, at, _)| rect.contains(pointer) && Some(*at) == layer)
            .filter_map(|(rect, _, said)| {
                // A topic named outright wins over words at the same place: a view names one
                // where a common word ("Check out") means something of its own.
                let (topic, outright) = match said {
                    Said::Words(words) => (for_label(words), false),
                    Said::Topic(id) => (topic(id), true),
                };
                Some(((rect.area(), !outright), *rect, topic?))
            })
            .min_by(|a, b| a.0.0.total_cmp(&b.0.0).then(a.0.1.cmp(&b.0.1)))
            .map(|(_, rect, topic)| (rect, topic))
    };
    let Some((rect, topic)) = found else {
        return;
    };
    let palette = Palette::of(ctx);
    // Beside the control, below it where there is room.
    let at = Pos2::new(rect.left(), rect.bottom() + 6.0);
    egui::Area::new(Id::new("help-osd"))
        .order(Order::Tooltip)
        .fixed_pos(at)
        .constrain(true)
        .interactable(false)
        .pivot(Align2::LEFT_TOP)
        .show(ctx, |ui| osd(ui, topic, palette));
}

/// A topic, as the Alt overlay shows it.
fn osd(ui: &mut Ui, topic: &Topic, palette: Palette) {
    Frame::new()
        .fill(palette.surface)
        .stroke(egui::Stroke::new(1.5, palette.accent))
        .corner_radius(crate::theme::RADIUS_BOX)
        .inner_margin(Margin::same(14))
        .shadow(ui.style().visuals.popup_shadow)
        .show(ui, |ui| {
            ui.set_max_width(420.0);
            body(ui, topic, palette);
            ui.add_space(8.0);
            ui.label(
                RichText::new(format!(
                    "{}  Alt shows this anywhere · F1 shows every topic",
                    icon::INFO
                ))
                .small()
                .color(palette.muted),
            );
        });
}

/// A topic's title, what it does, its git, and its cases.
pub(crate) fn body(ui: &mut Ui, topic: &Topic, palette: Palette) {
    ui.label(
        RichText::new(topic.title)
            .size(crate::theme::HEADING)
            .semibold(),
    );
    ui.add_space(4.0);
    ui.add(egui::Label::new(topic.what).wrap());
    if !topic.git.is_empty() {
        ui.add_space(6.0);
        ui.add(
            egui::Label::new(
                // In code's family at the small size (`small` alone would set the body's
                // family back): `--recursive` reads as two dashes.
                RichText::new(format!("egit runs: {}", topic.git))
                    .family(egui::FontFamily::Monospace)
                    .size(crate::theme::SMALL)
                    .color(palette.muted),
            )
            .wrap(),
        );
    }
    if topic.cases.is_empty() {
        return;
    }
    ui.add_space(8.0);
    for (question, answer) in topic.cases {
        ui.add(egui::Label::new(RichText::new(*question).medium().color(palette.text)).wrap());
        ui.add(egui::Label::new(RichText::new(*answer).color(text_soft(palette))).wrap());
        ui.add_space(4.0);
    }
}

/// An answer's colour: the muted text's, which reads on every surface.
const fn text_soft(palette: Palette) -> Color32 {
    palette.muted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_topic_is_named_once_and_said_whole() {
        let all: Vec<&Topic> = topics().collect();
        for (at, topic) in all.iter().enumerate() {
            assert!(
                all.iter().skip(at + 1).all(|other| other.id != topic.id),
                "{} twice",
                topic.id
            );
            assert!(
                !topic.title.is_empty() && topic.what.len() > 40,
                "{}",
                topic.id
            );
            for text in [topic.what, topic.git]
                .into_iter()
                .chain(topic.cases.iter().flat_map(|(q, a)| [*q, *a]))
            {
                assert!(!text.contains("  "), "{}: doubled spaces", topic.id);
                assert!(text.trim() == text, "{}: spaces at an end", topic.id);
            }
        }
        for group in Group::ALL {
            assert!(
                all.iter().any(|topic| topic.group == group),
                "{}",
                group.name()
            );
        }
    }

    #[test]
    fn each_control_s_words_are_claimed_once() {
        let mut claimed: Vec<(&str, &str)> = topics()
            .flat_map(|topic| topic.on.iter().map(move |words| (*words, topic.id)))
            .collect();
        claimed.sort_unstable();
        for pair in claimed.windows(2) {
            if let [(a, first), (b, second)] = pair {
                assert!(a != b, "{a:?} claimed by {first} and {second}");
            }
        }
    }

    #[test]
    fn patterns_fit_by_how_many_words_they_name() {
        assert_eq!(fit("Push", "Push"), Some(usize::MAX));
        assert_eq!(fit("Push", "Push it"), None);
        assert_eq!(fit("Stage *", "Stage src/main.rs"), Some(6));
        assert_eq!(fit("Merge * into *", "Merge topic into main"), Some(12));
        assert_eq!(fit("Merge * into *", "Merge topic"), None);
        assert_eq!(
            fit("*, keeping merges", "Rebase onto main, keeping merges"),
            Some(16)
        );
        assert_eq!(fit("a*a", "a"), None, "the ends never overlap");
    }

    #[test]
    fn controls_find_their_topics_by_their_words() {
        let id = |label: &str| for_label(label).map(|topic| topic.id);
        assert_eq!(id("Push"), Some("push"));
        assert_eq!(
            id("\u{e13a}  Push"),
            Some("push"),
            "a tool's icon is skipped"
        );
        assert_eq!(id("Merge topic into main"), Some("merge"));
        assert_eq!(
            id("Merge topic into main (no fast-forward)"),
            Some("merge-no-ff"),
            "the pattern naming more of its words"
        );
        assert_eq!(id("Rebase interactively from here…"), Some("interactive"));
        assert_eq!(id("Mend everything"), Some("submodule-mend"));
        assert_eq!(id("Stage src/main.rs"), Some("stage"));
        assert_eq!(id("Nothing at all"), None);
        assert_eq!(id("1a2b3c4d"), Some("commit-name"), "a commit's name");
    }

    /// Every topic a view names outright (`note_topic(…, "id")`) exists.
    #[test]
    fn topics_named_in_the_views_exist() {
        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut folders = vec![source];
        let mut named = 0_usize;
        while let Some(folder) = folders.pop() {
            for entry in std::fs::read_dir(folder).unwrap() {
                let path = entry.unwrap().path();
                // Help's own code names topics only through what it is given.
                if path.is_dir() && !path.ends_with("help") {
                    folders.push(path);
                    continue;
                }
                if path.is_dir() {
                    continue;
                }
                let text = std::fs::read_to_string(&path).unwrap();
                for call in text.split("note_topic(").skip(1) {
                    // The call's last argument, when it is written out.
                    let call = call.split(';').next().unwrap();
                    let Some(id) = call.rsplit('"').nth(1) else {
                        continue;
                    };
                    if call.trim_end().ends_with("\")") {
                        named += 1;
                        assert!(topic(id).is_some(), "{}: {id}", path.display());
                    }
                }
            }
        }
        for at in 0..5 {
            let id = crate::views::history::column_topic(at);
            assert!(topic(id).is_some(), "{id}");
        }
        assert!(named > 10, "{named} found");
    }
}
