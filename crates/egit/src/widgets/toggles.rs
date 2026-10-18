//! Controls that are on or off, or one of a few: checkboxes (a square box, filled with the
//! accent and ticked when checked), radio buttons (a round one, filled and dotted), and a
//! segmented control (a few choices side by side, one picked). Each is a whole row to click,
//! at least a control's height tall, and named for screen readers with its state.

use egui::{
    Align2, Color32, Response, Sense, Shape, Stroke, StrokeKind, TextStyle, TextWrapMode, Ui,
    WidgetInfo, WidgetText, WidgetType, pos2, vec2,
};

use crate::theme::{CONTROL, Palette, RADIUS_CONTROL, RADIUS_INNER};

/// The box's (or the circle's) size.
const BOX: f32 = 16.0;

/// Between the box and its words.
const GAP: f32 = 8.0;

/// What a toggle shows when it is on.
#[derive(Clone, Copy)]
enum Mark {
    /// A tick in a square box.
    Check,
    /// A dot in a circle.
    Dot,
}

/// A checkbox for `checked`, labelled `text`; a click anywhere on its row toggles it.
pub(crate) fn checkbox(ui: &mut Ui, checked: &mut bool, text: impl Into<WidgetText>) -> Response {
    let text = text.into();
    let words = text.text().to_owned();
    let mut response = toggle(ui, *checked, text, Mark::Check);
    crate::help::note(ui, response.rect, &words);
    if response.clicked() {
        *checked = !*checked;
        response.mark_changed();
    }
    response
}

/// A radio button, `selected` or not, labelled `text`.
pub(crate) fn radio(ui: &mut Ui, selected: bool, text: impl Into<WidgetText>) -> Response {
    let text = text.into();
    let words = text.text().to_owned();
    let response = toggle(ui, selected, text, Mark::Dot);
    crate::help::note(ui, response.rect, &words);
    response
}

/// A radio button labelled with someone's words (a repository's name), explained by the help
/// topic `topic` rather than by them.
pub(crate) fn radio_of(
    ui: &mut Ui,
    selected: bool,
    text: impl Into<WidgetText>,
    topic: &'static str,
) -> Response {
    let response = toggle(ui, selected, text.into(), Mark::Dot);
    crate::help::note_topic(ui, response.rect, topic);
    response
}

/// A radio button picking `value` for `current`.
pub(crate) fn radio_value<T: PartialEq>(
    ui: &mut Ui,
    current: &mut T,
    value: T,
    text: impl Into<WidgetText>,
) -> Response {
    let mut response = radio(ui, *current == value, text);
    if response.clicked() && *current != value {
        *current = value;
        response.mark_changed();
    }
    response
}

/// Draws a toggle: its box or circle, then its words (wrapped to the room left), the box beside
/// the first line.
fn toggle(ui: &mut Ui, on: bool, text: WidgetText, mark: Mark) -> Response {
    let wrap = (ui.available_width() - BOX - GAP).max(BOX);
    let galley = text.into_galley(ui, Some(TextWrapMode::Wrap), wrap, TextStyle::Body);
    let size = vec2(BOX + GAP + galley.size().x, galley.size().y.max(CONTROL));
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let kind = match mark {
        Mark::Check => WidgetType::Checkbox,
        Mark::Dot => WidgetType::RadioButton,
    };
    response.widget_info(|| WidgetInfo::selected(kind, ui.is_enabled(), on, galley.text()));
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let palette = Palette::of(ui.ctx());
    // Beside the first line: centred in a row a control's height, or at a taller label's top.
    let first_line = galley.rows.first().map_or(BOX, |row| row.height());
    let middle = if galley.size().y <= CONTROL {
        rect.center().y
    } else {
        rect.top() + first_line / 2.0
    };
    let boxed = egui::Rect::from_center_size(pos2(rect.left() + BOX / 2.0, middle), vec2(BOX, BOX));
    paint_mark(ui, boxed, on, mark, response.hovered(), &palette);
    let text_top = if galley.size().y <= CONTROL {
        rect.center().y - galley.size().y / 2.0
    } else {
        rect.top()
    };
    ui.painter().galley(
        pos2(boxed.right() + GAP, text_top),
        galley,
        ui.visuals().text_color(),
    );
    response
}

/// The box or circle: an empty field with an edge (stronger under the pointer), or filled with
/// the accent and marked.
fn paint_mark(ui: &Ui, boxed: egui::Rect, on: bool, mark: Mark, hovered: bool, palette: &Palette) {
    let painter = ui.painter();
    let edge = if hovered {
        palette.accent
    } else {
        palette.border_strong
    };
    let (fill, stroke) = if on {
        // A shade nearer the text's colour under the pointer.
        let accent = if hovered {
            palette.accent.lerp_to_gamma(palette.text, 0.18)
        } else {
            palette.accent
        };
        (accent, Stroke::NONE)
    } else {
        (palette.field, Stroke::new(1.5, edge))
    };
    match mark {
        Mark::Check => {
            painter.rect(boxed, RADIUS_INNER, fill, stroke, StrokeKind::Inside);
            if on {
                let at = |x: f32, y: f32| {
                    pos2(
                        boxed.left() + x * boxed.width(),
                        boxed.top() + y * boxed.height(),
                    )
                };
                painter.add(Shape::line(
                    vec![at(0.26, 0.52), at(0.43, 0.69), at(0.75, 0.33)],
                    Stroke::new(2.0, palette.on_accent),
                ));
            }
        }
        Mark::Dot => {
            let radius = BOX / 2.0;
            if on {
                painter.circle_filled(boxed.center(), radius, fill);
                painter.circle_filled(boxed.center(), 3.0, palette.on_accent);
            } else {
                painter.circle(boxed.center(), radius - 0.75, fill, stroke);
            }
        }
    }
}

/// A segmented control: `options` side by side in one frame, the one `current` holds filled;
/// a click picks another. Returns whether it changed. Sized from its words before it is
/// drawn, so it is placed as any widget of a control's height is (centred on its row).
pub(crate) fn segmented<T: PartialEq + Copy>(
    ui: &mut Ui,
    current: &mut T,
    options: &[(T, &str)],
) -> bool {
    let palette = Palette::of(ui.ctx());
    let galleys: Vec<_> = options
        .iter()
        .map(|&(_, text)| {
            WidgetText::from(text).into_galley(
                ui,
                Some(TextWrapMode::Extend),
                f32::INFINITY,
                TextStyle::Button,
            )
        })
        .collect();
    let edge = SEGMENTED_MARGIN + 1.0;
    let segments: f32 = galleys
        .iter()
        .map(|galley| galley.size().x + SEGMENT_PAD)
        .sum();
    let gaps = SEGMENT_GAP * f32::from(u16::try_from(options.len().saturating_sub(1)).unwrap_or(0));
    let size = vec2(segments + gaps + 2.0 * edge, CONTROL);
    let (id, rect) = ui.allocate_space(size);
    ui.painter().rect(
        rect,
        RADIUS_CONTROL,
        palette.field,
        Stroke::new(1.0, palette.border),
        StrokeKind::Inside,
    );
    let mut changed = false;
    let mut left = rect.left() + edge;
    for (index, (&(value, text), galley)) in options.iter().zip(galleys).enumerate() {
        let width = galley.size().x + SEGMENT_PAD;
        let place = egui::Rect::from_min_size(
            pos2(left, rect.top() + edge),
            vec2(width, CONTROL - 2.0 * edge),
        );
        left += width + SEGMENT_GAP;
        crate::help::note(ui, place, text);
        let picked = *current == value;
        if segment(
            ui,
            (id.with(index), place),
            (picked, text),
            galley,
            &palette,
        )
        .clicked()
            && !picked
        {
            *current = value;
            changed = true;
        }
    }
    changed
}

/// Between a segmented control's edge and its segments: a radius of [`RADIUS_CONTROL`] less
/// it and the edge leaves [`RADIUS_INNER`], the segments' own, so their corners follow.
const SEGMENTED_MARGIN: f32 = 1.0;

/// A segment's words' room on either side, together.
const SEGMENT_PAD: f32 = 20.0;

/// Between two segments.
const SEGMENT_GAP: f32 = 2.0;

/// One segment, in `place`: its words on the accent's tint when picked, a hover's fill
/// otherwise.
fn segment(
    ui: &Ui,
    (id, place): (egui::Id, egui::Rect),
    (picked, text): (bool, &str),
    galley: std::sync::Arc<egui::Galley>,
    palette: &Palette,
) -> Response {
    let response = ui.interact(place, id, Sense::click());
    response.widget_info(|| {
        WidgetInfo::selected(WidgetType::SelectableLabel, ui.is_enabled(), picked, text)
    });
    let fill = if picked {
        let tint = palette.accent.gamma_multiply(0.28);
        if response.hovered() {
            palette.accent.gamma_multiply(0.4)
        } else {
            tint
        }
    } else if response.hovered() {
        palette.surface_alt
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(place, RADIUS_INNER, fill);
    let color = if picked { palette.text } else { palette.muted };
    ui.painter().galley(
        Align2::CENTER_CENTER
            .align_size_within_rect(galley.size(), place)
            .min,
        galley,
        color,
    );
    response
}
