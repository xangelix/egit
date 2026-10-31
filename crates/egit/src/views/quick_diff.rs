//! Quick diff: a tab comparing any two texts, which has nothing to do with a repository. Two
//! texts are pasted or typed, and shown as any file's diff is: the same document and grid, read
//! from `git diff --no-index` (`egit_git::text_diff`), unified or side by side, with its whole
//! text or ignoring whitespace. A page at a time: the texts, then what differs between them.
//!
//! A field draws its whole text every frame (about 0.03 ms a KiB, measured), so one holds at most
//! [`LONG`]: two then spend under 4 ms of a frame. A longer paste never reaches it: taken from
//! the frame's events first, it is shown on the text grid, which draws only what is on screen,
//! to be compared or cleared rather than edited.
//!
//! Nothing here is kept: the texts go with the tab.

use std::sync::Arc;

use bstr::BString;
use egit_git::{CancelToken, DiffOptions, Git, WHOLE_FILE};
use egit_text::Text;
use egui::text_edit::TextEditState;
use egui::{Event, Id, Key, Label, Layout, Modifiers, Rect, RichText, Sense, Ui, UiBuilder, vec2};
use egui_async::Bind;

use super::file::{placeholder, show_sides};
use crate::app::AppState;
use crate::commands::keys_text;
use crate::diff::{DiffDocument, SplitDiff, read_text_diff};
use crate::grid::{GridSource as _, Plain, TextGrid};
use crate::preferences::Preferences;
use crate::problem::Problem;
use crate::theme::{CONTROL, Palette, icon};
use crate::widgets::{self, muted};

/// The most a field holds, in bytes.
const LONG: usize = 64 * 1024;

/// The room between the two texts, and around them.
const GAP: f32 = 12.0;

/// One of the two texts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Which {
    Before,
    After,
}

impl Which {
    const fn other(self) -> Self {
        match self {
            Self::Before => Self::After,
            Self::After => Self::Before,
        }
    }
}

/// A text past [`LONG`]: measured in a job, then shown on the grid.
struct Long {
    read: Bind<Arc<Plain>, Problem>,
    grid: TextGrid,
}

/// One of the two texts as it stands.
struct Side {
    /// What it is called.
    words: &'static str,
    /// Its field.
    field: Id,
    /// What its field holds.
    text: String,
    /// What it holds instead, too long for a field.
    long: Option<Long>,
}

impl Side {
    fn new(words: &'static str) -> Self {
        Self {
            words,
            field: Id::new(("quick-diff-field", words)),
            text: String::new(),
            long: None,
        }
    }

    /// Whether it holds nothing.
    const fn is_empty(&self) -> bool {
        self.long.is_none() && self.text.is_empty()
    }

    /// Whether what it holds is still being measured.
    fn is_reading(&mut self) -> bool {
        self.long
            .as_mut()
            .is_some_and(|long| long.read.read_as_ref().is_none())
    }

    /// Its bytes, once read.
    fn bytes(&mut self) -> Option<Arc<[u8]>> {
        match &mut self.long {
            None => Some(Arc::from(self.text.as_bytes())),
            Some(long) => match long.read.read_as_ref() {
                Some(Ok(plain)) => Some(plain.text().shared()),
                _ => None,
            },
        }
    }

    /// Holds `text`, whichever way its length asks, in place of what it held.
    fn hold(&mut self, ctx: &egui::Context, text: String) {
        forget_field(ctx, self.field);
        if text.len() <= LONG {
            self.text = text;
            self.long = None;
            return;
        }
        self.text.clear();
        let mut read = Bind::new(true);
        read.request(measured(text));
        self.long = Some(Long {
            read,
            grid: TextGrid::new(("quick-diff-long", self.words)),
        });
    }

    /// How long it is, in words; past a field's room, that it can't be edited.
    fn length(&mut self) -> Option<String> {
        match &mut self.long {
            None if self.text.is_empty() => None,
            None => Some(lines(self.text.lines().count())),
            Some(long) => match long.read.read_as_ref() {
                Some(Ok(plain)) => {
                    let text = plain.text();
                    let bytes = u64::try_from(text.bytes().len()).unwrap_or(u64::MAX);
                    Some(format!(
                        "{} · {}: too long to edit here",
                        lines(usize::try_from(text.lines()).unwrap_or(usize::MAX)),
                        crate::format::size(bytes)
                    ))
                }
                _ => None,
            },
        }
    }
}

/// `count` lines, in words.
fn lines(count: usize) -> String {
    match count {
        1 => "1 line".to_owned(),
        count => {
            let count = u64::try_from(count).unwrap_or(u64::MAX);
            format!("{} lines", crate::format::count(count))
        }
    }
}

/// `text` measured for the grid (a job's: it may be long).
async fn measured(text: String) -> Result<Arc<Plain>, Problem> {
    crate::jobs::blocking(async move {
        let text = Text::new(text.into_bytes())
            .map_err(|_| Problem::new("That's too long to compare", "4 GiB is the most."))?;
        Ok(Arc::new(Plain::new(
            text,
            crate::preferences::text_options(),
        )))
    })
    .await
}

/// egui's memory of a field: its cursor, and a copy of its text for undo.
fn forget_field(ctx: &egui::Context, field: Id) {
    ctx.data_mut(|data| data.remove::<TextEditState>(field));
}

/// What was asked to be compared: the texts, and the name of a file saying its language.
#[derive(Clone)]
struct Asked {
    before: Arc<[u8]>,
    after: Arc<[u8]>,
    name: BString,
}

/// The Quick diff tab's state.
pub(crate) struct QuickDiffView {
    before: Side,
    after: Side,
    /// The name of a file like these, for its colours.
    name: String,
    /// The field to give the keys to, next it is drawn.
    focus: Option<Which>,
    /// What is being compared, while their differences show; the texts' page otherwise.
    comparing: Option<Asked>,
    whole: bool,
    ignore_whitespace: bool,
    split: bool,
    diff: Bind<Arc<DiffDocument>, Problem>,
    /// Stops the diff's git when another is asked for, or the tab closes.
    cancel: CancelToken,
    sides: Bind<Arc<SplitDiff>, Problem>,
    diff_grid: TextGrid,
    sides_grid: TextGrid,
}

impl QuickDiffView {
    /// Empty, the first field given the keys, its diffs showing first as preferred.
    fn new(ctx: &egui::Context) -> Self {
        let preferred = Preferences::of(ctx).files;
        let mut view = Self {
            before: Side::new("Before"),
            after: Side::new("After"),
            name: String::new(),
            focus: Some(Which::Before),
            comparing: None,
            whole: false,
            ignore_whitespace: false,
            split: preferred.split_diffs,
            diff: Bind::new(true),
            cancel: CancelToken::new(),
            sides: Bind::new(true),
            diff_grid: TextGrid::new("quick-diff-unified"),
            sides_grid: TextGrid::new("quick-diff-sides"),
        };
        view.diff_grid.set_wrap(preferred.wrap);
        view.sides_grid.set_wrap(preferred.wrap);
        view
    }

    const fn side(&self, which: Which) -> &Side {
        match which {
            Which::Before => &self.before,
            Which::After => &self.after,
        }
    }

    const fn side_mut(&mut self, which: Which) -> &mut Side {
        match which {
            Which::Before => &mut self.before,
            Which::After => &mut self.after,
        }
    }

    /// Whether the texts can be compared, and why not.
    fn ready(&mut self, git: Result<&Git, &'static str>) -> Result<(), &'static str> {
        git?;
        if self.before.is_reading() || self.after.is_reading() {
            return Err("Still reading what was pasted");
        }
        if self.before.is_empty() && self.after.is_empty() {
            return Err("Paste or type a text on either side first");
        }
        if self.before.bytes().is_none() || self.after.bytes().is_none() {
            return Err("One of them couldn't be read");
        }
        Ok(())
    }

    /// Shows what differs between the texts as they stand.
    fn compare(&mut self, git: &Git) {
        let (Some(before), Some(after)) = (self.before.bytes(), self.after.bytes()) else {
            return;
        };
        self.comparing = Some(Asked {
            before,
            after,
            name: BString::from(self.name.trim()),
        });
        // Nothing of the last comparison shows: it was of other texts.
        self.diff = Bind::new(true);
        self.sides = Bind::new(true);
        let wrap = self.diff_grid.wraps();
        self.diff_grid = TextGrid::new("quick-diff-unified");
        self.sides_grid = TextGrid::new("quick-diff-sides");
        self.diff_grid.set_wrap(wrap);
        self.sides_grid.set_wrap(wrap);
        self.read(git);
    }

    /// Reads the diff of what is being compared, as asked for now.
    fn read(&mut self, git: &Git) {
        let Some(asked) = self.comparing.clone() else {
            return;
        };
        self.cancel.cancel();
        self.cancel = CancelToken::new();
        let options = DiffOptions {
            context: if self.whole {
                WHOLE_FILE
            } else {
                DiffOptions::default().context
            },
            ignore_whitespace: self.ignore_whitespace,
            ..DiffOptions::default()
        };
        self.diff.request(read_text_diff(
            git.clone(),
            (asked.before, asked.after),
            asked.name,
            options,
            self.cancel.clone(),
        ));
    }

    /// The texts change places.
    fn swap(&mut self, ctx: &egui::Context) {
        std::mem::swap(&mut self.before.text, &mut self.after.text);
        std::mem::swap(&mut self.before.long, &mut self.after.long);
        // A field's undo would bring back the other's text.
        forget_field(ctx, self.before.field);
        forget_field(ctx, self.after.field);
    }
}

#[cfg(test)]
impl QuickDiffView {
    /// What the two fields hold.
    pub(crate) fn fields(&self) -> (&str, &str) {
        (&self.before.text, &self.after.text)
    }

    /// Whether each text is too long for a field.
    pub(crate) const fn long(&self) -> (bool, bool) {
        (self.before.long.is_some(), self.after.long.is_some())
    }

    /// Whether both texts have been read, however long.
    pub(crate) fn is_read(&mut self) -> bool {
        !self.before.is_reading() && !self.after.is_reading()
    }

    /// The lines the diff shows, once read and not being read again.
    pub(crate) fn diff_lines(&mut self) -> Option<u32> {
        if self.diff.is_pending() {
            return None;
        }
        self.diff.ok_ref().map(|diff| diff.measure().lines())
    }

    /// Whether the two sides are split and show, once a read has landed.
    pub(crate) fn sides_shown(&mut self) -> bool {
        self.sides.ok_ref().is_some()
    }

    /// Whether what differs between them shows, not the texts.
    pub(crate) const fn is_comparing(&self) -> bool {
        self.comparing.is_some()
    }
}

impl Drop for QuickDiffView {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// Draws the Quick diff tab.
pub(crate) fn show(ui: &mut Ui, state: &mut AppState) {
    let palette = Palette::of(ui.ctx());
    ui.painter()
        .rect_filled(ui.available_rect_before_wrap(), 0.0, palette.panel);
    let AppState {
        git, quick_diff, ..
    } = state;
    let git = match git.read_as_ref() {
        Some(Ok(git)) => Ok(git),
        Some(Err(_)) => Err("git isn't usable: Home says why"),
        None => Err("Looking for git…"),
    };
    let view = quick_diff.get_or_insert_with(|| Box::new(QuickDiffView::new(ui.ctx())));
    if view.comparing.is_some() {
        differences(ui, view, git);
    } else {
        texts(ui, view, git);
    }
}

/// Forgets the tab's texts, once it has closed.
pub(crate) fn closed(ctx: &egui::Context, state: &mut AppState) {
    if let Some(view) = state.quick_diff.take() {
        forget_field(ctx, view.before.field);
        forget_field(ctx, view.after.field);
    }
}

/// A paste, before the fields see this frame's events. One too long for a field never reaches
/// it, and replaces all its side holds; one with no field having the keys (any here, or another
/// page's) lands in the first side empty. Pasted into both empty, the keys then go to the other:
/// two texts are two pastes.
fn take_paste(ctx: &egui::Context, view: &mut QuickDiffView) {
    let some = ctx.input(|input| {
        input
            .events
            .iter()
            .any(|event| matches!(event, Event::Paste(text) if !text.is_empty()))
    });
    if !some {
        return;
    }
    let focused = ctx.memory(egui::Memory::focused);
    let has_keys = [Which::Before, Which::After]
        .into_iter()
        .find(|which| focused == Some(view.side(*which).field));
    // Another field's: the name's, or a dialog's.
    if has_keys.is_none() && ctx.text_edit_focused() {
        return;
    }
    let onto = has_keys.or_else(|| {
        [Which::Before, Which::After]
            .into_iter()
            .find(|which| view.side(*which).is_empty())
    });
    let Some(onto) = onto else {
        return;
    };
    let first = view.before.is_empty() && view.after.is_empty();
    let least = if has_keys.is_some() { LONG } else { 0 };
    let taken = ctx.input_mut(|input| {
        let at = input
            .events
            .iter()
            .position(|event| matches!(event, Event::Paste(text) if text.len() > least))?;
        match input.events.remove(at) {
            Event::Paste(text) => Some(text),
            _ => None,
        }
    });
    if let Some(text) = taken {
        view.side_mut(onto).hold(ctx, text);
    }
    if first {
        view.focus = Some(onto.other());
    }
}

/// The texts' page: a bar, and the two texts beside each other (on a narrow window, one above
/// the other).
fn texts(ui: &mut Ui, view: &mut QuickDiffView, git: Result<&Git, &'static str>) {
    let ctx = ui.ctx().clone();
    let keys_free = ctx.memory(|memory| memory.top_modal_layer().is_none());
    if keys_free {
        take_paste(&ctx, view);
    }
    // Taken before a field sees it: Enter alone ends a line there.
    let keyed =
        keys_free && ui.input_mut(|input| input.consume_key(Modifiers::COMMAND, Key::Enter));
    let ready = view.ready(git);
    let narrow = crate::narrow::is_narrow(&ctx);
    let mut compare = keyed;
    let mut swap = false;
    widgets::bar(ui, |ui| {
        ui.horizontal(|ui| {
            widgets::heading(ui, "Quick diff");
            ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                let keys = keys_text("⌘Enter");
                let button = widgets::primary(ui, ready.is_ok(), "Compare")
                    .on_hover_text(format!("Show what differs between them ({keys})"));
                compare |= match ready {
                    Ok(()) => button.clicked(),
                    Err(why) => {
                        button.on_disabled_hover_text(why);
                        false
                    }
                };
                if !narrow {
                    ui.label(muted(ui, keys.as_ref()).small());
                }
                let both_empty = view.before.is_empty() && view.after.is_empty();
                swap = widgets::secondary(
                    ui,
                    !both_empty,
                    &format!("{}  Swap", icon::ARROWS_LEFT_RIGHT),
                )
                .on_hover_text("The two texts change places")
                .clicked();
                if !narrow {
                    name_field(ui, &mut view.name);
                    ui.with_layout(Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.add(
                            Label::new(muted(ui, "Any two texts: no repository needed."))
                                .truncate(),
                        );
                    });
                }
            });
        });
        if narrow {
            ui.horizontal(|ui| {
                ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                    name_field(ui, &mut view.name);
                });
            });
        }
    });

    let room = ui.available_rect_before_wrap().shrink(GAP);
    let (first, second) = if narrow {
        let height = (room.height() - GAP) / 2.0;
        (
            Rect::from_min_size(room.min, vec2(room.width(), height)),
            Rect::from_min_max(egui::pos2(room.left(), room.top() + height + GAP), room.max),
        )
    } else {
        let width = (room.width() - GAP) / 2.0;
        (
            Rect::from_min_size(room.min, vec2(width, room.height())),
            Rect::from_min_max(egui::pos2(room.left() + width + GAP, room.top()), room.max),
        )
    };
    let focus = view.focus.take();
    if draw_side(ui, &mut view.before, first, focus == Some(Which::Before)) {
        view.focus = Some(Which::Before);
    }
    if draw_side(ui, &mut view.after, second, focus == Some(Which::After)) {
        view.focus = Some(Which::After);
    }
    if swap {
        view.swap(&ctx);
    }
    if compare && let Ok(git) = git {
        view.compare(git);
    }
}

/// What its language is told by: the name of a file like these.
fn name_field(ui: &mut Ui, name: &mut String) {
    let field = ui.add(
        widgets::line_field(name)
            .id_salt("quick-diff-name")
            .hint_text("main.rs")
            .desired_width(120.0),
    );
    let label = ui.label("File name");
    field
        .labelled_by(label.id)
        .on_hover_text("Optional: colours the differences as a file of this name would be");
}

/// One of the texts in `rect`: what it is called and how long it is, and its field (or, too long
/// for one, the grid); given the keys where `focus` asks. Returns whether it was cleared: its
/// field, maybe not drawn this frame, wants the keys next.
fn draw_side(ui: &mut Ui, side: &mut Side, rect: Rect, focus: bool) -> bool {
    let page = UiBuilder::new()
        .id(side.field.with("side"))
        .max_rect(rect)
        .layout(Layout::top_down(egui::Align::Min));
    let mut clear = false;
    ui.scope_builder(page, |ui| {
        let label = ui
            .horizontal(|ui| {
                ui.set_min_height(CONTROL);
                let label = widgets::heading(ui, side.words);
                let length = side.length();
                ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                    if !side.is_empty() {
                        let name = format!("Clear the text {}", side.words.to_lowercase());
                        clear = widgets::secondary_as(ui, true, ("Clear", &name)).clicked();
                    }
                    if let Some(length) = length {
                        ui.with_layout(Layout::left_to_right(egui::Align::Center), |ui| {
                            ui.add(Label::new(muted(ui, length)).truncate());
                        });
                    }
                });
                label
            })
            .inner;
        ui.add_space(4.0);
        let height = (rect.bottom() - ui.cursor().top()).max(CONTROL);
        if let Some(long) = &mut side.long {
            // Nothing that isn't drawn has the keys.
            ui.ctx()
                .memory_mut(|memory| memory.surrender_focus(side.field));
            match long.read.read_as_ref() {
                None => widgets::working(ui, "Reading what was pasted…"),
                Some(Err(problem)) => {
                    let problem = problem.clone();
                    widgets::problem_box(ui, &problem, |_| {});
                }
                Some(Ok(plain)) => {
                    let plain = Arc::clone(plain);
                    widgets::list_box(ui, |ui| long.grid.show(ui, &*plain));
                }
            }
            return;
        }
        let hint = if side.words == "Before" {
            "Paste or type the first text"
        } else {
            "Paste or type the text to compare it with"
        };
        let edit = widgets::text_area(&mut side.text)
            .id(side.field)
            .font(egui::TextStyle::Monospace)
            .hint_text(hint)
            .min_size(vec2(0.0, height));
        let field = widgets::bounded_text_area(ui, edit, height, side.words).labelled_by(label.id);
        if focus {
            field.request_focus();
        }
    });
    if clear {
        side.hold(ui.ctx(), String::new());
    } else if side.long.is_none() && side.text.len() > LONG {
        // Typed past a field's room, or pasted onto what it held.
        let text = std::mem::take(&mut side.text);
        side.hold(ui.ctx(), text);
    }
    clear
}

/// What differs between the texts: a bar going back to them and saying how to look, over the diff.
fn differences(ui: &mut Ui, view: &mut QuickDiffView, git: Result<&Git, &'static str>) {
    let ctx = ui.ctx().clone();
    // Escape closes a find bar or clears a selection first (the grid takes it), then goes back.
    let grid_takes_it = [&view.diff_grid, &view.sides_grid]
        .iter()
        .any(|grid| grid.has_selection() || grid.is_finding());
    let keys_free = ctx.memory(|memory| memory.top_modal_layer().is_none());
    let mut back = !grid_takes_it
        && keys_free
        && ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape));
    let narrow = crate::narrow::is_narrow(&ctx);
    let palette = Palette::of(&ctx);
    let reading = view.diff.is_pending();
    let mut read_again = false;
    let changed = match view.diff.read_as_ref() {
        Some(Ok(diff)) if diff.changed() != (0, 0) => Some(diff.changed()),
        _ => None,
    };
    let name = view
        .comparing
        .as_ref()
        .map_or_else(String::new, |asked| asked.name.to_string());
    widgets::bar(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            back |= widgets::secondary(ui, true, &format!("{}  Edit the texts", icon::ARROW_LEFT))
                .clicked();
            if !name.is_empty() {
                widgets::row_label(ui, &name);
            }
            if let Some((added, removed)) = changed {
                ui.label(RichText::new(format!("+{added}")).color(palette.ok));
                ui.label(RichText::new(format!("−{removed}")).color(palette.danger));
            }
            if !narrow {
                widgets::segmented(
                    ui,
                    &mut view.split,
                    &[(false, "Unified"), (true, "Side by side")],
                );
            }
            read_again |= widgets::checkbox(ui, &mut view.whole, "Whole file").changed();
            read_again |= widgets::checkbox(ui, &mut view.ignore_whitespace, "Ignore whitespace")
                .on_hover_text("Lines that differ only in spaces and tabs count as the same")
                .changed();
            let mut wrap = view.diff_grid.wraps();
            if widgets::checkbox(ui, &mut wrap, "Wrap lines").changed() {
                view.diff_grid.set_wrap(wrap);
                view.sides_grid.set_wrap(wrap);
            }
            // Its room always kept: the row doesn't wrap as a read comes and goes.
            let (place, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
            if reading {
                ui.put(place, egui::Spinner::new().size(14.0));
            }
        });
    });
    if read_again && let Ok(git) = git {
        view.diff_grid.clear_selection();
        view.read(git);
    }
    match view.diff.read_as_ref() {
        None => {
            placed(ui, |ui| widgets::working(ui, "Comparing…"));
        }
        Some(Err(problem)) => {
            let problem = problem.clone();
            placed(ui, |ui| widgets::problem_box(ui, &problem, |_| {}));
        }
        Some(Ok(diff)) if diff.changed() == (0, 0) => {
            let said = if view.ignore_whitespace {
                "Nothing but whitespace differs (Ignore whitespace is on)."
            } else {
                "The two texts are the same."
            };
            placeholder(ui, said.to_owned());
        }
        // A narrow window (a phone's) has no room for two sides.
        Some(Ok(diff)) if view.split && !narrow => {
            let diff = Arc::clone(diff);
            show_sides(ui, diff, reading, &mut view.sides, &mut view.sides_grid);
        }
        Some(Ok(diff)) => {
            let diff = Arc::clone(diff);
            view.diff_grid.show(ui, &*diff);
        }
    }
    if back {
        view.cancel.cancel();
        view.comparing = None;
    }
}

/// `add` set in from the page's edges, as a file view's messages are.
fn placed(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .inner_margin(egui::Margin::same(16))
        .show(ui, add);
}
