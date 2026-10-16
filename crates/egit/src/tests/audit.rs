//! The checks every screen passes (after eshine's): no two widgets share an id, no text or
//! control is drawn partly over another on the same layer, nothing moves while the window sits
//! idle, and every control has a name made of words or numbers, not just an icon.
//!
//! Elements covered by something else or scrolled out of view are skipped.

use egui::accesskit::{Action, Role};
use egui::{LayerId, Rect};
use egui_kittest::Harness;
use egui_kittest::kittest::NodeT as _;

use crate::app::EgitApp;

/// One element of a screen.
#[derive(Clone, Debug)]
struct Element {
    role: Role,
    label: String,
    rect: Rect,
    /// The part of `rect` its containers let show.
    visible: Rect,
    layer: LayerId,
    control: bool,
}

impl Element {
    fn describe(&self) -> String {
        format!(
            "{:?} {:?} at ({:.0}, {:.0}) {:.0}×{:.0}",
            self.role,
            self.label,
            self.rect.min.x,
            self.rect.min.y,
            self.rect.width(),
            self.rect.height()
        )
    }
}

/// Runs every check on what `harness` shows; returns a line per problem.
pub(crate) fn run(harness: &mut Harness<'_, EgitApp>, screen: &str) -> Vec<String> {
    harness
        .ctx
        .options_mut(|options| options.warn_on_id_clash = true);
    harness.run_steps(3);
    let mut problems = id_clashes(harness, screen);
    let before = elements(harness);
    harness.run_steps(3);
    let after = elements(harness);

    for element in before
        .iter()
        .filter(|element| element.visible.is_positive())
    {
        let same = |other: &&Element| other.role == element.role && other.label == element.label;
        if before.iter().filter(same).count() == 1
            && let Some(moved) = after.iter().find(same)
            && moved.rect.min.distance(element.rect.min) > 0.5
        {
            problems.push(format!(
                "{screen}: moves while idle (to {:.0}, {:.0}): {}",
                moved.rect.min.x,
                moved.rect.min.y,
                element.describe()
            ));
        }
    }

    for element in after
        .iter()
        .filter(|element| element.control && element.visible.is_positive())
    {
        // Words or numbers, not only icon glyphs (which live in the private-use area).
        let named = element
            .label
            .chars()
            .any(|c| c.is_alphanumeric() && !('\u{E000}'..='\u{F8FF}').contains(&c));
        // Dividers and scroll bars are dragged, not read.
        let handle = element.role == Role::ScrollBar
            || element.rect.width().min(element.rect.height()) < 8.0;
        if !named && !handle {
            problems.push(format!(
                "{screen}: a control without a name in words: {}",
                element.describe()
            ));
        }
    }

    let shown: Vec<&Element> = after
        .iter()
        .filter(|element| element.visible.is_positive())
        .collect();
    for (index, a) in shown.iter().enumerate() {
        for b in shown.iter().skip(index.saturating_add(1)) {
            let shared = a.visible.intersect(b.visible);
            let nested = a.visible.expand(1.0).contains_rect(b.visible)
                || b.visible.expand(1.0).contains_rect(a.visible);
            if a.layer == b.layer && shared.is_positive() && shared.area() > 4.0 && !nested {
                problems.push(format!(
                    "{screen}: drawn over each other: {} and {}",
                    a.describe(),
                    b.describe()
                ));
            }
        }
    }
    problems
}

/// Asserts that `screen` passes every check.
pub(crate) fn assert_clean(harness: &mut Harness<'_, EgitApp>, screen: &str) {
    let problems = run(harness, screen);
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// Every labelled or interactive element shown.
fn elements(harness: &Harness<'_, EgitApp>) -> Vec<Element> {
    let ctx = &harness.ctx;
    let screen = ctx.content_rect();
    let layers: Vec<LayerId> =
        ctx.memory(|memory| memory.areas().visible_layer_ids().into_iter().collect());
    let reachable = ctx.interactive_rects_last_pass();
    let mut found = Vec::new();
    for node in harness.root().children_recursive() {
        let access = node.accesskit_node();
        let data = access.data();
        let role = data.role();
        if matches!(
            role,
            Role::GenericContainer | Role::Window | Role::ScrollView | Role::Unknown
        ) || access.is_hidden()
            || access.is_disabled()
            || access.bounding_box().is_none()
        {
            continue;
        }
        let rect = node.rect();
        if rect.width() < 1.0 || rect.height() < 1.0 || !screen.contains(rect.center()) {
            continue;
        }
        let editable = matches!(
            role,
            Role::TextInput | Role::MultilineTextInput | Role::SearchInput | Role::PasswordInput
        );
        let control = !matches!(role, Role::Label | Role::Image)
            && (data.supports_action(Action::Click) || editable);
        // A label is found by its own text: one scrolled out of its panel may otherwise borrow a
        // neighbouring panel's shapes.
        let text = (role == Role::Label)
            .then(|| access.value())
            .flatten()
            .filter(|text| !text.is_empty());
        let mut visible = visible_part(harness, rect, text.as_deref());
        if control {
            visible = reachable_part(&reachable, rect, visible);
        }
        let mut layer = LayerId::background();
        let mut parent = node.parent();
        while let Some(ancestor) = parent {
            let id = ancestor.accesskit_node().locate().0.0;
            if let Some(found_layer) = layers.iter().find(|layer| layer.id.value() == id) {
                layer = *found_layer;
                break;
            }
            parent = ancestor.parent();
        }
        found.push(Element {
            role,
            label: access
                .label()
                .or_else(|| access.value())
                .unwrap_or_default(),
            rect,
            visible,
            layer,
            control,
        });
    }
    found
}

/// egui's own warning, painted where two widgets share an id: they would hover, click, and
/// toggle as one.
fn id_clashes(harness: &Harness<'_, EgitApp>, screen: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for clipped in &harness.output().shapes {
        if let egui::Shape::Text(text) = &clipped.shape {
            let text = text.galley.text();
            if text.contains(" use of ") && text.contains(" ID ") {
                let bounds = clipped.shape.visual_bounding_rect();
                let line = format!(
                    "{screen}: two widgets share an id: {} at ({:.0}, {:.0})",
                    text.trim_start_matches("🔥 "),
                    bounds.min.x,
                    bounds.min.y
                );
                if !found.contains(&line) {
                    found.push(line);
                }
            }
        }
    }
    found
}

/// The part of `rect` that shows: its shapes are clipped by the panel or scroll view around it.
/// A widget that painted nothing is outside its clip (egui skips it). With `text`, only text that
/// includes it counts as the element's own.
fn visible_part(harness: &Harness<'_, EgitApp>, rect: Rect, text: Option<&str>) -> Rect {
    harness
        .output()
        .shapes
        .iter()
        .rev()
        .find(|clipped| {
            let bounds = clipped.shape.visual_bounding_rect();
            let own = match (&clipped.shape, text) {
                (_, None) => true,
                (egui::Shape::Text(shape), Some(text)) => shape.galley.text().contains(text),
                _ => false,
            };
            own && !matches!(clipped.shape, egui::Shape::Mesh(_))
                && bounds.is_positive()
                && rect.expand(2.0).contains_rect(bounds)
        })
        .map_or(Rect::NOTHING, |clipped| rect.intersect(clipped.clip_rect))
}

/// The part of a control's visible area egui would also hit-test.
fn reachable_part(reachable: &[Rect], rect: Rect, visible: Rect) -> Rect {
    let own = reachable
        .iter()
        .filter(|candidate| rect.expand(1.0).contains_rect(**candidate))
        .fold(Rect::NOTHING, |union, candidate| union.union(*candidate));
    if own.is_positive() {
        visible.intersect(own)
    } else {
        Rect::NOTHING
    }
}
