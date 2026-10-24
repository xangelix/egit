//! The pieces views are built from, so every view looks and answers alike: text in a few sizes,
//! buttons of a few kinds (primary, secondary, danger, and a toolbar's quiet ones), toggles
//! (`toggles`), people's initials (`avatar`), cards, facts in aligned columns, sections with
//! their actions, labels for where a setting comes from, and problem boxes.

mod avatar;
mod toggles;

use std::path::Path;
use std::sync::Arc;

use egit_git::{ConfigEntry, ConfigScope};
use egui::{
    Button, Color32, CornerRadius, Event, Frame, Key, Margin, Response, RichText, Sense, Stroke,
    StrokeKind, TextBuffer, TextEdit, Ui, WidgetText,
};

pub(crate) use avatar::{avatar, avatars};
pub(crate) use toggles::{checkbox, checkbox_of, radio, radio_of, radio_value, segmented};

use crate::format::display_path;
use crate::problem::Problem;
use crate::theme::Weighted as _;
use crate::theme::{CONTROL, HEADING, Palette, SMALL, Weight};

/// The smallest a control is drawn each way, in points: big enough to hit (WCAG 2.5.8 asks
/// for 24).
pub(crate) const TARGET: f32 = 24.0;

/// A one-line field's margin, which makes it as tall as a button.
/// Between a field's edge and its text: the same on every side of a text area, at the sides
/// of a one-line field (whose text is centred in a control's height).
const FIELD_INSET: i8 = 8;

/// A page's title (Home's, the profile's, a repository's overview).
const PAGE_TITLE: f32 = 22.0;

/// A panel's or a dialog's title.
pub(crate) const PANEL_TITLE: f32 = 15.0;

/// The width of a fact's label column: facts in different cards line up.
pub(crate) const FACT_LABEL: f32 = 112.0;

/// A raised panel grouping related content.
pub(crate) fn card<R>(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
    let palette = Palette::of(ui.ctx());
    Frame::new()
        .fill(palette.surface)
        .stroke(Stroke::new(1.0, palette.border))
        .corner_radius(CornerRadius::same(crate::theme::RADIUS_CARD))
        .inner_margin(Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add_contents(ui)
        })
        .inner
}

/// A card's or a section's title.
pub(crate) fn heading(ui: &mut Ui, text: &str) -> Response {
    let response = ui.label(RichText::new(text).size(HEADING).semibold());
    crate::help::note(ui, response.rect, text);
    response
}

/// How long something has run, "12 s", in a place of its own width: ticking, it moves nothing
/// beside it.
pub(crate) fn seconds(ui: &mut Ui, elapsed: std::time::Duration, color: Color32) -> Response {
    let said = format!("{} s", elapsed.as_secs());
    ui.add_sized(
        [44.0, ui.text_style_height(&egui::TextStyle::Body)],
        egui::Label::new(RichText::new(said).color(color)).selectable(false),
    )
}

/// A heading naming something of the user's (a path, a name), explained by the help topic
/// `topic` rather than by its words.
pub(crate) fn heading_of(ui: &mut Ui, text: &str, topic: &'static str) -> Response {
    let response = ui.label(RichText::new(text).size(HEADING).semibold());
    crate::help::note_topic(ui, response.rect, topic);
    response
}

/// A page's title.
pub(crate) fn page_title(ui: &mut Ui, text: &str) -> Response {
    ui.label(RichText::new(text).size(PAGE_TITLE).semibold())
}

/// The height of a panel's first row (the sidebar's filter, the commit table's column titles,
/// the details' title): one band across the window, so the panels' first rows line up.
pub(crate) const HEAD: f32 = 40.0;

/// What a panel's first row holds, by its first line's height: a control (a field), a title, or
/// text.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Head {
    Control,
    Title,
    Text,
}

/// A panel's first row: [`HEAD`] tall, `add_contents` laid out from the left, its first line
/// centred on the band; what wraps grows downwards, its first line staying where it is, as far
/// below the panel's top as a single line's.
pub(crate) fn panel_head<R>(
    ui: &mut Ui,
    holds: Head,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> R {
    let line = match holds {
        Head::Control => CONTROL,
        Head::Title => ui.fonts_mut(|fonts| {
            fonts.row_height(&egui::FontId::new(PANEL_TITLE, Weight::SemiBold.family()))
        }),
        Head::Text => ui.text_style_height(&egui::TextStyle::Body),
    };
    let inset = ((HEAD - line) / 2.0).max(0.0);
    // Beside a control, a label or a smaller button centres on it; a title's or text's line
    // is what it holds, and what wraps grows downwards from its top.
    let align = match holds {
        Head::Control => egui::Align::Center,
        Head::Title | Head::Text => egui::Align::Min,
    };
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.add_space(inset);
        let inner = ui
            .allocate_ui_with_layout(
                egui::vec2(ui.available_width(), line),
                egui::Layout::left_to_right(align),
                |ui| {
                    ui.set_min_height(line);
                    add_contents(ui)
                },
            )
            .inner;
        ui.add_space(inset);
        inner
    })
    .inner
}

/// A panel's or a dialog's title.
pub(crate) fn panel_title(ui: &mut Ui, text: impl Into<String>) -> Response {
    ui.add(egui::Label::new(RichText::new(text).size(PANEL_TITLE).semibold()).wrap())
}

/// A section's header: its title (see [`section_title`]), then `add_right` from the right
/// edge (its buttons), all on one line a control's height tall.
pub(crate) fn section_header(
    ui: &mut Ui,
    (title, count, name): (&str, Option<usize>, &str),
    add_right: impl FnOnce(&mut Ui),
) {
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), CONTROL),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            section_title(ui, title, count, name);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), add_right);
        },
    );
}

/// A section's title, as every list's is (the sidebar's sections, a commit's files, the
/// changes to stage): small capitals, muted, then how many it holds in a pill. Named `name`
/// for those who don't see it: the title as written, or a sentence with the count.
pub(crate) fn section_title(
    ui: &mut Ui,
    title: &str,
    count: Option<usize>,
    name: &str,
) -> Response {
    let palette = Palette::of(ui.ctx());
    let words = title;
    let (title, count) = section_galleys(ui.painter(), title, count, &palette);
    let width = section_width(&title, count.as_ref());
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, title.size().y.max(PILL)), Sense::hover());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, name));
    crate::help::note(ui, rect, words);
    let pill = palette.surface_alt;
    paint_section(
        ui.painter(),
        rect.left_center(),
        (title, count),
        (&palette, pill),
    );
    response
}

/// A count's pill's height.
const PILL: f32 = 18.0;

/// A section title's and its count's text, laid out.
pub(crate) fn section_galleys(
    painter: &egui::Painter,
    title: &str,
    count: Option<usize>,
    palette: &Palette,
) -> (Arc<egui::Galley>, Option<Arc<egui::Galley>>) {
    let mut job = egui::text::LayoutJob::default();
    job.append(
        &title.to_uppercase(),
        0.0,
        egui::TextFormat {
            font_id: egui::FontId::new(SMALL, Weight::SemiBold.family()),
            color: palette.muted,
            extra_letter_spacing: 0.6,
            ..egui::TextFormat::default()
        },
    );
    let title = painter.layout_job(job);
    let count = count.map(|count| {
        painter.layout_no_wrap(
            crate::format::count(u64::try_from(count).unwrap_or(u64::MAX)),
            egui::FontId::new(SMALL, Weight::Medium.family()),
            palette.muted,
        )
    });
    (title, count)
}

/// How wide a section's title and count are.
fn section_width(title: &egui::Galley, count: Option<&Arc<egui::Galley>>) -> f32 {
    title.size().x + count.map_or(0.0, |count| 8.0 + count.size().x + 12.0)
}

/// Paints a section's title and count (on a `pill` of that colour) from `at` (their left,
/// centred on its height); returns how wide they are.
pub(crate) fn paint_section(
    painter: &egui::Painter,
    at: egui::Pos2,
    (title, count): (Arc<egui::Galley>, Option<Arc<egui::Galley>>),
    (palette, pill_fill): (&Palette, Color32),
) -> f32 {
    let width = section_width(&title, count.as_ref());
    let title_width = title.size().x;
    painter.galley(
        egui::pos2(at.x, at.y - title.size().y / 2.0),
        title,
        palette.muted,
    );
    if let Some(count) = count {
        let pill = egui::Rect::from_min_size(
            egui::pos2(at.x + title_width + 8.0, at.y - PILL / 2.0),
            egui::vec2(count.size().x + 12.0, PILL),
        );
        painter.rect_filled(pill, PILL / 2.0, pill_fill);
        let at = egui::Align2::CENTER_CENTER
            .align_size_within_rect(count.size(), pill)
            .min;
        painter.galley(at, count, palette.muted);
    }
    width
}

/// Between a bar's edges and its controls: 12 points at its sides, 6 above and below.
const BAR_MARGIN: Margin = Margin::symmetric(12, 6);

/// A bar over a view (its back button, what it shows, its controls): the chrome's tone, its
/// controls [`BAR_MARGIN`] from its edges, a hairline under it. Nothing is spaced from it:
/// bars stacked one under another make one strip, and what follows meets the hairline.
pub(crate) fn bar<R>(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
    let palette = Palette::of(ui.ctx());
    let spacing = ui.spacing().item_spacing.y;
    ui.spacing_mut().item_spacing.y = 0.0;
    let shown = Frame::new()
        .fill(palette.panel)
        .inner_margin(BAR_MARGIN)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = spacing;
            add_contents(ui)
        });
    let rect = shown.response.rect;
    ui.painter().hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(1.0, palette.border),
    );
    shown.inner
}

/// A box around a list (a commit's files): its rows on the window's background inside a
/// border, filling the rest of the panel.
pub(crate) fn list_box<R>(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
    let palette = Palette::of(ui.ctx());
    Frame::new()
        .fill(palette.background)
        .stroke(Stroke::new(1.0, palette.border))
        .corner_radius(CornerRadius::same(crate::theme::RADIUS_BOX))
        .inner_margin(Margin::same(4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add_contents(ui)
        })
        .inner
}

/// A box holding a long text apart from what is around it (a commit's description): the
/// window's background, a border, rounded corners.
pub(crate) fn text_box<R>(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
    let palette = Palette::of(ui.ctx());
    Frame::new()
        .fill(palette.background)
        .stroke(Stroke::new(1.0, palette.border))
        .corner_radius(CornerRadius::same(crate::theme::RADIUS_BOX))
        .inner_margin(Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add_contents(ui)
        })
        .inner
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

/// A [`fact`] whose label is explained by the help topic `topic` rather than by its word
/// (the details' "Commit" is a commit's name, not committing).
pub(crate) fn fact_of<R>(
    ui: &mut Ui,
    width: f32,
    (label, topic): (&str, &'static str),
    add_value: impl FnOnce(&mut Ui) -> R,
) -> R {
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), CONTROL),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            let shown = fact_label(ui, width, label, egui::Align::Center);
            crate::help::note_topic(ui, shown.rect, topic);
            add_value(ui)
        },
    )
    .inner
}

/// A fact whose value may take several lines: the label beside the value's first line, both
/// as far below the row's top (and the last line as far above its bottom) as a one-line
/// [`fact`]'s text, so facts keep one rhythm.
pub(crate) fn fact_top<R>(
    ui: &mut Ui,
    width: f32,
    label: &str,
    add_value: impl FnOnce(&mut Ui) -> R,
) -> R {
    let line = ui.text_style_height(&egui::TextStyle::Body);
    let inset = ((CONTROL - line) / 2.0).max(0.0);
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.add_space(inset);
        let inner = ui
            .horizontal_top(|ui| {
                fact_label(ui, width, label, egui::Align::Min);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    add_value(ui)
                })
                .inner
            })
            .inner;
        ui.add_space(inset);
        inner
    })
    .inner
}

/// A fact whose value may take several lines ([`fact_top`]), which help topic `topic`
/// explains.
pub(crate) fn fact_top_of<R>(
    ui: &mut Ui,
    width: f32,
    (label, topic): (&str, &'static str),
    add_value: impl FnOnce(&mut Ui) -> R,
) -> R {
    let shown = ui.scope(|ui| fact_top(ui, width, label, add_value));
    crate::help::note_topic(ui, shown.response.rect, topic);
    shown.inner
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
            let response = ui.add(egui::Label::new(text).truncate().selectable(false));
            crate::help::note(ui, response.rect, label);
            response
        },
    )
    .inner
}

/// Secondary text.
pub(crate) fn muted(ui: &Ui, text: impl Into<String>) -> RichText {
    RichText::new(text).color(Palette::of(ui.ctx()).muted)
}

/// What a banner or a notice says, by its colour: something done, something to know, a warning,
/// a problem.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tone {
    Done,
    Info,
    Warn,
    Danger,
}

impl Tone {
    /// Its colour on `palette`.
    pub(crate) const fn color(self, palette: Palette) -> Color32 {
        match self {
            Self::Done => palette.ok,
            Self::Info => palette.accent,
            Self::Warn => palette.warn,
            Self::Danger => palette.danger,
        }
    }

    /// Its icon.
    pub(crate) const fn icon(self) -> &'static str {
        match self {
            Self::Done => crate::theme::icon::CHECK_CIRCLE,
            Self::Info => crate::theme::icon::INFO,
            Self::Warn => crate::theme::icon::WARNING,
            Self::Danger => crate::theme::icon::WARNING_CIRCLE,
        }
    }
}

/// A banner: what stands until it is dealt with, or what just happened, in a box tinted by its
/// tone, every banner alike (its edge, corners, and room inside).
pub(crate) fn banner<R>(ui: &mut Ui, tone: Tone, add: impl FnOnce(&mut Ui) -> R) -> R {
    let color = tone.color(Palette::of(ui.ctx()));
    Frame::new()
        .fill(color.gamma_multiply(0.10))
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.6)))
        .corner_radius(CornerRadius::same(crate::theme::RADIUS_BOX))
        .inner_margin(Margin::symmetric(12, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// A notice of a line: its tone's icon and its words (wrapping in what is left), then at its
/// right what `add_buttons` adds (laid from the right) and, where it can go, an × dismissing it.
/// Whether it was dismissed.
pub(crate) fn notice(
    ui: &mut Ui,
    tone: Tone,
    words: impl Into<WidgetText>,
    dismissable: bool,
    add_buttons: impl FnOnce(&mut Ui),
) -> bool {
    let palette = Palette::of(ui.ctx());
    let words = words.into();
    banner(ui, tone, |ui| {
        let mut dismissed = false;
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if dismissable {
                    dismissed = icon_tool(ui, true, crate::theme::icon::X, "Dismiss")
                        .on_hover_text("Dismiss")
                        .clicked();
                }
                add_buttons(ui);
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.label(RichText::new(tone.icon()).color(tone.color(palette)));
                    ui.add(egui::Label::new(words).wrap());
                });
            });
        });
        dismissed
    })
}

/// A chip: a few small words in a thin outline of their colour ("may fail", "Draft", a label),
/// 18 points tall however tall its row, so chips read as marks rather than buttons.
pub(crate) fn chip(ui: &mut Ui, text: &str, color: Color32) -> Response {
    const HEIGHT: f32 = 18.0;
    let galley =
        ui.painter()
            .layout_no_wrap(text.to_owned(), egui::FontId::proportional(SMALL), color);
    let size = egui::vec2(galley.size().x + 12.0, HEIGHT);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(crate::theme::RADIUS_INNER),
        Stroke::new(1.0, color.gamma_multiply(0.7)),
        StrokeKind::Inside,
    );
    let at = egui::pos2(
        rect.center().x - galley.size().x / 2.0,
        rect.center().y - galley.size().y / 2.0,
    );
    ui.painter().galley(at, galley, color);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, text));
    response
}

/// How wide `text` is in the body's font.
pub(crate) fn text_width(ui: &Ui, text: &str) -> f32 {
    text_width_as(ui, text, &egui::TextStyle::Body)
}

/// How wide `text` is in `style`'s font.
pub(crate) fn text_width_as(ui: &Ui, text: &str, style: &egui::TextStyle) -> f32 {
    let font = style.resolve(ui.style());
    ui.painter()
        .layout_no_wrap(text.to_owned(), font, Color32::PLACEHOLDER)
        .size()
        .x
}

/// `text` with every digit the widest of them (in `style`'s font): as wide as it can get while
/// its digits change.
fn steadied(ui: &Ui, text: &str, style: &egui::TextStyle) -> String {
    let widest = ('0'..='9')
        .max_by(|one, other| {
            text_width_as(ui, &one.to_string(), style).total_cmp(&text_width_as(
                ui,
                &other.to_string(),
                style,
            ))
        })
        .unwrap_or('0');
    text.chars()
        .map(|character| {
            if character.is_ascii_digit() {
                widest
            } else {
                character
            }
        })
        .collect()
}

/// How wide the longest duration egit writes is ("59 min 59 s", "23 h 59 min", "99 d 23 h") in
/// `style`'s font, digits at their widest: a column of durations that never moves what is
/// beside it.
pub(crate) fn duration_width(ui: &Ui, style: &egui::TextStyle) -> f32 {
    ["59 min 59 s", "23 h 59 min", "99 d 23 h"]
        .iter()
        .map(|longest| text_width_as(ui, &steadied(ui, longest, style), style))
        .fold(0.0_f32, f32::max)
}

/// A column `width` wide and `height` tall in a row, its contents against its start, end, or
/// centre (`align`, across): as wide whatever it holds, so what follows doesn't move as that
/// changes, and columns of rows line up.
pub(crate) fn column<R>(
    ui: &mut Ui,
    (width, height): (f32, f32),
    align: egui::Align,
    add: impl FnOnce(&mut Ui) -> R,
) -> R {
    let layout = match align {
        egui::Align::Max => egui::Layout::right_to_left(egui::Align::Center),
        egui::Align::Min | egui::Align::Center => {
            egui::Layout::left_to_right(egui::Align::Center).with_main_align(align)
        }
    };
    ui.allocate_ui_with_layout(egui::vec2(width, height), layout, |ui| {
        ui.set_min_size(egui::vec2(width, height));
        ui.set_max_width(width);
        add(ui)
    })
    .inner
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
        let response = ui.add_enabled(
            false,
            Button::new(text).wrap_mode(egui::TextWrapMode::Extend),
        );
        crate::help::note(ui, response.rect, text);
        return response;
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
    let response = ui.add(
        Button::new(RichText::new(text).color(on).semibold())
            .fill(fill)
            .stroke(Stroke::NONE)
            .wrap_mode(egui::TextWrapMode::Extend),
    );
    crate::help::note(ui, response.rect, text);
    response
}

/// Any other button. Its words never wrap: a row too narrow wraps whole buttons instead.
pub(crate) fn secondary(ui: &mut Ui, enabled: bool, text: &str) -> Response {
    let response = ui.add_enabled(
        enabled,
        Button::new(text).wrap_mode(egui::TextWrapMode::Extend),
    );
    crate::help::note(ui, response.rect, text);
    response
}

/// A [`secondary`] button whose words are in the danger's colour: what loses work, beside other
/// buttons (a menu's danger items alike), quieter than [`danger`]'s fill.
pub(crate) fn risky(ui: &mut Ui, enabled: bool, text: &str) -> Response {
    let words = if enabled {
        RichText::new(text).color(Palette::of(ui.ctx()).danger)
    } else {
        RichText::new(text)
    };
    let response = ui.add_enabled(
        enabled,
        Button::new(words).wrap_mode(egui::TextWrapMode::Extend),
    );
    crate::help::note(ui, response.rect, text);
    response
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
    let response = quiet(ui, |ui| {
        ui.add(
            Button::selectable(selected, look.label(icon, text))
                .min_size(egui::vec2(width, CONTROL)),
        )
    });
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Button, response.enabled(), selected, text)
    });
    crate::help::note(ui, response.rect, text);
    if look == Look::Compact {
        response.on_hover_text(text)
    } else {
        response
    }
}

/// How wide a button showing `text` draws ([`secondary`], [`primary`], [`danger`]).
pub(crate) fn button_width(ui: &Ui, text: &str) -> f32 {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let words = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font, Color32::PLACEHOLDER)
        .size()
        .x;
    2.0f32.mul_add(ui.spacing().button_padding.x, words)
}

/// In a wrapping row, starts the next row unless `widths` (side by side, spaced) fit in what is
/// left of this one: for a part drawn in a `Ui` of its own, which can't wrap inside the row.
pub(crate) fn room_for(ui: &mut Ui, widths: &[f32]) {
    let spacing = ui.spacing().item_spacing.x;
    let needed: f32 = widths.iter().map(|width| width + spacing).sum();
    if ui.available_size_before_wrap().x < needed {
        ui.end_row();
    }
}

/// How wide [`tool`] (or, for a `menu`, [`tool_menu`]) draws its button as `look` says: its
/// words and padding, at least its least width. For a bar choosing its look before drawing.
pub(crate) fn tool_width(ui: &Ui, (icon, text): (&str, &str), look: Look, menu: bool) -> f32 {
    let mut label = look.label(icon, text);
    if menu {
        label.push_str("  ");
        label.push_str(crate::theme::icon::CARET_DOWN);
    }
    let font = egui::TextStyle::Button.resolve(ui.style());
    let words = ui
        .painter()
        .layout_no_wrap(label, font, Color32::PLACEHOLDER)
        .size()
        .x;
    let least = if look == Look::Compact && !menu {
        CONTROL
    } else {
        0.0
    };
    (2.0f32.mul_add(ui.spacing().button_padding.x, words)).max(least)
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
    let shown = menu_button(ui, icon, text, look, add_contents);
    crate::help::note(ui, shown.0.rect, text);
    shown
}

/// A [`tool_menu`] named with someone's words (a conversation's title, a model's name),
/// explained by the help topic `topic` rather than by them.
pub(crate) fn tool_menu_of<R>(
    ui: &mut Ui,
    (icon, text): (&str, &str),
    look: Look,
    topic: &'static str,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> (Response, Option<R>) {
    let shown = menu_button(ui, icon, text, look, add_contents);
    crate::help::note_topic(ui, shown.0.rect, topic);
    shown
}

/// A toolbar's menu button, unexplained.
fn menu_button<R>(
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
    let (response, inner) = quiet(ui, |ui| {
        egui::containers::menu::MenuButton::from_button(button).ui(ui, add_contents)
    });
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
    let response = quiet(ui, |ui| {
        ui.add_enabled(
            enabled,
            Button::new(glyph)
                .frame_when_inactive(false)
                .min_size(egui::vec2(CONTROL, CONTROL)),
        )
    });
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, name));
    crate::help::note(ui, response.rect, name);
    response
}

/// An [`icon_tool`] named with someone's words (a job's name), explained by help topic `topic`.
pub(crate) fn icon_tool_of(
    ui: &mut Ui,
    (glyph, name): (&str, &str),
    topic: &'static str,
) -> Response {
    let response = quiet(ui, |ui| {
        ui.add(
            Button::new(glyph)
                .frame_when_inactive(false)
                .min_size(egui::vec2(CONTROL, CONTROL)),
        )
    });
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, name));
    crate::help::note_topic(ui, response.rect, topic);
    response
}

/// A list of files' header button choosing between a list and a tree of their folders (for
/// every list: a preference); returns the preferences changed when clicked.
pub(crate) fn tree_toggle(ui: &mut Ui) -> Option<crate::preferences::Preferences> {
    let mut preferences = crate::preferences::Preferences::of(ui.ctx());
    let (glyph, name) = if preferences.files.lists.is_tree() {
        (crate::theme::icon::LIST_BULLETS, "Show the files as a list")
    } else {
        (
            crate::theme::icon::TREE_VIEW,
            "Show the files in a tree of their folders",
        )
    };
    let clicked = icon_tool(ui, true, glyph, name)
        .on_hover_text(name)
        .clicked();
    clicked.then(|| {
        preferences.files.lists = if preferences.files.lists.is_tree() {
            crate::preferences::FileLists::List
        } else {
            crate::preferences::FileLists::Tree
        };
        preferences
    })
}

/// Draws `add`'s quiet buttons (no frame until the pointer is on them) the same size whether
/// or not the pointer is on them. egui takes a button's edge out of its padding, yet leaves a
/// frameless one's edge out of its size: where inactive buttons have edges, as here, a quiet
/// one grew by its edge under the pointer, pushing everything after it. Without an inactive
/// edge, it is the same size either way.
pub(crate) fn quiet<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let edge = ui.visuals().widgets.inactive.bg_stroke;
    ui.visuals_mut().widgets.inactive.bg_stroke = Stroke::NONE;
    let inner = add(ui);
    ui.visuals_mut().widgets.inactive.bg_stroke = edge;
    inner
}

/// A button that reads as a link (a commit's name to go to): no frame until the pointer is on
/// it, and a slim inset, which it hangs into, so its words line up with text beside or above
/// it.
pub(crate) fn link(ui: &mut Ui, text: impl Into<WidgetText>) -> Response {
    // Plain words in the links' colour: they read as something to follow.
    let text = match text.into() {
        WidgetText::Text(plain) => {
            WidgetText::from(RichText::new(plain).color(ui.visuals().hyperlink_color))
        }
        other => other,
    };
    let words = text.text().to_owned();
    let response = ui
        .scope(|ui| {
            ui.spacing_mut().button_padding = egui::vec2(LINK_INSET, 2.0);
            ui.add_space(-LINK_INSET);
            quiet(ui, |ui| {
                ui.add(
                    Button::new(text)
                        .frame_when_inactive(false)
                        .min_size(egui::vec2(0.0, TARGET)),
                )
            })
        })
        .inner;
    crate::help::note(ui, response.rect, &words);
    response
}

/// A link's inset from its hover's edge, which it hangs into.
const LINK_INSET: f32 = 4.0;

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
    crate::help::note(ui, rect, name);
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
    TextEdit::singleline(text)
        .margin(Margin::symmetric(FIELD_INSET, 0))
        .min_size(egui::vec2(0.0, CONTROL))
        .vertical_align(egui::Align::Center)
}

/// A text field of several lines, filling its width, its text [`FIELD_INSET`] from every
/// edge.
pub(crate) fn text_area(text: &mut dyn TextBuffer) -> TextEdit<'_> {
    TextEdit::multiline(text)
        .margin(Margin::same(FIELD_INSET))
        .desired_width(f32::INFINITY)
}

/// Shows the scroll bars of what `ui` holds next while there is more than shows, rather than
/// only under the pointer (egui's floating bars are invisible until then): where text is cut,
/// the bar says there is more. Thin, and in room of their own, so nothing sits under them.
pub(crate) fn show_overflow(ui: &mut Ui) {
    ui.spacing_mut().scroll = egui::style::ScrollStyle {
        // Never wider than the room it keeps, less a gap from what it scrolls.
        bar_width: 6.0,
        floating_allocated_width: 8.0,
        ..egui::style::ScrollStyle::thin()
    };
}

/// A text area at most `max_height` tall (`edit`: from [`text_area`]): what doesn't fit
/// scrolls inside its edge, which stays put, the cursor kept in view as it moves. Named by
/// what `labelled_by` on the answer links it to, as any field.
pub(crate) fn bounded_text_area(
    ui: &mut Ui,
    edit: TextEdit<'_>,
    max_height: f32,
    salt: &str,
) -> Response {
    let fill = ui.visuals().text_edit_bg_color();
    let corner = ui.visuals().widgets.inactive.corner_radius;
    let shown = Frame::new()
        .fill(fill)
        .corner_radius(corner)
        .show(ui, |ui| {
            show_overflow(ui);
            egui::ScrollArea::vertical()
                .id_salt(salt)
                .max_height(max_height)
                .auto_shrink([false, true])
                // Its own edge goes; its text keeps its inset.
                .show(ui, |ui| {
                    ui.add(edit.frame(Frame::new().inner_margin(Margin::same(FIELD_INSET))))
                })
                .inner
        });
    let field = shown.inner;
    // Its edge, as a field's: the selection's while typed into, stronger under the pointer.
    let stroke = if field.has_focus() {
        ui.visuals().selection.stroke
    } else {
        ui.style().interact(&field).bg_stroke
    };
    ui.painter()
        .rect_stroke(shown.response.rect, corner, stroke, StrokeKind::Inside);
    field
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
            CornerRadius::same(crate::theme::RADIUS_CONTROL),
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
    banner(ui, Tone::Danger, |ui| {
        ui.label(
            RichText::new(format!("{}  {}", Tone::Danger.icon(), problem.title))
                .color(palette.danger)
                .semibold(),
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
