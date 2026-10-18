//! Colours, fonts, and the shape of egui's widgets, for the dark and the light theme.

use std::sync::{Arc, LazyLock};

use egui::{Color32, CornerRadius, Stroke, Visuals};
use serde::{Deserialize, Serialize};

/// Phosphor's icons, as characters of the font every text style falls back to.
pub(crate) use egui_phosphor::regular as icon;

/// The theme the user chose.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ThemeChoice {
    /// Whatever the desktop prefers.
    #[default]
    System,
    /// Always dark.
    Dark,
    /// Always light.
    Light,
}

/// One theme's colours.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Palette {
    /// Whether it is the dark theme (syntax colours follow).
    pub(crate) dark: bool,
    /// Behind what is read: the commit table, a diff, a file.
    pub(crate) background: Color32,
    /// The chrome around it (bars, the sidebar, the details), the tab bar, and pages.
    pub(crate) panel: Color32,
    /// Cards and dialogs.
    pub(crate) surface: Color32,
    /// A step off `surface`: inputs, stripes, code.
    pub(crate) surface_alt: Color32,
    /// Lines between things.
    pub(crate) border: Color32,
    /// Edges that must show: a field's or a checkbox's under the pointer.
    pub(crate) border_strong: Color32,
    /// Behind what is typed: text fields, a checkbox's box.
    pub(crate) field: Color32,
    /// Text.
    pub(crate) text: Color32,
    /// Secondary text.
    pub(crate) muted: Color32,
    /// The accent: primary buttons, selection, focus.
    pub(crate) accent: Color32,
    /// Text on the accent.
    pub(crate) on_accent: Color32,
    /// Good news.
    pub(crate) ok: Color32,
    /// Something to watch.
    pub(crate) warn: Color32,
    /// Something wrong.
    pub(crate) danger: Color32,
    /// Text on the danger colour.
    pub(crate) on_danger: Color32,
    /// The commit graph's lanes, in turn.
    pub(crate) lanes: [Color32; 10],
    /// Behind a diff's added lines, and its changed words.
    pub(crate) added: Color32,
    pub(crate) added_word: Color32,
    /// Behind a diff's removed lines, and its changed words.
    pub(crate) removed: Color32,
    pub(crate) removed_word: Color32,
    /// Behind a diff's hunk headers.
    pub(crate) hunk: Color32,
    /// Behind a conflict's current lines, and its current side's header.
    pub(crate) current: Color32,
    pub(crate) current_header: Color32,
    /// Behind a conflict's incoming lines, and its incoming side's header.
    pub(crate) incoming: Color32,
    pub(crate) incoming_header: Color32,
    /// Behind a resolved conflict's lines.
    pub(crate) resolved: Color32,
    /// Behind the commit table's rows a search found.
    pub(crate) found: Color32,
    /// Over a selected (or found) row while the pointer rests on it: a little lighter, which
    /// keeps its text as readable in either theme.
    pub(crate) selected_hover: Color32,
}

impl Palette {
    /// The dark theme.
    pub(crate) const DARK: Self = Self {
        dark: true,
        background: Color32::from_rgb(0x12, 0x15, 0x19),
        panel: Color32::from_rgb(0x1C, 0x20, 0x26),
        surface: Color32::from_rgb(0x24, 0x29, 0x30),
        surface_alt: Color32::from_rgb(0x2C, 0x32, 0x3A),
        border: Color32::from_rgb(0x36, 0x3D, 0x47),
        border_strong: Color32::from_rgb(0x55, 0x5F, 0x6D),
        field: Color32::from_rgb(0x12, 0x15, 0x19),
        text: Color32::from_rgb(0xE6, 0xE9, 0xEE),
        muted: Color32::from_rgb(0x9C, 0xA6, 0xB4),
        accent: Color32::from_rgb(0x3F, 0xB6, 0xA8),
        on_accent: Color32::from_rgb(0x08, 0x18, 0x16),
        ok: Color32::from_rgb(0x5B, 0xC9, 0x87),
        warn: Color32::from_rgb(0xE5, 0xB5, 0x46),
        danger: Color32::from_rgb(0xF5, 0x7A, 0x77),
        on_danger: Color32::from_rgb(0x2A, 0x08, 0x08),
        lanes: [
            Color32::from_rgb(0x4F, 0xC3, 0xF7),
            Color32::from_rgb(0x81, 0xC7, 0x84),
            Color32::from_rgb(0xFF, 0xB7, 0x4D),
            Color32::from_rgb(0xBA, 0x68, 0xC8),
            Color32::from_rgb(0xF0, 0x62, 0x92),
            Color32::from_rgb(0x4D, 0xD0, 0xE1),
            Color32::from_rgb(0xDC, 0xE7, 0x75),
            Color32::from_rgb(0xFF, 0x8A, 0x65),
            Color32::from_rgb(0x95, 0x75, 0xCD),
            Color32::from_rgb(0xA1, 0x88, 0x7F),
        ],
        added: Color32::from_rgba_unmultiplied_const(0x3F, 0xB9, 0x50, 40),
        added_word: Color32::from_rgba_unmultiplied_const(0x3F, 0xB9, 0x50, 100),
        removed: Color32::from_rgba_unmultiplied_const(0xF8, 0x51, 0x49, 40),
        removed_word: Color32::from_rgba_unmultiplied_const(0xF8, 0x51, 0x49, 100),
        hunk: Color32::from_rgba_unmultiplied_const(0x3F, 0xB6, 0xA8, 30),
        current: Color32::from_rgba_unmultiplied_const(0x4F, 0xC3, 0xF7, 30),
        current_header: Color32::from_rgba_unmultiplied_const(0x4F, 0xC3, 0xF7, 64),
        incoming: Color32::from_rgba_unmultiplied_const(0xB3, 0x88, 0xFF, 30),
        incoming_header: Color32::from_rgba_unmultiplied_const(0xB3, 0x88, 0xFF, 64),
        resolved: Color32::from_rgba_unmultiplied_const(0x3F, 0xB6, 0xA8, 22),
        found: Color32::from_rgba_unmultiplied_const(0xE5, 0xB5, 0x46, 40),
        selected_hover: Color32::from_rgba_unmultiplied_const(0xFF, 0xFF, 0xFF, 12),
    };

    /// The light theme.
    pub(crate) const LIGHT: Self = Self {
        dark: false,
        background: Color32::from_rgb(0xFF, 0xFF, 0xFF),
        panel: Color32::from_rgb(0xF3, 0xF5, 0xF8),
        surface: Color32::from_rgb(0xFF, 0xFF, 0xFF),
        surface_alt: Color32::from_rgb(0xEB, 0xEE, 0xF2),
        border: Color32::from_rgb(0xD5, 0xDB, 0xE3),
        border_strong: Color32::from_rgb(0x9A, 0xA4, 0xB2),
        field: Color32::from_rgb(0xFF, 0xFF, 0xFF),
        text: Color32::from_rgb(0x1A, 0x21, 0x2C),
        muted: Color32::from_rgb(0x55, 0x60, 0x71),
        accent: Color32::from_rgb(0x0A, 0x84, 0x77),
        on_accent: Color32::from_rgb(0xFF, 0xFF, 0xFF),
        ok: Color32::from_rgb(0x18, 0x76, 0x35),
        warn: Color32::from_rgb(0x8F, 0x5B, 0x00),
        danger: Color32::from_rgb(0xC0, 0x2B, 0x26),
        on_danger: Color32::from_rgb(0xFF, 0xFF, 0xFF),
        lanes: [
            Color32::from_rgb(0x02, 0x77, 0xBD),
            Color32::from_rgb(0x2E, 0x7D, 0x32),
            Color32::from_rgb(0xEF, 0x6C, 0x00),
            Color32::from_rgb(0x7B, 0x1F, 0xA2),
            Color32::from_rgb(0xC2, 0x18, 0x5B),
            Color32::from_rgb(0x00, 0x83, 0x8F),
            Color32::from_rgb(0x82, 0x77, 0x17),
            Color32::from_rgb(0xD8, 0x43, 0x15),
            Color32::from_rgb(0x45, 0x27, 0xA0),
            Color32::from_rgb(0x6D, 0x4C, 0x41),
        ],
        added: Color32::from_rgba_unmultiplied_const(0x1A, 0x7F, 0x37, 32),
        added_word: Color32::from_rgba_unmultiplied_const(0x1A, 0x7F, 0x37, 80),
        removed: Color32::from_rgba_unmultiplied_const(0xCF, 0x22, 0x2E, 32),
        removed_word: Color32::from_rgba_unmultiplied_const(0xCF, 0x22, 0x2E, 80),
        hunk: Color32::from_rgba_unmultiplied_const(0x0A, 0x84, 0x77, 26),
        current: Color32::from_rgba_unmultiplied_const(0x02, 0x77, 0xBD, 24),
        current_header: Color32::from_rgba_unmultiplied_const(0x02, 0x77, 0xBD, 52),
        incoming: Color32::from_rgba_unmultiplied_const(0x7B, 0x1F, 0xA2, 22),
        incoming_header: Color32::from_rgba_unmultiplied_const(0x7B, 0x1F, 0xA2, 46),
        resolved: Color32::from_rgba_unmultiplied_const(0x0A, 0x84, 0x77, 20),
        found: Color32::from_rgba_unmultiplied_const(0xE8, 0xA8, 0x00, 56),
        selected_hover: Color32::from_rgba_unmultiplied_const(0xFF, 0xFF, 0xFF, 48),
    };

    /// The palette egui is currently showing.
    pub(crate) fn of(ctx: &egui::Context) -> Self {
        if ctx.global_style().visuals.dark_mode {
            Self::DARK
        } else {
            Self::LIGHT
        }
    }
}

/// Small notes: where a setting comes from, a status bar's text.
pub(crate) const SMALL: f32 = 11.5;
/// The body's text, and buttons'.
pub(crate) const BODY: f32 = 13.0;
/// Code, names of commits, paths in a diff.
pub(crate) const CODE: f32 = 12.5;
/// A page's or a dialog's title.
pub(crate) const TITLE: f32 = 18.0;
/// A card's or a section's heading.
pub(crate) const HEADING: f32 = 14.0;
/// Corner radii, from the inside out: each container a step rounder than what sits inside
/// it, so their corners follow one another. What sits inside a control or a box: a row's
/// highlight, a segment, a badge, a section's band, a checkbox.
pub(crate) const RADIUS_INNER: u8 = 4;
/// Controls: buttons, fields, segmented controls (and the focus ring inside them).
pub(crate) const RADIUS_CONTROL: u8 = 6;
/// Boxes and menus: a list's or a text's box, a menu, a banner, a problem's box.
pub(crate) const RADIUS_BOX: u8 = 8;
/// Cards, dialogs, and windows.
pub(crate) const RADIUS_CARD: u8 = 12;

/// A control's height: buttons, fields, menus' buttons.
pub(crate) const CONTROL: f32 = 28.0;

/// Behind what is selected (text, a table's rows).
pub(crate) fn selection(palette: &Palette) -> Color32 {
    palette.accent.gamma_multiply(0.35)
}

/// Geist, the interface's font (1.800, as Google Fonts ships it; SIL Open Font License 1.1:
/// `fonts/OFL-Geist.txt`): one variable font holding every weight. Unlike Inter, it puts no
/// glyphs where Phosphor's icons are (Unicode's private use area), so the icons show though
/// it comes first, setting the lines' height.
static GEIST: &[u8] = include_bytes!("../fonts/Geist.ttf");

/// A weight of the interface's text: regular for reading, heavier to stand out (a title, a
/// section's name, a button's words).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Weight {
    Regular,
    Medium,
    SemiBold,
}

impl Weight {
    /// Every weight.
    pub(crate) const ALL: [Self; 3] = [Self::Regular, Self::Medium, Self::SemiBold];

    /// Its font's name among egui's, and its `wght` on Geist's axis.
    const fn font(self) -> (&'static str, f32) {
        match self {
            Self::Regular => ("Geist", 400.0),
            Self::Medium => ("Geist Medium", 500.0),
            Self::SemiBold => ("Geist SemiBold", 600.0),
        }
    }

    /// The family drawing text in this weight (regular is egui's proportional family).
    pub(crate) fn family(self) -> egui::FontFamily {
        static MEDIUM: LazyLock<egui::FontFamily> =
            LazyLock::new(|| egui::FontFamily::Name(Weight::Medium.font().0.into()));
        static SEMIBOLD: LazyLock<egui::FontFamily> =
            LazyLock::new(|| egui::FontFamily::Name(Weight::SemiBold.font().0.into()));
        match self {
            Self::Regular => egui::FontFamily::Proportional,
            Self::Medium => MEDIUM.clone(),
            Self::SemiBold => SEMIBOLD.clone(),
        }
    }
}

/// Text in a weight of the interface's font.
pub(crate) trait Weighted {
    /// A little heavier: a button's words, a name that leads its line.
    fn medium(self) -> Self;
    /// Heavier still: a title, a section's name, what a panel is about.
    fn semibold(self) -> Self;
}

impl Weighted for egui::RichText {
    fn medium(self) -> Self {
        self.family(Weight::Medium.family())
    }

    fn semibold(self) -> Self {
        self.family(Weight::SemiBold.family())
    }
}

/// Installs the fonts: Geist in each weight, then egui's own (for scripts and emoji Geist
/// lacks) and Phosphor's icons behind each; the monospace family stays egui's. The font's
/// bytes are borrowed, never copied.
pub(crate) fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    let behind = fonts
        .families
        .get(&egui::FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    for weight in Weight::ALL {
        let (name, wght) = weight.font();
        let tweak = egui::FontTweak {
            coords: egui::epaint::text::VariationCoords::new([(b"wght", wght)]),
            ..egui::FontTweak::default()
        };
        fonts.font_data.insert(
            name.to_owned(),
            Arc::new(egui::FontData::from_static(GEIST).tweak(tweak)),
        );
        let family = std::iter::once(name.to_owned())
            .chain(behind.iter().cloned())
            .collect();
        fonts.families.insert(weight.family(), family);
    }
    ctx.set_fonts(fonts);
}

/// Applies `choice` and shapes both themes' styles.
pub(crate) fn apply(ctx: &egui::Context, choice: ThemeChoice) {
    ctx.set_theme(match choice {
        ThemeChoice::System => egui::ThemePreference::System,
        ThemeChoice::Dark => egui::ThemePreference::Dark,
        ThemeChoice::Light => egui::ThemePreference::Light,
    });
    ctx.style_mut_of(egui::Theme::Dark, |style| {
        shape(style, &Palette::DARK, true);
    });
    ctx.style_mut_of(egui::Theme::Light, |style| {
        shape(style, &Palette::LIGHT, false);
    });
}

/// One theme's style.
fn shape(style: &mut egui::Style, palette: &Palette, dark: bool) {
    let mut visuals = if dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };
    visuals.panel_fill = palette.panel;
    visuals.window_fill = palette.surface;
    visuals.faint_bg_color = palette.surface_alt;
    visuals.extreme_bg_color = palette.background;
    visuals.code_bg_color = palette.surface_alt;
    visuals.window_stroke = Stroke::new(1.0, palette.border);
    visuals.window_corner_radius = CornerRadius::same(RADIUS_CARD);
    visuals.menu_corner_radius = CornerRadius::same(RADIUS_BOX);
    visuals.hyperlink_color = palette.accent;
    visuals.warn_fg_color = palette.warn;
    visuals.error_fg_color = palette.danger;
    visuals.selection.bg_fill = selection(palette);
    visuals.selection.stroke = Stroke::new(1.0, palette.accent);
    visuals.override_text_color = None;

    visuals.text_edit_bg_color = Some(palette.field);

    // Every control has an edge: a field shows where to type before it is clicked, a button
    // where it ends; the pointer darkens (or lightens) it, a press marks it with the accent.
    let radius = CornerRadius::same(RADIUS_CONTROL);
    for (state, fill, edge) in [
        (
            &mut visuals.widgets.noninteractive,
            palette.surface,
            palette.border,
        ),
        (
            &mut visuals.widgets.inactive,
            palette.surface_alt,
            palette.border,
        ),
        (
            &mut visuals.widgets.hovered,
            palette.border,
            palette.border_strong,
        ),
        (&mut visuals.widgets.active, palette.border, palette.accent),
        (
            &mut visuals.widgets.open,
            palette.surface_alt,
            palette.border_strong,
        ),
    ] {
        state.corner_radius = radius;
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
        state.bg_stroke = Stroke::new(1.0, edge);
        state.expansion = 0.0;
    }
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, palette.text);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, palette.text);
    // Lines a control draws (a tab's cross, a menu's caret) thicken under the pointer.
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.5, palette.text);
    visuals.widgets.active.fg_stroke = Stroke::new(1.5, palette.text);
    visuals.widgets.open.fg_stroke = Stroke::new(1.0, palette.text);
    style.visuals = visuals;

    // One scale of text: small notes, the body (and buttons), code, headings.
    style.text_styles = [
        (egui::TextStyle::Small, egui::FontId::proportional(SMALL)),
        (egui::TextStyle::Body, egui::FontId::proportional(BODY)),
        (
            egui::TextStyle::Button,
            egui::FontId::new(BODY, Weight::Medium.family()),
        ),
        (egui::TextStyle::Monospace, egui::FontId::monospace(CODE)),
        (
            egui::TextStyle::Heading,
            egui::FontId::new(TITLE, Weight::SemiBold.family()),
        ),
    ]
    .into();

    // Controls are 28 points tall, their text 12 from their sides; things in a row 8 apart.
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(12.0, 5.0);
    style.spacing.interact_size = egui::vec2(28.0, CONTROL);
    style.spacing.icon_width = 16.0;
    style.spacing.icon_width_inner = 8.0;
    style.spacing.icon_spacing = 8.0;
    style.spacing.combo_height = 320.0;
    style.spacing.menu_margin = egui::Margin::same(6);
    style.interaction.tooltip_delay = 0.4;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The WCAG relative luminance of a colour.
    fn luminance(color: Color32) -> f32 {
        let channel = |value: u8| {
            let value = f32::from(value) / 255.0;
            if value <= 0.039_28 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(color.r()) + 0.7152 * channel(color.g()) + 0.0722 * channel(color.b())
    }

    fn contrast(a: Color32, b: Color32) -> f32 {
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn text_meets_wcag_aa_on_every_background() {
        for palette in [Palette::DARK, Palette::LIGHT] {
            for background in [
                palette.background,
                palette.panel,
                palette.surface,
                palette.surface_alt,
                palette.field,
            ] {
                for (name, text) in [
                    ("text", palette.text),
                    ("muted", palette.muted),
                    ("ok", palette.ok),
                    ("warn", palette.warn),
                    ("danger", palette.danger),
                ] {
                    let ratio = contrast(text, background);
                    assert!(ratio >= 4.5, "{name} on {background:?}: {ratio:.2}");
                }
            }
            assert!(contrast(palette.on_accent, palette.accent) >= 4.5);
            assert!(contrast(palette.on_danger, palette.danger) >= 4.5);
        }
    }

    /// `over` (premultiplied, maybe translucent) drawn over the opaque `under`.
    // The relaxations `clippy.toml` gives `#[test]` functions, for this helper.
    #[allow(
        clippy::arithmetic_side_effects,
        clippy::integer_division,
        clippy::unwrap_used
    )]
    fn blend(over: Color32, under: Color32) -> Color32 {
        let mix = |top: u8, bottom: u8| {
            let rest = u16::from(255 - over.a());
            top + u8::try_from(u16::from(bottom) * rest / 255).unwrap()
        };
        Color32::from_rgb(
            mix(over.r(), under.r()),
            mix(over.g(), under.g()),
            mix(over.b(), under.b()),
        )
    }

    /// Text on a selected row, and on one the pointer rests on.
    #[test]
    fn text_meets_wcag_aa_on_a_selected_row() {
        for palette in [Palette::DARK, Palette::LIGHT] {
            for under in [palette.background, palette.panel, palette.surface] {
                let selected = blend(selection(&palette), under);
                let hovered = blend(palette.selected_hover, selected);
                for background in [selected, hovered] {
                    let ratio = contrast(palette.text, background);
                    assert!(
                        ratio >= 4.5,
                        "{background:?} ({}): {ratio:.2}",
                        palette.dark
                    );
                }
            }
        }
    }

    /// Text on every background a line takes in the grid; muted text (hunk headers, a
    /// conflict's headers but the selected one's) on all but words' highlights and the selected
    /// conflict's headers, which always take the text colour.
    #[test]
    fn text_meets_wcag_aa_on_every_line_background() {
        for palette in [Palette::DARK, Palette::LIGHT] {
            for (name, line, muted) in [
                ("added", palette.added, true),
                ("added word", palette.added_word, false),
                ("removed", palette.removed, true),
                ("removed word", palette.removed_word, false),
                ("hunk", palette.hunk, true),
                ("current", palette.current, true),
                ("current header", palette.current_header, false),
                ("incoming", palette.incoming, true),
                ("incoming header", palette.incoming_header, false),
                ("resolved", palette.resolved, true),
                ("found", palette.found, true),
            ] {
                let background = blend(line, palette.background);
                // A found row under the pointer, too.
                let hovered = if name == "found" {
                    blend(palette.selected_hover, background)
                } else {
                    background
                };
                let texts = [Some(palette.text), muted.then_some(palette.muted)];
                for text in texts.into_iter().flatten() {
                    for background in [background, hovered] {
                        let ratio = contrast(text, background);
                        assert!(ratio >= 4.5, "{name} ({}): {ratio:.2}", palette.dark);
                    }
                }
            }
        }
    }
}
