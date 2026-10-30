//! A narrow window: a phone's screen (held upright, or on its side), or a desktop window made
//! narrow. There is no room for panels side by side, so each view shows one thing at a time
//! (the history, then a commit's details, then a file), its parts stacked rather than beside
//! each other, and a bar's controls on rows of their own.
//!
//! Decided from the window's size each frame, so turning a phone (or resizing a window) moves
//! between the two at once; what each view shows is kept either way.

/// Narrower than this, a window is narrow: a phone held upright is 320 to 430 points wide.
const NARROWER_THAN: f32 = 600.0;

/// Shorter than this, and narrower than [`SIDEWAYS_NARROWER_THAN`], a window is a phone on its
/// side: panels side by side would leave each a sliver of a short screen.
const SHORTER_THAN: f32 = 500.0;

/// A phone on its side is at most this wide (the largest are 932 points).
const SIDEWAYS_NARROWER_THAN: f32 = 1_000.0;

/// Whether the window is narrow: each view shows one thing at a time.
pub(crate) fn is_narrow(ctx: &egui::Context) -> bool {
    narrow_at(ctx.content_rect().size())
}

/// How much room a window has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Room {
    /// Room for panels side by side.
    Wide,
    /// One thing at a time ([`is_narrow`]).
    Narrow,
}

impl Room {
    /// The room `ctx`'s window has.
    pub(crate) fn of(ctx: &egui::Context) -> Self {
        if is_narrow(ctx) {
            Self::Narrow
        } else {
            Self::Wide
        }
    }
}

/// Whether a window `room` in size is narrow.
fn narrow_at(room: egui::Vec2) -> bool {
    room.x < NARROWER_THAN || (room.y < SHORTER_THAN && room.x < SIDEWAYS_NARROWER_THAN)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phones_are_narrow_either_way_up_and_desktops_are_not() {
        let narrow = |width, height| narrow_at(egui::vec2(width, height));
        assert!(narrow(390.0, 844.0));
        assert!(narrow(844.0, 390.0));
        assert!(!narrow(1180.0, 800.0));
        // A tablet, and a desktop window made short but wide.
        assert!(!narrow(820.0, 1180.0));
        assert!(!narrow(1400.0, 420.0));
    }
}
