//! Colours, fonts, and the shape of egui's widgets, for the dark and the light theme.

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
    /// Behind everything.
    pub(crate) background: Color32,
    /// Panels and the tab bar.
    pub(crate) panel: Color32,
    /// Cards and dialogs.
    pub(crate) surface: Color32,
    /// A step off `surface`: inputs, stripes, code.
    pub(crate) surface_alt: Color32,
    /// Lines between things.
    pub(crate) border: Color32,
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
}

impl Palette {
    /// The dark theme.
    pub(crate) const DARK: Self = Self {
        background: Color32::from_rgb(0x15, 0x18, 0x1C),
        panel: Color32::from_rgb(0x1B, 0x1F, 0x24),
        surface: Color32::from_rgb(0x22, 0x27, 0x2E),
        surface_alt: Color32::from_rgb(0x2A, 0x30, 0x38),
        border: Color32::from_rgb(0x36, 0x3D, 0x47),
        text: Color32::from_rgb(0xE6, 0xE9, 0xEE),
        muted: Color32::from_rgb(0x9C, 0xA6, 0xB4),
        accent: Color32::from_rgb(0x3F, 0xB6, 0xA8),
        on_accent: Color32::from_rgb(0x08, 0x18, 0x16),
        ok: Color32::from_rgb(0x5B, 0xC9, 0x87),
        warn: Color32::from_rgb(0xE5, 0xB5, 0x46),
        danger: Color32::from_rgb(0xF5, 0x7A, 0x77),
    };

    /// The light theme.
    pub(crate) const LIGHT: Self = Self {
        background: Color32::from_rgb(0xF2, 0xF4, 0xF7),
        panel: Color32::from_rgb(0xF8, 0xF9, 0xFB),
        surface: Color32::from_rgb(0xFF, 0xFF, 0xFF),
        surface_alt: Color32::from_rgb(0xEE, 0xF1, 0xF4),
        border: Color32::from_rgb(0xD5, 0xDB, 0xE3),
        text: Color32::from_rgb(0x1A, 0x21, 0x2C),
        muted: Color32::from_rgb(0x55, 0x60, 0x71),
        accent: Color32::from_rgb(0x0A, 0x84, 0x77),
        on_accent: Color32::from_rgb(0xFF, 0xFF, 0xFF),
        ok: Color32::from_rgb(0x18, 0x76, 0x35),
        warn: Color32::from_rgb(0x8F, 0x5B, 0x00),
        danger: Color32::from_rgb(0xC0, 0x2B, 0x26),
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

/// Installs the fonts: egui's, with Phosphor's icons as a fallback of both families.
pub(crate) fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
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
    visuals.window_corner_radius = CornerRadius::same(10);
    visuals.menu_corner_radius = CornerRadius::same(8);
    visuals.hyperlink_color = palette.accent;
    visuals.warn_fg_color = palette.warn;
    visuals.error_fg_color = palette.danger;
    visuals.selection.bg_fill = palette.accent.gamma_multiply(0.35);
    visuals.selection.stroke = Stroke::new(1.0, palette.accent);
    visuals.override_text_color = None;

    let radius = CornerRadius::same(6);
    for (state, fill) in [
        (&mut visuals.widgets.noninteractive, palette.surface),
        (&mut visuals.widgets.inactive, palette.surface_alt),
        (&mut visuals.widgets.hovered, palette.border),
        (&mut visuals.widgets.active, palette.border),
        (&mut visuals.widgets.open, palette.surface_alt),
    ] {
        state.corner_radius = radius;
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
    }
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, palette.border);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, palette.text);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, palette.text);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.5, palette.text);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, palette.accent.gamma_multiply(0.6));
    visuals.widgets.active.fg_stroke = Stroke::new(1.5, palette.text);
    visuals.widgets.active.bg_stroke = Stroke::new(1.0, palette.accent);
    style.visuals = visuals;

    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(10.0, 4.0);
    style.spacing.interact_size.y = 26.0;
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
        }
    }
}
