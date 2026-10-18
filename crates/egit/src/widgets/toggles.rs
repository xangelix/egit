//! Controls that are on or off, or one of a few: checkboxes (a square box, filled with the
//! accent and ticked when checked), radio buttons (a round one, filled and dotted), and a
//! segmented control (a few choices side by side, one picked). Each is a whole row to click,
//! at least a control's height tall, and named for screen readers with its state.

use egui::{
    Align2, Color32, Response, Sense, Shape, Stroke, StrokeKind, TextStyle, TextWrapMode, Ui,
    WidgetInfo, WidgetText, WidgetType, pos2, vec2,
};

use crate::theme::{CONTROL, Palette};

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
    let mut response = toggle(ui, *checked, text.into(), Mark::Check);
    if response.clicked() {
        *checked = !*checked;
        response.mark_changed();
    }
    response
}

/// A radio button, `selected` or not, labelled `text`.
pub(crate) fn radio(ui: &mut Ui, selected: bool, text: impl Into<WidgetText>) -> Response {
    toggle(ui, selected, text.into(), Mark::Dot)
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
            painter.rect(boxed, 4.0, fill, stroke, StrokeKind::Inside);
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
/// a click picks another. Returns whether it changed.
pub(crate) fn segmented<T: PartialEq + Copy>(
    ui: &mut Ui,
    current: &mut T,
    options: &[(T, &str)],
) -> bool {
    let palette = Palette::of(ui.ctx());
    let mut changed = false;
    egui::Frame::new()
        .fill(palette.field)
        .stroke(Stroke::new(1.0, palette.border))
        .corner_radius(6)
        .inner_margin(2)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.horizontal(|ui| {
                for &(value, text) in options {
                    let picked = *current == value;
                    let segment = segment(ui, picked, text, &palette);
                    if segment.clicked() && !picked {
                        *current = value;
                        changed = true;
                    }
                }
            });
        });
    changed
}

/// One segment: its words on the accent's tint when picked, a hover's fill otherwise.
fn segment(ui: &mut Ui, picked: bool, text: &str, palette: &Palette) -> Response {
    let galley = WidgetText::from(text).into_galley(
        ui,
        Some(TextWrapMode::Extend),
        f32::INFINITY,
        TextStyle::Button,
    );
    let height = CONTROL - 4.0;
    let size = vec2(galley.size().x + 20.0, height);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
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
    ui.painter().rect_filled(rect, 4.0, fill);
    let color = if picked { palette.text } else { palette.muted };
    ui.painter().galley(
        Align2::CENTER_CENTER
            .align_size_within_rect(galley.size(), rect)
            .min,
        galley,
        color,
    );
    response
}
