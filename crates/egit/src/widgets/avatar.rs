//! People as their initials in a circle of a colour of their own, picked by their email so one
//! person looks alike everywhere. Drawn here and never fetched: no address leaves egit to find
//! a picture.

use egui::{Align2, Color32, FontId, Pos2, Sense, Stroke, Ui, Vec2};

use crate::fonts;
use crate::theme::{Palette, Weight};

/// How much of an avatar the one before it in a row covers: little, so its initials show.
const OVERLAP: f32 = 0.2;

/// At most this many avatars in a row; the rest are counted ("+2").
const SHOWN: usize = 3;

/// Up to two letters for someone: their name's first word's first and its last word's first
/// ("Ada Lovelace" AL, "A. Lovelace" AL, "Linus" L, "李小龍" 李), in upper case; else their
/// email's first letter; else "?".
fn initials(name: &str, email: &str) -> String {
    let mut words = name
        .split_whitespace()
        .filter(|word| !word.starts_with(['(', '[', '<']))
        .filter_map(|word| word.chars().find(|c| c.is_alphanumeric()));
    let first = words.next();
    let last = words.next_back();
    let mut letters = String::new();
    match (first, last) {
        (Some(first), last) => {
            // A letter's first in upper case: "ß" is "S", not "SS".
            letters.extend(first.to_uppercase().take(1));
            letters.extend(last.into_iter().flat_map(|c| c.to_uppercase().take(1)));
        }
        (None, _) => match email.chars().find(|c| c.is_alphanumeric()) {
            Some(letter) => letters.extend(letter.to_uppercase().take(1)),
            None => letters.push('?'),
        },
    }
    letters
}

/// The colour that is someone's: their email (in any case; their name without one) hashed
/// (FNV-1a) among the palette's.
fn color(palette: Palette, (name, email): (&str, &str)) -> Color32 {
    let key = if email.is_empty() { name } else { email };
    let hash = key.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte.to_ascii_lowercase())).wrapping_mul(0x0100_0000_01b3)
    });
    let count = u64::try_from(palette.avatars.len()).unwrap_or(1);
    hash.checked_rem(count)
        .and_then(|at| usize::try_from(at).ok())
        .and_then(|at| palette.avatars.get(at).copied())
        .unwrap_or(palette.accent)
}

/// Paints someone's avatar, `diameter` across, centred on `center`, ringed in `edge` (what is
/// behind it, so overlapping avatars stay apart).
fn paint_avatar(ui: &Ui, (center, diameter): (Pos2, f32), person: (&str, &str), edge: Color32) {
    let palette = Palette::of(ui.ctx());
    let radius = diameter / 2.0;
    let painter = ui.painter();
    painter.circle(
        center,
        radius,
        color(palette, person),
        Stroke::new(1.5, edge),
    );
    let letters = initials(person.0, person.1);
    let font = FontId::new((diameter * 0.42).round(), Weight::SemiBold.family());
    fonts::note_text(ui, &font, &letters);
    painter.text(
        center,
        Align2::CENTER_CENTER,
        letters,
        font,
        palette.on_avatar,
    );
}

/// Someone's avatar, `diameter` across, laid out in the row.
pub(crate) fn avatar(ui: &mut Ui, diameter: f32, person: (&str, &str), edge: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(diameter), Sense::hover());
    paint_avatar(ui, (rect.center(), diameter), person, edge);
}

/// Several people's avatars in a row, each covering a little of the one before (the first
/// on top), at most three and then how many more ("+2").
pub(crate) fn avatars(ui: &mut Ui, diameter: f32, people: &[(&str, &str)], edge: Color32) {
    let shown = people.len().min(SHOWN);
    let more = people.len().saturating_sub(SHOWN);
    let step = diameter * (1.0 - OVERLAP);
    let count = f32::from(u16::try_from(shown).unwrap_or(u16::MAX));
    let width = if shown == 0 {
        0.0
    } else {
        diameter + step * (count - 1.0)
    };
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, diameter), Sense::hover());
    // The last drawn first, so the first lies on top.
    for (at, person) in people.iter().take(shown).enumerate().rev() {
        let offset = step * f32::from(u16::try_from(at).unwrap_or(u16::MAX));
        let center = egui::pos2(rect.left() + offset + diameter / 2.0, rect.center().y);
        paint_avatar(ui, (center, diameter), *person, edge);
    }
    if more > 0 {
        let palette = Palette::of(ui.ctx());
        ui.add(
            egui::Label::new(
                egui::RichText::new(format!("+{more}"))
                    .small()
                    .color(palette.muted),
            )
            .selectable(false),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initials_are_a_names_first_and_last_words() {
        assert_eq!(initials("Ada Lovelace", "ada@example.com"), "AL");
        assert_eq!(initials("A. Lovelace", ""), "AL");
        assert_eq!(initials("ada augusta king lovelace", ""), "AL");
        assert_eq!(initials("Linus", ""), "L");
        assert_eq!(initials("李小龍", ""), "李");
        assert_eq!(initials("Ada (work)", ""), "A");
        assert_eq!(initials("éva ßtraße", ""), "ÉS");
        assert_eq!(initials("", "zoe@example.com"), "Z");
        assert_eq!(initials("  ", "@"), "?");
    }

    #[test]
    fn one_person_has_one_colour_whatever_the_case() {
        let palette = Palette::DARK;
        let ada = color(palette, ("Ada", "ada@example.com"));
        assert_eq!(color(palette, ("A. Lovelace", "ADA@example.com")), ada);
        assert!(palette.avatars.contains(&ada));
        // Different people spread across the colours.
        let colours: std::collections::HashSet<_> = (0..100)
            .map(|n| color(palette, ("", &format!("person{n}@example.com"))))
            .collect();
        assert!(colours.len() >= 8, "{}", colours.len());
    }
}
