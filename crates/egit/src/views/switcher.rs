//! The branch switcher: the task bar's branch (and how it compares with its upstream) is a menu
//! of where to go. Its filter is typed into at once (smart case, the letters in order: as the
//! palette matches); the local branches come first (the one checked out marked, not offered),
//! then the remotes' (each through a local branch tracking it, unless one of its name is here:
//! then that one), the best first while something is typed; ↑ ↓ and Enter, or a click, go;
//! "Create a branch here…" makes one where `HEAD` is. The names are gathered once a read of the
//! refs, sorted (natural order, as the sidebar sorts them), and matched again only as the
//! filter changes.

use bstr::{BStr, BString, ByteSlice as _};
use egit_git::{RefKind, SwitchTo};
use egui::{Key, Modifiers, RichText, Ui};

use crate::theme::{Palette, icon};
use crate::widgets;
use egit_git::Reference;

/// The refs, and which read of them they are.
type Refs = (std::sync::Arc<[Reference]>, u64);

/// The most rows shown at once; the rest scroll.
const SHOWN: f32 = 10.0;

/// A row's height.
const ROW: f32 = 28.0;

/// The help topic explaining it.
pub(super) const TOPIC: &str = "branch-switcher";

/// What the switcher asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Switch {
    /// Go there.
    To(SwitchTo),
    /// Make a branch where `HEAD` is (a dialog asks its name).
    Create,
}

/// The switcher's state: what is typed, the row picked, and the names gathered (with which
/// read of the refs they are of, and those the filter keeps).
#[derive(Debug, Default)]
pub(super) struct Switcher {
    query: String,
    picked: usize,
    /// The refs' read the names are of (0: none gathered).
    read: u64,
    /// Local branches, then the remotes', each sorted.
    names: Vec<(RefKind, BString)>,
    /// What the filter keeps (places in `names`), the best first while it holds something;
    /// and the filter they are for.
    kept: Vec<usize>,
    kept_for: Option<String>,
    /// Whether the filter has had the focus since the menu opened.
    focused: bool,
}

impl Switcher {
    /// Gathers the branches' names from `refs` (once a read: `read`), sorted.
    fn gather(&mut self, (refs, read): &Refs) {
        if self.read == *read && self.read != 0 {
            return;
        }
        self.read = *read;
        let mut names: Vec<(RefKind, BString)> = refs
            .iter()
            .filter(|reference| matches!(reference.kind, RefKind::Branch | RefKind::RemoteBranch))
            .map(|reference| (reference.kind, reference.short_name().to_owned()))
            .collect();
        names.sort_by(|(one_kind, one), (other_kind, other)| {
            (*one_kind == RefKind::RemoteBranch)
                .cmp(&(*other_kind == RefKind::RemoteBranch))
                .then_with(|| natural(one.as_bstr(), other.as_bstr()))
        });
        self.names = names;
        self.kept_for = None;
    }

    /// Keeps the names the filter matches (all of them for none), the best first.
    fn filter(&mut self) {
        if self.kept_for.as_deref() == Some(self.query.as_str()) {
            return;
        }
        let query = self.query.trim();
        if query.is_empty() {
            self.kept = (0..self.names.len()).collect();
        } else {
            let mut scored: Vec<(u32, usize)> = self
                .names
                .iter()
                .enumerate()
                .filter_map(|(at, (_, name))| {
                    crate::commands::score(query, &name.to_str_lossy()).map(|score| (score, at))
                })
                .collect();
            // The best first; among equals, the order gathered (locals before the remotes').
            scored.sort_by(|one, other| other.0.cmp(&one.0).then(one.1.cmp(&other.1)));
            self.kept = scored.into_iter().map(|(_, at)| at).collect();
        }
        self.kept_for = Some(self.query.clone());
        self.picked = 0;
    }
}

/// Natural order: `v2.9` before `v2.10`, either case alike.
fn natural(one: &BStr, other: &BStr) -> std::cmp::Ordering {
    super::sidebar::natural(one, other)
}

/// The switcher's contents, inside its menu: the filter, the branches it keeps, and creating
/// one. `current` is the branch checked out (none when detached); `allowed` says why going
/// anywhere can't be done now (a task running, an operation in progress). Returns what was
/// asked, the menu to close.
pub(super) fn contents(
    ui: &mut Ui,
    (refs, current): (&Refs, Option<&BStr>),
    allowed: Result<(), &str>,
    switcher: &mut Switcher,
) -> Option<Switch> {
    let palette = Palette::of(ui.ctx());
    ui.set_width(340.0);
    switcher.gather(refs);
    let field = ui.add(
        egui::TextEdit::singleline(&mut switcher.query)
            .hint_text("Switch to a branch…")
            .desired_width(f32::INFINITY),
    );
    crate::help::note_topic(ui, field.rect, TOPIC);
    if !switcher.focused {
        field.request_focus();
        switcher.focused = true;
    }
    switcher.filter();
    let count = switcher.kept.len();
    // ↑ ↓ pick, Enter goes (the field keeps the keys it doesn't use).
    let (up, down, enter) = ui.input_mut(|input| {
        (
            input.consume_key(Modifiers::NONE, Key::ArrowUp),
            input.consume_key(Modifiers::NONE, Key::ArrowDown),
            input.consume_key(Modifiers::NONE, Key::Enter),
        )
    });
    if up {
        switcher.picked = switcher.picked.saturating_sub(1);
    }
    if down {
        switcher.picked = switcher
            .picked
            .saturating_add(1)
            .min(count.saturating_sub(1));
    }
    let mut asked = None;
    if count == 0 {
        ui.add_space(6.0);
        ui.label(widgets::muted(ui, "No branch holds those letters."));
    } else {
        let rows = (f32::from(u16::try_from(count).unwrap_or(u16::MAX))).min(SHOWN);
        egui::ScrollArea::vertical()
            .id_salt("branch-switcher-rows")
            .max_height(rows * ROW)
            .auto_shrink([false, true])
            .show_rows(ui, ROW, count, |ui, visible| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for place in visible {
                    let Some((kind, name)) = switcher
                        .kept
                        .get(place)
                        .and_then(|at| switcher.names.get(*at))
                    else {
                        continue;
                    };
                    let here = *kind == RefKind::Branch && Some(name.as_bstr()) == current;
                    let picked = place == switcher.picked;
                    if row(
                        ui,
                        (*kind, name.as_bstr()),
                        (here, picked),
                        &allowed,
                        palette,
                    ) || (picked && enter && !here)
                    {
                        asked = going(refs, *kind, name.as_bstr());
                    }
                }
            });
    }
    ui.separator();
    if super::actions::item(ui, (icon::GIT_BRANCH, "Create a branch here…"), Ok(())) {
        asked = Some(Switch::Create);
    }
    if allowed.is_err() && asked.as_ref().is_some_and(|asked| *asked != Switch::Create) {
        asked = None;
    }
    if asked.is_some() {
        switcher.query.clear();
        switcher.focused = false;
        ui.close();
    }
    asked
}

/// Where going to `name` (of `kind`) leads: a local branch; a remote's through the local
/// branch of its name, made to track it unless there is one.
fn going(refs: &Refs, kind: RefKind, name: &BStr) -> Option<Switch> {
    match kind {
        RefKind::Branch => Some(Switch::To(SwitchTo::Branch(name.to_owned()))),
        RefKind::RemoteBranch => {
            let local = name.split_once_str("/").map(|(_, local)| local)?;
            let local = BStr::new(local);
            let here = refs.0.iter().any(|reference| {
                reference.kind == RefKind::Branch && reference.short_name() == local
            });
            Some(Switch::To(if here {
                SwitchTo::Branch(local.to_owned())
            } else {
                SwitchTo::Track {
                    remote_branch: name.to_owned(),
                    local: local.to_owned(),
                }
            }))
        }
        RefKind::Tag | RefKind::Other => None,
    }
}

/// A branch's row: its icon, its name, and for the one checked out a mark; `picked` lit (↑ ↓).
/// Whether it was clicked.
fn row(
    ui: &mut Ui,
    (kind, name): (RefKind, &BStr),
    (here, picked): (bool, bool),
    allowed: &Result<(), &str>,
    palette: Palette,
) -> bool {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ROW),
        if here {
            egui::Sense::hover()
        } else {
            egui::Sense::click()
        },
    );
    let words = name.to_str_lossy();
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            !here && allowed.is_ok(),
            format!("Switch to {words}"),
        )
    });
    crate::help::note_topic(ui, rect, TOPIC);
    widgets::row_fill(
        ui,
        rect,
        crate::theme::RADIUS_INNER.into(),
        picked,
        response.hovered(),
    );
    let glyph = match kind {
        RefKind::RemoteBranch => icon::CLOUD,
        _ => icon::GIT_BRANCH,
    };
    let mut inside = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(egui::vec2(8.0, 0.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    inside.label(RichText::new(glyph).color(palette.muted));
    let text = if here {
        RichText::new(words.as_ref()).color(palette.ok)
    } else {
        RichText::new(words.as_ref()).color(palette.text)
    };
    inside.add(egui::Label::new(text).selectable(false).truncate());
    if here {
        inside.label(widgets::muted(&inside, "checked out"));
    }
    let response = match allowed {
        Err(why) if !here => response.on_hover_text(*why),
        _ => response,
    };
    response.clicked() && allowed.is_ok()
}

#[cfg(test)]
mod tests {
    use egit_git::ObjectId;

    use super::*;

    fn refs(names: &[(&str, RefKind)]) -> Refs {
        let target = ObjectId::from_hex(b"1111111111111111111111111111111111111111").unwrap();
        let all: Vec<Reference> = names
            .iter()
            .map(|(name, kind)| Reference {
                name: (*name).into(),
                kind: *kind,
                target,
            })
            .collect();
        (std::sync::Arc::from(all), 1)
    }

    #[test]
    fn branches_are_gathered_locals_first_and_matched_best_first() {
        let refs = refs(&[
            ("refs/remotes/origin/topic", RefKind::RemoteBranch),
            ("refs/heads/v2.10", RefKind::Branch),
            ("refs/heads/v2.9", RefKind::Branch),
            ("refs/tags/v1.0", RefKind::Tag),
            ("refs/heads/topic", RefKind::Branch),
        ]);
        let mut switcher = Switcher::default();
        switcher.gather(&refs);
        let names: Vec<&str> = switcher
            .names
            .iter()
            .map(|(_, name)| name.to_str().unwrap())
            .collect();
        assert_eq!(names, ["topic", "v2.9", "v2.10", "origin/topic"]);
        "top".clone_into(&mut switcher.query);
        switcher.filter();
        let kept: Vec<&str> = switcher
            .kept
            .iter()
            .map(|at| switcher.names[*at].1.to_str().unwrap())
            .collect();
        assert_eq!(kept, ["topic", "origin/topic"]);
    }

    #[test]
    fn a_remote_s_branch_is_tracked_or_its_local_one_switched_to() {
        let with_local = refs(&[
            ("refs/heads/topic", RefKind::Branch),
            ("refs/remotes/origin/topic", RefKind::RemoteBranch),
        ]);
        assert_eq!(
            going(&with_local, RefKind::RemoteBranch, "origin/topic".into()),
            Some(Switch::To(SwitchTo::Branch("topic".into())))
        );
        let alone = refs(&[("refs/remotes/origin/feature/x", RefKind::RemoteBranch)]);
        assert_eq!(
            going(&alone, RefKind::RemoteBranch, "origin/feature/x".into()),
            Some(Switch::To(SwitchTo::Track {
                remote_branch: "origin/feature/x".into(),
                local: "feature/x".into(),
            }))
        );
    }
}
