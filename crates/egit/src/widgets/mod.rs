//! The pieces views are built from, so every view looks and answers alike: text in a few sizes,
//! buttons of a few kinds (primary, secondary, danger, and a toolbar's quiet ones), toggles
//! (`toggles`), cards, facts in aligned columns, sections with their actions, labels for where
//! a setting comes from, and problem boxes.

mod toggles;

use std::path::Path;

use egit_git::{ConfigEntry, ConfigScope};
use egui::{
    Button, Color32, CornerRadius, Event, Frame, Key, Margin, Response, RichText, Stroke,
    StrokeKind, TextBuffer, TextEdit, Ui, WidgetText,
};

pub(crate) use toggles::{checkbox, radio, radio_value, segmented};

use crate::format::display_path;
use crate::problem::Problem;
use crate::theme::{CONTROL, HEADING, Palette};

/// The smallest a control is drawn each way, in points: big enough to hit (WCAG 2.5.8 asks
/// for 24).
pub(crate) const TARGET: f32 = 24.0;

/// A one-line field's margin, which makes it as tall as a button.
const FIELD_MARGIN: Margin = Margin::symmetric(8, 6);

/// A page's title (Home's, the profile's, a repository's overview).
const PAGE_TITLE: f32 = 22.0;

/// A panel's or a dialog's title.
pub(crate) const PANEL_TITLE: f32 = 16.0;

/// The width of a fact's label column: facts in different cards line up.
pub(crate) const FACT_LABEL: f32 = 112.0;

/// A raised panel grouping related content.
pub(crate) fn card<R>(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
    let palette = Palette::of(ui.ctx());
    Frame::new()
        .fill(palette.surface)
        .stroke(Stroke::new(1.0, palette.border))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add_contents(ui)
        })
        .inner
}

/// A card's or a section's title.
pub(crate) fn heading(ui: &mut Ui, text: &str) -> Response {
    ui.label(RichText::new(text).size(HEADING).strong())
}

/// A page's title.
pub(crate) fn page_title(ui: &mut Ui, text: &str) -> Response {
    ui.label(RichText::new(text).size(PAGE_TITLE).strong())
}

/// A panel's or a dialog's title.
pub(crate) fn panel_title(ui: &mut Ui, text: impl Into<String>) -> Response {
    ui.add(egui::Label::new(RichText::new(text).size(PANEL_TITLE).strong()).wrap())
}

/// A section's header: its title (and how many it holds, muted), then `add_right` from the
/// right edge (its buttons), all on one line a control's height tall.
pub(crate) fn section_header(
    ui: &mut Ui,
    title: &str,
    count: Option<usize>,
    add_right: impl FnOnce(&mut Ui),
) {
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), CONTROL),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.label(RichText::new(title).size(HEADING).strong());
            if let Some(count) = count {
                ui.label(muted(ui, count.to_string()));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), add_right);
        },
    );
}

/// A fact: its label, muted, in a column `width` wide (so facts line up, in one card or
/// several: [`FACT_LABEL`] on a page), then `add_value`, centred on one line with it.
pub(crate) fn fact<R>(
    ui: &mut Ui,
    width: f32,
    label: &str,
    add_value: impl FnOnce(&mut Ui) -> R,
) -> R {
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), CONTROL),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            fact_label(ui, width, label, egui::Align::Center);
            add_value(ui)
        },
    )
    .inner
}

/// A fact whose value may take several lines: the label beside the value's first line.
pub(crate) fn fact_top<R>(
    ui: &mut Ui,
    width: f32,
    label: &str,
    add_value: impl FnOnce(&mut Ui) -> R,
) -> R {
    ui.horizontal_top(|ui| {
        fact_label(ui, width, label, egui::Align::Min);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            add_value(ui)
        })
        .inner
    })
    .inner
}

/// A form's row: its label in a column `width` wide, centred on the field `add_field` adds
/// (filling the rest), which the label names.
pub(crate) fn form_row(
    ui: &mut Ui,
    width: f32,
    label: &str,
    add_field: impl FnOnce(&mut Ui) -> Response,
) -> Response {
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), CONTROL),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            let label = fact_label(ui, width, label, egui::Align::Center);
            add_field(ui).labelled_by(label.id)
        },
    )
    .inner
}

/// A fact's label in its column.
fn fact_label(ui: &mut Ui, width: f32, label: &str, align: egui::Align) -> Response {
    let text = muted(ui, label);
    // A control's height when centred on a row (so it centres the same as the value), none
    // beside a value's first line.
    let height = if align == egui::Align::Center {
        CONTROL
    } else {
        0.0
    };
    ui.allocate_ui_with_layout(
        egui::vec2(width, height),
        egui::Layout::left_to_right(align),
        |ui| {
            ui.set_min_width(width);
            ui.add(egui::Label::new(text).truncate().selectable(false))
        },
    )
    .inner
}

/// Secondary text.
pub(crate) fn muted(ui: &Ui, text: impl Into<String>) -> RichText {
    RichText::new(text).color(Palette::of(ui.ctx()).muted)
}

/// The button for the main thing to do: filled with the accent; while it can't be pressed, a
/// plain one (a faded accent reads as pressable).
pub(crate) fn primary(ui: &mut Ui, enabled: bool, text: &str) -> Response {
    let palette = Palette::of(ui.ctx());
    filled(ui, enabled, text, palette.accent, palette.on_accent)
}

/// A button for what can't be undone easily (deleting, a hard reset, a force push): filled
/// with the danger colour.
pub(crate) fn danger(ui: &mut Ui, enabled: bool, text: &str) -> Response {
    let palette = Palette::of(ui.ctx());
    filled(ui, enabled, text, palette.danger, palette.on_danger)
}

/// A button filled with `fill` (a shade nearer the text's colour under the pointer), its text
/// `on`; a plain one while disabled.
fn filled(ui: &mut Ui, enabled: bool, text: &str, fill: Color32, on: Color32) -> Response {
    if !enabled {
        return ui.add_enabled(
            false,
            Button::new(text).wrap_mode(egui::TextWrapMode::Extend),
        );
    }
    // Whether the pointer was on it last frame: a button's fill is chosen before it is drawn.
    let hovered = ui
        .ctx()
        .read_response(ui.next_auto_id())
        .is_some_and(|response| response.hovered());
    let fill = if hovered {
        fill.lerp_to_gamma(Palette::of(ui.ctx()).text, 0.18)
    } else {
        fill
    };
    ui.add(
        Button::new(RichText::new(text).color(on).strong())
            .fill(fill)
            .stroke(Stroke::NONE)
            .wrap_mode(egui::TextWrapMode::Extend),
    )
}

/// Any other button. Its words never wrap: a row too narrow wraps whole buttons instead.
pub(crate) fn secondary(ui: &mut Ui, enabled: bool, text: &str) -> Response {
    ui.add_enabled(
        enabled,
        Button::new(text).wrap_mode(egui::TextWrapMode::Extend),
    )
}

/// How a toolbar's button shows: its icon and words, or (where room is short) its icon alone,
/// its words on hover. Named by its words either way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Look {
    Full,
    Compact,
}

impl Look {
    /// The words shown beside `icon`, or none.
    fn label(self, icon: &str, text: &str) -> String {
        match self {
            Self::Full => format!("{icon}  {text}"),
            Self::Compact => icon.to_owned(),
        }
    }
}

/// A toolbar's button: quiet (no frame until the pointer is on it), tinted while `selected`
/// (a view it shows is open); named `text`, which a compact one shows on hover.
pub(crate) fn tool(ui: &mut Ui, icon: &str, text: &str, selected: bool, look: Look) -> Response {
    let width = if look == Look::Compact { CONTROL } else { 0.0 };
    let response = ui.add(
        Button::selectable(selected, look.label(icon, text)).min_size(egui::vec2(width, CONTROL)),
    );
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Button, response.enabled(), selected, text)
    });
    if look == Look::Compact {
        response.on_hover_text(text)
    } else {
        response
    }
}

/// A toolbar's menu: a quiet button with a caret, opening `add_contents` below it; named
/// `text` (the caret only shows it opens a menu).
pub(crate) fn tool_menu<R>(
    ui: &mut Ui,
    icon: &str,
    text: &str,
    look: Look,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> (Response, Option<R>) {
    let button = Button::new(format!(
        "{}  {}",
        look.label(icon, text),
        crate::theme::icon::CARET_DOWN
    ))
    .frame_when_inactive(false)
    .min_size(egui::vec2(0.0, CONTROL));
    let (response, inner) =
        egui::containers::menu::MenuButton::from_button(button).ui(ui, add_contents);
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, response.enabled(), text)
    });
    let response = if look == Look::Compact {
        response.on_hover_text(text)
    } else {
        response
    };
    (response, inner.map(|inner| inner.inner))
}

/// A toolbar's button of one icon (a whole control square), named `name` in words for those
/// who don't see the icon.
pub(crate) fn icon_tool(ui: &mut Ui, enabled: bool, glyph: &str, name: &str) -> Response {
    let response = ui.add_enabled(
        enabled,
        Button::new(glyph)
            .frame_when_inactive(false)
            .min_size(egui::vec2(CONTROL, CONTROL)),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, name));
    response
}

/// A button that reads as a link (a commit's name to go to): no frame until the pointer is on
/// it, and a slim inset, so it lines up with text beside or above it.
pub(crate) fn link(ui: &mut Ui, text: impl Into<WidgetText>) -> Response {
    ui.scope(|ui| {
        ui.spacing_mut().button_padding = egui::vec2(4.0, 2.0);
        ui.add(
            Button::new(text)
                .frame_when_inactive(false)
                .min_size(egui::vec2(0.0, TARGET)),
        )
    })
    .inner
}

/// A thin upright line between a toolbar's groups.
pub(crate) fn toolbar_separator(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(9.0, CONTROL), egui::Sense::hover());
    let color = Palette::of(ui.ctx()).border;
    ui.painter().vline(
        rect.center().x,
        egui::Rangef::new(rect.top() + 6.0, rect.bottom() - 6.0),
        Stroke::new(1.0, color),
    );
}

/// A compact button: small text, yet a whole target to hit.
pub(crate) fn small<'a>(text: impl Into<WidgetText>) -> Button<'a> {
    Button::new(text)
        .small()
        .min_size(egui::vec2(TARGET, TARGET))
}

/// A button of one icon filling exactly `rect` (so never wider than its place): the glyph, a
/// background while the pointer is on it or presses it, and `name`, its words for those who
/// don't see the icon.
pub(crate) fn icon_button(
    ui: &Ui,
    rect: egui::Rect,
    id: egui::Id,
    glyph: &str,
    name: &str,
    enabled: bool,
) -> Response {
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let response = ui.interact(rect, id, sense);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, name));
    let visuals = ui.style().interact(&response);
    if enabled && (response.hovered() || response.is_pointer_button_down_on()) {
        ui.painter()
            .rect_filled(rect, visuals.corner_radius, visuals.weak_bg_fill);
    }
    let color = if enabled {
        visuals.text_color()
    } else {
        ui.visuals().weak_text_color()
    };
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        glyph,
        egui::TextStyle::Button.resolve(ui.style()),
        color,
    );
    response
}

/// Paints a list row's background: the selection's, a step lighter (or darker) while the
/// pointer rests on it; or a hovered row's.
pub(crate) fn row_fill(ui: &Ui, rect: egui::Rect, rounding: f32, selected: bool, hovered: bool) {
    let palette = Palette::of(ui.ctx());
    let painter = ui.painter();
    if selected {
        painter.rect_filled(rect, rounding, palette.accent.gamma_multiply(0.22));
        if hovered {
            painter.rect_filled(rect, rounding, palette.selected_hover);
        }
    } else if hovered {
        painter.rect_filled(rect, rounding, palette.surface_alt);
    }
}

/// A one-line text field, as tall as a button.
pub(crate) fn line_field(text: &mut dyn TextBuffer) -> TextEdit<'_> {
    TextEdit::singleline(text).margin(FIELD_MARGIN)
}

/// Shows which control has the keyboard focus, as a browser's `:focus-visible` does: a ring
/// around it once a key (Tab, an arrow) or an assistive technology moved the focus, and none
/// from the next press of the pointer. Called once a frame, after everything is drawn.
pub(crate) fn focus_ring(ctx: &egui::Context) {
    let id = egui::Id::new("egit-focus-visible");
    // The frame's events as they came: views consume the arrows they act on.
    let moved = ctx.input(|input| {
        input.raw.events.iter().fold(None, |shown, event| match event {
            Event::PointerButton { pressed: true, .. } => Some(false),
            Event::Key {
                key: Key::Tab | Key::ArrowUp | Key::ArrowDown | Key::ArrowLeft | Key::ArrowRight,
                pressed: true,
                ..
            } => Some(true),
            Event::AccessKitActionRequest(request)
                if request.action == egui::accesskit::Action::Focus =>
            {
                Some(true)
            }
            _ => shown,
        })
    });
    let visible = moved.map_or_else(
        || ctx.data(|data| data.get_temp(id)).unwrap_or(false),
        |visible| {
            ctx.data_mut(|data| data.insert_temp(id, visible));
            visible
        },
    );
    if !visible {
        return;
    }
    // A text field shows the focus itself (its accent edge).
    if ctx.text_edit_focused() {
        return;
    }
    let Some(focused) = ctx
        .memory(egui::Memory::focused)
        .and_then(|focused| ctx.read_response(focused))
    else {
        return;
    };
    // An area taking keys for what it holds (the commit table, a text grid) shows where they
    // act by its own selection: a ring around all of it would only weigh on the screen.
    if focused.rect.height() > 3.0 * CONTROL {
        return;
    }
    // Inside the control, and only where it shows: never over a neighbour.
    ctx.layer_painter(focused.layer_id)
        .with_clip_rect(focused.interact_rect)
        .rect_stroke(
            focused.rect,
            CornerRadius::same(4),
            Stroke::new(2.0, Palette::of(ctx).accent),
            StrokeKind::Inside,
        );
}

/// A tab's answer to the pointer resting on it (`egui_tiles`' tabs have none): a line under it.
pub(crate) fn tab_hover(response: &Response) {
    if response.hovered() {
        let rect = response.rect.shrink(1.0);
        response
            .ctx
            .layer_painter(response.layer_id)
            .with_clip_rect(response.rect)
            .hline(
                rect.x_range(),
                rect.bottom() - 1.0,
                Stroke::new(2.0, Palette::of(&response.ctx).muted),
            );
    }
}

/// Where a setting comes from: its scope and file, or git's default when nothing sets it.
pub(crate) fn origin(ui: &mut Ui, entry: Option<ConfigEntry<'_>>, home: Option<&Path>) {
    let text = origin_text(entry, home);
    let response = ui.label(muted(ui, text).small());
    if let Some(file) = entry.and_then(ConfigEntry::file) {
        response.on_hover_text(file.display().to_string());
    }
}

/// The words [`origin`] shows.
pub(crate) fn origin_text(entry: Option<ConfigEntry<'_>>, home: Option<&Path>) -> String {
    let Some(entry) = entry else {
        return "git's default".to_owned();
    };
    let scope = scope_name(entry.scope);
    entry.file().map_or_else(
        || format!("{scope} · {}", entry.origin),
        |file| format!("{scope} · {}", display_path(&file, home)),
    )
}

/// A scope as the user reads it.
pub(crate) const fn scope_name(scope: ConfigScope) -> &'static str {
    match scope {
        ConfigScope::System => "system",
        ConfigScope::Global => "global",
        ConfigScope::Local => "repository",
        ConfigScope::Worktree => "work tree",
        ConfigScope::Command => "command",
        ConfigScope::Unknown => "elsewhere",
    }
}

/// A problem, with git's words below the sentence; `add_actions` adds buttons beside them.
pub(crate) fn problem_box(ui: &mut Ui, problem: &Problem, add_actions: impl FnOnce(&mut Ui)) {
    let palette = Palette::of(ui.ctx());
    Frame::new()
        .fill(palette.danger.gamma_multiply(0.08))
        .stroke(Stroke::new(1.0, palette.danger.gamma_multiply(0.6)))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new(format!(
                    "{}  {}",
                    crate::theme::icon::WARNING_CIRCLE,
                    problem.title
                ))
                .color(palette.danger)
                .strong(),
            );
            if !problem.detail.is_empty() {
                ui.add(egui::Label::new(problem.detail.as_str()).selectable(true));
            }
            ui.horizontal(add_actions);
        });
}

/// A dialog's row of buttons, laid out from the right (the first added is the rightmost), as
/// tall as its buttons.
pub(crate) fn buttons_row(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), 0.0),
        egui::Layout::right_to_left(egui::Align::Center),
        add,
    );
}

/// A note that something is happening in the background.
pub(crate) fn working(ui: &mut Ui, text: &str) {
    ui.horizontal(|ui| {
        ui.spinner();
        ui.label(muted(ui, text));
    });
}
