//! The command palette (⌘K, or ⇧⌘P): a field, and under it the commands whose words hold what
//! is typed (letters in order, word starts first), each with its shortcut, or why it can't run
//! now. ↑ and ↓ choose, Enter runs, Escape closes.
//!
//! What it offers is gathered as it opens (the repository's commands, recent repositories, …,
//! and a switch to each of its branches), then only narrowed while it is open. A branch is
//! offered by its place among the refs, which are shared: a repository's tens of thousands
//! cost four bytes each, its words made only for the rows drawn. A letter typed after the
//! others narrows among what they matched.

use std::borrow::Cow;
use std::sync::Arc;

use bstr::{BStr, ByteSlice as _};
use egit_git::Reference;
use egui::{Id, Key, Modal, Modifiers, Sense, Ui};

use crate::commands::{Command, RepoCommand, SWITCH_TO, score, score_letters};
use crate::theme::Palette;
use crate::widgets::muted;

/// A row's height.
const ROW_HEIGHT: f32 = 28.0;

/// The words before a ref's name in an offer going to its commit.
const GO_TO: &str = "Go to ";

/// The most rows shown at once.
const ROWS_SHOWN: f32 = 12.0;

/// One command offered, in its words.
#[derive(Clone, Debug)]
pub(crate) struct Offer {
    pub(crate) command: Command,
    pub(crate) label: String,
    /// Its shortcut, as this platform names its keys.
    pub(crate) keys: Option<Cow<'static, str>>,
    /// Whether it can run now, and why not.
    pub(crate) allowed: Result<(), &'static str>,
}

/// What the palette offers: commands, then a switch to each of some branches, then going to
/// each remote's branch and tag (its commit shown in the history).
#[derive(Debug)]
pub(crate) struct Offers {
    pub(crate) commands: Vec<Offer>,
    /// The refs read, and the places among them of the branches offered.
    pub(crate) refs: Arc<[Reference]>,
    pub(crate) branches: Vec<u32>,
    /// The places among them of the remotes' branches and the tags offered to go to.
    pub(crate) goes: Vec<u32>,
    /// Whether a switch can happen now, and why not.
    pub(crate) switching: Result<(), &'static str>,
}

impl Default for Offers {
    fn default() -> Self {
        Self {
            commands: Vec::new(),
            refs: Arc::from(Vec::new()),
            branches: Vec::new(),
            goes: Vec::new(),
            switching: Ok(()),
        }
    }
}

impl Offers {
    const fn len(&self) -> usize {
        self.commands
            .len()
            .saturating_add(self.branches.len())
            .saturating_add(self.goes.len())
    }

    /// The offer at `at`.
    fn get(&self, at: usize) -> Option<Item<'_>> {
        if let Some(offer) = self.commands.get(at) {
            return Some(Item::Command(offer));
        }
        let at = at.checked_sub(self.commands.len())?;
        if let Some(place) = self.branches.get(at) {
            let reference = self.refs.get(usize::try_from(*place).ok()?)?;
            return Some(Item::Switch(reference.short_name(), self.switching));
        }
        let place = self.goes.get(at.checked_sub(self.branches.len())?)?;
        let reference = self.refs.get(usize::try_from(*place).ok()?)?;
        Some(Item::GoTo(reference))
    }
}

/// One offer, as it is scored and drawn.
#[derive(Clone, Copy)]
enum Item<'a> {
    Command(&'a Offer),
    /// A switch to this branch (its short name), and whether it can happen now.
    Switch(&'a BStr, Result<(), &'static str>),
    /// Going to this ref's commit (a remote's branch, a tag).
    GoTo(&'a Reference),
}

impl<'a> Item<'a> {
    fn score(self, query: &str) -> Option<u32> {
        match self {
            Self::Command(offer) => score(query, &offer.label),
            Self::Switch(name, _) => score_letters(query, SWITCH_TO.chars().chain(name.chars())),
            Self::GoTo(reference) => {
                score_letters(query, GO_TO.chars().chain(reference.short_name().chars()))
            }
        }
    }

    /// Its words (a branch's made now: only rows drawn ask).
    fn label(self) -> Cow<'a, str> {
        match self {
            Self::Command(offer) => Cow::Borrowed(&offer.label),
            Self::Switch(name, _) => Cow::Owned(format!("{SWITCH_TO}{name}")),
            Self::GoTo(reference) => Cow::Owned(format!("{GO_TO}{}", reference.short_name())),
        }
    }

    fn keys(self) -> Option<&'a str> {
        match self {
            Self::Command(offer) => offer.keys.as_deref(),
            Self::Switch(..) | Self::GoTo(_) => None,
        }
    }

    const fn allowed(self) -> Result<(), &'static str> {
        match self {
            Self::Command(offer) => offer.allowed,
            Self::Switch(_, allowed) => allowed,
            Self::GoTo(_) => Ok(()),
        }
    }

    fn command(self) -> Command {
        match self {
            Self::Command(offer) => offer.command.clone(),
            Self::Switch(name, _) => Command::Repo(RepoCommand::Switch(name.to_owned())),
            Self::GoTo(reference) => Command::Repo(RepoCommand::GoTo(reference.target)),
        }
    }
}

/// The palette's state.
#[derive(Debug, Default)]
pub(crate) struct PaletteView {
    open: bool,
    query: String,
    /// The query `matching` was narrowed for: one typed further narrows among those.
    narrowed: String,
    /// The offers as it opened, and those matching the query (by place), best first.
    offers: Offers,
    matching: Vec<usize>,
    selected: usize,
    /// Whether the field should take the focus (as it opens).
    focus: bool,
    /// Its keys this frame, taken before the views below could see them.
    keys: Keys,
}

/// The palette's keys pressed this frame.
#[derive(Clone, Copy, Debug, Default)]
struct Keys {
    /// Rows moved down (↓), less those moved up (↑).
    moved: isize,
    enter: bool,
    escape: bool,
}

/// Takes the palette's keys (↑, ↓, Enter, Escape) while it is open, before anything else this
/// frame sees them: the views below it would otherwise act on them too (Escape leaving the
/// reflog, ↑ moving through the commits).
pub(crate) fn take_keys(ctx: &egui::Context, view: &mut PaletteView) {
    if !view.open {
        return;
    }
    view.keys = ctx.input_mut(|input| {
        let up = input.consume_key(Modifiers::NONE, Key::ArrowUp);
        let down = input.consume_key(Modifiers::NONE, Key::ArrowDown);
        Keys {
            moved: match (up, down) {
                (true, false) => -1,
                (false, true) => 1,
                _ => 0,
            },
            enter: input.consume_key(Modifiers::NONE, Key::Enter),
            escape: input.consume_key(Modifiers::NONE, Key::Escape),
        }
    });
}

impl PaletteView {
    /// Opens it, offering `offers`.
    pub(crate) fn open(&mut self, offers: Offers) {
        self.open = true;
        self.query.clear();
        self.offers = offers;
        self.selected = 0;
        self.focus = true;
        self.narrow();
    }

    /// Whether it is open.
    pub(crate) const fn is_open(&self) -> bool {
        self.open
    }

    /// Ranks the offers by how well they match the query (unmatched ones left out), best
    /// first, the offers' own order among equals.
    fn narrow(&mut self) {
        let query = self.query.trim();
        if query.is_empty() {
            self.matching.clear();
            self.matching.extend(0..self.offers.len());
        } else {
            let offers = &self.offers;
            let rank = |at: usize| Some((offers.get(at)?.score(query)?, at));
            // Letters typed after the others match only where those did.
            let further = !self.narrowed.is_empty() && query.starts_with(self.narrowed.as_str());
            let mut ranked: Vec<(u32, usize)> = if further {
                self.matching.iter().copied().filter_map(rank).collect()
            } else {
                (0..offers.len()).filter_map(rank).collect()
            };
            ranked.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            self.matching.clear();
            self.matching.extend(ranked.into_iter().map(|(_, at)| at));
        }
        self.narrowed.clear();
        self.narrowed.push_str(query);
        self.selected = 0;
    }
}

/// Draws the palette while it is open; returns the command chosen (it closes then).
pub(crate) fn show(ctx: &egui::Context, view: &mut PaletteView) -> Option<Command> {
    if !view.open {
        return None;
    }
    let palette = Palette::of(ctx);
    let frame = egui::Frame::new()
        .fill(palette.surface)
        .stroke(egui::Stroke::new(1.0, palette.border))
        .corner_radius(egui::CornerRadius::same(crate::theme::RADIUS_CARD))
        .inner_margin(egui::Margin::same(14));
    let mut chosen = None;
    // Hung from near the top: its field stays put as what matches narrows.
    let id = Id::new("command-palette");
    let area = Modal::default_area(id).anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 96.0));
    let shown = Modal::new(id).area(area).frame(frame).show(ctx, |ui| {
        ui.set_width(560.0);
        chosen = contents(ui, view);
    });
    if shown.should_close() || chosen.is_some() {
        view.open = false;
    }
    chosen
}

/// The field, the keys, and the rows; the command chosen, if one was.
fn contents(ui: &mut Ui, view: &mut PaletteView) -> Option<Command> {
    let label = ui.label(muted(ui, "Command, branch, or repository"));
    let field = ui
        .add(
            crate::widgets::line_field(&mut view.query)
                .hint_text("Type to narrow; ↑ ↓ to choose; Enter to run")
                .desired_width(f32::INFINITY),
        )
        .labelled_by(label.id);
    if std::mem::take(&mut view.focus) {
        field.request_focus();
    }
    if field.changed() {
        view.narrow();
    }
    let count = view.matching.len();
    let Keys {
        moved,
        enter,
        escape,
    } = std::mem::take(&mut view.keys);
    if escape {
        view.open = false;
        return None;
    }
    view.selected = view
        .selected
        .saturating_add_signed(moved)
        .min(count.saturating_sub(1));
    let mut chosen = view
        .matching
        .get(view.selected)
        .and_then(|at| view.offers.get(*at))
        .filter(|offer| enter && offer.allowed().is_ok())
        .map(Item::command);
    ui.add_space(6.0);
    if count == 0 {
        ui.label(muted(ui, "Nothing matches."));
        return chosen;
    }
    let pitch = ROW_HEIGHT + ui.spacing().item_spacing.y;
    let height =
        (pitch * ROWS_SHOWN).min(pitch * f32::from(u16::try_from(count).unwrap_or(u16::MAX)));
    let mut scroll = egui::ScrollArea::vertical()
        .id_salt("command-palette-rows")
        .max_height(height)
        .auto_shrink([false, true]);
    if moved != 0 {
        // Keep the chosen row in view.
        let row = f32::from(u16::try_from(view.selected).unwrap_or(u16::MAX));
        let offset = ui
            .ctx()
            .data(|data| data.get_temp::<f32>(Id::new("command-palette-offset")))
            .unwrap_or(0.0);
        let top = row * pitch;
        if top < offset {
            scroll = scroll.vertical_scroll_offset(top);
        } else if top + pitch > offset + height {
            scroll = scroll.vertical_scroll_offset(top + pitch - height);
        }
    }
    let shown = scroll.show_rows(ui, ROW_HEIGHT, count, |ui, rows| {
        for row in rows {
            if let Some(command) = offer_row(ui, view, row) {
                chosen = Some(command);
            }
        }
    });
    ui.ctx().data_mut(|data| {
        data.insert_temp(Id::new("command-palette-offset"), shown.state.offset.y);
    });
    chosen
}

/// One offer's row: its words, and its keys or why it can't run; a click runs it.
fn offer_row(ui: &mut Ui, view: &mut PaletteView, row: usize) -> Option<Command> {
    let offer = view.matching.get(row).and_then(|at| view.offers.get(*at))?;
    let palette = Palette::of(ui.ctx());
    let selected = view.selected == row;
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_HEIGHT), Sense::hover());
    let response = ui.interact(rect, Id::new(("command-palette-row", row)), Sense::click());
    let allowed = offer.allowed();
    let enabled = allowed.is_ok();
    let label = offer.label();
    // Named with why it can't run, which is painted beside it.
    response.widget_info(|| {
        let name = match allowed {
            Ok(()) => Cow::Borrowed(&*label),
            Err(why) => Cow::Owned(format!("{label}: {why}")),
        };
        egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, enabled, selected, name)
    });
    crate::widgets::row_fill(ui, rect, 6.0, selected, response.hovered());
    let text = if enabled { palette.text } else { palette.muted };
    let font = egui::TextStyle::Body.resolve(ui.style());
    ui.painter().text(
        egui::pos2(rect.left() + 10.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        &*label,
        font.clone(),
        text,
    );
    let aside = match allowed {
        Err(why) => Some(why),
        Ok(()) => offer.keys(),
    };
    if let Some(aside) = aside {
        ui.painter().text(
            egui::pos2(rect.right() - 10.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            aside,
            font,
            palette.muted,
        );
    }
    let chosen = (response.clicked() && enabled).then(|| offer.command());
    // The pointer picks a row as it moves over it; resting, it leaves ↑ ↓ their choice.
    if response.hovered() && ui.input(|input| input.pointer.delta() != egui::Vec2::ZERO) {
        view.selected = row;
    }
    chosen
}

#[cfg(test)]
mod tests {
    use egit_git::{ObjectId, RefKind};

    use super::*;

    /// A palette offering a few commands and switches to `branches`.
    fn offering(branches: &[&str]) -> PaletteView {
        let refs: Vec<Reference> = branches
            .iter()
            .map(|name| Reference {
                name: format!("refs/heads/{name}").into(),
                kind: RefKind::Branch,
                target: ObjectId::from_hex("ab".repeat(20).as_bytes()).unwrap(),
            })
            .collect();
        let commands = [RepoCommand::Fetch, RepoCommand::Search, RepoCommand::Reflog]
            .into_iter()
            .map(|command| {
                let command = Command::Repo(command);
                Offer {
                    label: command.label("pull requests"),
                    keys: None,
                    command,
                    allowed: Ok(()),
                }
            })
            .collect();
        let mut view = PaletteView::default();
        view.open(Offers {
            commands,
            branches: (0..u32::try_from(refs.len()).unwrap()).collect(),
            goes: Vec::new(),
            refs: Arc::from(refs),
            switching: Ok(()),
        });
        view
    }

    fn labels(view: &PaletteView) -> Vec<String> {
        view.matching
            .iter()
            .map(|at| view.offers.get(*at).unwrap().label().into_owned())
            .collect()
    }

    #[test]
    fn letters_typed_one_at_a_time_narrow_as_the_whole_query_would() {
        let branches = ["feature/search", "fix-sidebar", "release", "feature/reflog"];
        let mut typed = offering(&branches);
        assert_eq!(
            labels(&typed).len(),
            7,
            "everything, before anything is typed"
        );
        for letter in "sw feat".chars() {
            typed.query.push(letter);
            typed.narrow();
        }
        let mut whole = offering(&branches);
        whole.query.push_str("sw feat");
        whole.narrow();
        assert_eq!(labels(&typed), labels(&whole));
        assert_eq!(
            labels(&typed),
            ["Switch to feature/search", "Switch to feature/reflog"]
        );
        // A letter taken back starts over.
        typed.query.pop();
        typed.narrow();
        assert!(labels(&typed).len() >= 2);
        assert_eq!(typed.offers.get(typed.matching[0]).unwrap().command(), {
            Command::Repo(RepoCommand::Switch("feature/search".into()))
        });
    }
}
