//! The tabs across the window: Home, the profile, and one per open repository, laid out by
//! `egui_tiles` (tabs can be dragged beside each other).

use egui::{Color32, Ui, WidgetText};
use egui_tiles::{Container, SimplificationOptions, Tile, TileId, Tiles, Tree, UiResponse};

use crate::app::{Action, AppState};
use crate::repo::RepoId;
use crate::theme::{Palette, icon};
use crate::views;

/// What a tab shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pane {
    /// Opening repositories, recent ones, and who you are.
    Home,
    /// The git profile.
    Profile,
    /// egit's own preferences, and the keyboard shortcuts.
    Preferences,
    /// An open repository.
    Repo(RepoId),
}

/// The layout egit starts with: the Home tab.
pub(crate) fn initial() -> Tree<Pane> {
    Tree::new_tabs("egit-tabs", vec![Pane::Home])
}

/// Brings `pane`'s tab to the front, adding it beside the others if it isn't there.
pub(crate) fn show(tree: &mut Tree<Pane>, pane: Pane) {
    if tree.make_active(|_, tile| matches!(tile, Tile::Pane(shown) if *shown == pane)) {
        return;
    }
    let added = tree.tiles.insert_pane(pane);
    let tabs = tabs_container(tree);
    if let Some(Tile::Container(Container::Tabs(tabs))) = tabs.and_then(|id| tree.tiles.get_mut(id))
    {
        tabs.add_child(added);
        tabs.set_active(added);
    } else {
        // Every tab was dragged into a split: the new one joins a new set of tabs around the
        // old layout.
        let children = tree.root().into_iter().chain([added]).collect();
        let root = tree.tiles.insert_tab_tile(children);
        tree.root = Some(root);
        if let Some(Tile::Container(Container::Tabs(tabs))) = tree.tiles.get_mut(root) {
            tabs.set_active(added);
        }
    }
}

/// Adds `pane` to the set of tabs new tabs join, before the first tab `goes_after` names (else
/// last); brought to the front where `front` asks, so long as Home is there (nothing else was
/// chosen meanwhile). Nothing happens to a pane there already, but that.
pub(crate) fn insert(
    tree: &mut Tree<Pane>,
    pane: Pane,
    goes_after: impl Fn(Pane) -> bool,
    front: bool,
) {
    let front = front && self::front(tree) == Some(Pane::Home);
    if let Some(there) = tree.tiles.find_pane(&pane) {
        if front {
            tree.make_active(|id, _| id == there);
        }
        return;
    }
    let Some(tabs) = tabs_container(tree) else {
        show(tree, pane);
        return;
    };
    let added = tree.tiles.insert_pane(pane);
    let at = match tree.tiles.get(tabs) {
        Some(Tile::Container(Container::Tabs(container))) => {
            container.children.iter().position(|child| {
                tree.tiles
                    .get_pane(child)
                    .is_some_and(|other| goes_after(*other))
            })
        }
        _ => None,
    };
    if let Some(Tile::Container(Container::Tabs(container))) = tree.tiles.get_mut(tabs) {
        match at {
            Some(at) => container.children.insert(at, added),
            None => container.children.push(added),
        }
        if front {
            container.set_active(added);
        }
    }
}

/// For a tab reopened from the last close at `place` among them (`places`: those reopened so
/// far), the tabs it goes before: those reopened from later places, and those opened since
/// (Profile, Preferences, a repository opened this run); never Home.
pub(crate) fn after_restored(
    places: &std::collections::HashMap<RepoId, usize>,
    place: usize,
) -> impl Fn(Pane) -> bool + '_ {
    move |pane| match pane {
        Pane::Repo(other) => places.get(&other).is_none_or(|other| *other > place),
        Pane::Profile | Pane::Preferences => true,
        Pane::Home => false,
    }
}

/// Every pane, in the order the tabs show them (a split's from its first part on).
pub(crate) fn in_order(tree: &Tree<Pane>) -> Vec<Pane> {
    let mut panes = Vec::new();
    let mut pending: Vec<TileId> = tree.root().into_iter().collect();
    // Depth first, each container's children in their order.
    while let Some(id) = pending.pop() {
        match tree.tiles.get(id) {
            Some(Tile::Pane(pane)) => panes.push(*pane),
            Some(Tile::Container(container)) => {
                // Popped from the end: the first child last in.
                let start = pending.len();
                pending.extend(container.children().copied());
                if let Some(children) = pending.get_mut(start..) {
                    children.reverse();
                }
            }
            None => {}
        }
    }
    panes
}

/// The pane in front: of those showing, the first repository's, else the first.
pub(crate) fn front(tree: &Tree<Pane>) -> Option<Pane> {
    let shown: Vec<Pane> = tree
        .active_tiles()
        .into_iter()
        .filter_map(|id| tree.tiles.get_pane(&id).copied())
        .collect();
    shown
        .iter()
        .find(|pane| matches!(pane, Pane::Repo(_)))
        .or_else(|| shown.first())
        .copied()
}

/// Closes the tab in front (never Home's); returns the pane closed.
pub(crate) fn close_front(tree: &mut Tree<Pane>) -> Option<Pane> {
    let pane = front(tree).filter(|pane| *pane != Pane::Home)?;
    let id = tree.tiles.find_pane(&pane)?;
    tree.remove_recursively(id);
    Some(pane)
}

/// Which tabs to close.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Closing {
    /// This one.
    This(Pane),
    /// Every one but this (and Home).
    Others(Pane),
}

/// Closes the tabs `closing` names (never Home's); returns the panes closed.
pub(crate) fn close(tree: &mut Tree<Pane>, closing: Closing) -> Vec<Pane> {
    let closed: Vec<Pane> = match closing {
        Closing::This(pane) => vec![pane],
        Closing::Others(kept) => tree
            .tiles
            .tiles()
            .filter_map(|tile| match tile {
                Tile::Pane(pane) if *pane != kept => Some(*pane),
                _ => None,
            })
            .collect(),
    };
    closed
        .into_iter()
        .filter(|pane| *pane != Pane::Home)
        .filter(|pane| {
            let found = tree.tiles.find_pane(pane);
            if let Some(id) = found {
                tree.remove_recursively(id);
            }
            found.is_some()
        })
        .collect()
}

/// Brings the `n`th tab (from 1) of the main set of tabs to the front, if there is one.
pub(crate) fn show_nth(tree: &mut Tree<Pane>, n: usize) {
    let Some(tabs) = tabs_container(tree) else {
        return;
    };
    if let Some(Tile::Container(Container::Tabs(tabs))) = tree.tiles.get_mut(tabs)
        && let Some(child) = n
            .checked_sub(1)
            .and_then(|at| tabs.children.get(at))
            .copied()
    {
        tabs.set_active(child);
    }
}

/// The set of tabs new tabs join: the root's, or else the one holding Home.
fn tabs_container(tree: &Tree<Pane>) -> Option<TileId> {
    let root = tree.root()?;
    if matches!(
        tree.tiles.get(root),
        Some(Tile::Container(Container::Tabs(_)))
    ) {
        return Some(root);
    }
    let home = tree.tiles.find_pane(&Pane::Home)?;
    let parent = tree.tiles.parent_of(home)?;
    matches!(
        tree.tiles.get(parent),
        Some(Tile::Container(Container::Tabs(_)))
    )
    .then_some(parent)
}

/// Draws the tabs and what the visible ones show.
pub(crate) fn draw(tree: &mut Tree<Pane>, state: &mut AppState, ui: &mut Ui) {
    let mut behavior = Behavior { state };
    tree.ui(&mut behavior, ui);
}

struct Behavior<'a> {
    state: &'a mut AppState,
}

impl egui_tiles::Behavior<Pane> for Behavior<'_> {
    fn pane_ui(&mut self, ui: &mut Ui, _tile_id: TileId, pane: &mut Pane) -> UiResponse {
        match *pane {
            Pane::Home => views::home::show(ui, self.state),
            Pane::Profile => views::profile::show(ui, self.state),
            Pane::Preferences => views::preferences::show(ui, self.state),
            Pane::Repo(id) => views::repo::show(ui, self.state, id),
        }
        UiResponse::None
    }

    fn tab_title_for_pane(&mut self, pane: &Pane) -> WidgetText {
        match pane {
            Pane::Home => format!("{}  Home", icon::HOUSE).into(),
            Pane::Profile => format!("{}  Profile", icon::USER_CIRCLE).into(),
            Pane::Preferences => format!("{}  Preferences", icon::GEAR_SIX).into(),
            Pane::Repo(id) => {
                let repo = self.state.repos.iter().find(|repo| repo.id == *id);
                let title = repo.map_or("Repository", |repo| repo.title.as_str());
                // On another computer: a globe, beside its name.
                let glyph = if repo.is_some_and(|repo| repo.git.host().ssh().is_some()) {
                    icon::GLOBE
                } else {
                    icon::GIT_BRANCH
                };
                format!("{glyph}  {title}").into()
            }
        }
    }

    fn on_tab_button(
        &mut self,
        tiles: &mut Tiles<Pane>,
        tile_id: TileId,
        button_response: egui::Response,
    ) -> egui::Response {
        crate::widgets::tab_hover(&button_response);
        let Some(pane) = tiles.get_pane(&tile_id).copied() else {
            return button_response;
        };
        // A middle click closes it, as a browser's tab.
        if pane != Pane::Home && button_response.middle_clicked() {
            self.state
                .actions
                .push(Action::CloseTabs(Closing::This(pane)));
        }
        button_response.context_menu(|ui| self.tab_menu(ui, pane));
        match pane {
            Pane::Repo(id) => match self.state.repos.iter().find(|repo| repo.id == id) {
                Some(repo) => {
                    let path = repo.repository.work_tree().display().to_string();
                    // Its agents wanting the user, marked on its icon (seen from any tab).
                    let attention = repo.agents.attention();
                    if !attention.any() {
                        return button_response.on_hover_text(path);
                    }
                    let palette = Palette::of(&button_response.ctx);
                    let color = if attention.waiting > 0 {
                        palette.warn
                    } else {
                        palette.accent
                    };
                    let tab = button_response.rect;
                    let icon = egui::Rect::from_min_size(
                        egui::pos2(tab.left() + 6.0, tab.center().y - 12.0),
                        egui::vec2(20.0, 20.0),
                    );
                    let painter = button_response.ctx.layer_painter(button_response.layer_id);
                    crate::widgets::attention_dot(&painter, icon, color);
                    button_response.on_hover_text(format!("{path}\n{}", attention.words()))
                }
                None => button_response,
            },
            _ => button_response,
        }
    }

    /// A "+" after the tabs: Home, where repositories are opened, cloned, and made.
    fn top_bar_right_ui(
        &mut self,
        _tiles: &Tiles<Pane>,
        ui: &mut Ui,
        _tile_id: TileId,
        _tabs: &egui_tiles::Tabs,
        _scroll_offset: &mut f32,
    ) {
        ui.add_space(4.0);
        if crate::widgets::icon_tool(ui, true, icon::PLUS, "Open or clone a repository")
            .on_hover_text("Home: open, clone, or create a repository")
            .clicked()
        {
            self.state.actions.push(Action::ShowHome);
        }
    }

    fn close_button_outer_size(&self) -> f32 {
        // Big enough to hit (24 points); the cross drawn as large as egui_tiles draws it.
        24.0
    }

    fn close_button_inner_margin(&self) -> f32 {
        7.0
    }

    fn is_tab_closable(&self, tiles: &Tiles<Pane>, tile_id: TileId) -> bool {
        !matches!(tiles.get_pane(&tile_id), Some(Pane::Home))
    }

    fn on_tab_close(&mut self, tiles: &mut Tiles<Pane>, tile_id: TileId) -> bool {
        if let Some(Pane::Repo(id)) = tiles.get_pane(&tile_id) {
            self.state.actions.push(Action::Closed(*id));
        }
        true
    }

    fn simplification_options(&self) -> SimplificationOptions {
        SimplificationOptions {
            all_panes_must_have_tabs: true,
            ..SimplificationOptions::default()
        }
    }

    fn tab_bar_height(&self, _style: &egui::Style) -> f32 {
        34.0
    }

    fn gap_width(&self, _style: &egui::Style) -> f32 {
        2.0
    }

    /// Deeper than the chrome below: the selected tab rises out of it.
    fn tab_bar_color(&self, visuals: &egui::Visuals) -> Color32 {
        palette(visuals).recess
    }

    fn tab_bg_color(
        &self,
        visuals: &egui::Visuals,
        _tiles: &Tiles<Pane>,
        _tile_id: TileId,
        state: &egui_tiles::TabState,
    ) -> Color32 {
        // The selected tab in the tone of what it shows (the history's chrome, a page), the
        // others sunk into the bar.
        let palette = palette(visuals);
        if state.active {
            palette.panel
        } else {
            palette.recess
        }
    }
}

impl Behavior<'_> {
    /// A tab's menu: close it or the others; a repository's path copied, its folder opened.
    fn tab_menu(&mut self, ui: &mut Ui, pane: Pane) {
        let actions = &mut self.state.actions;
        // Home stays open.
        if pane != Pane::Home && views::actions::shortcut_item(ui, (icon::X, "Close the tab"), "⌘W")
        {
            actions.push(Action::CloseTabs(Closing::This(pane)));
        }
        if views::actions::item(ui, (icon::X_SQUARE, "Close the other tabs"), Ok(())) {
            actions.push(Action::CloseTabs(Closing::Others(pane)));
        }
        let Pane::Repo(id) = pane else {
            return;
        };
        let Some(repo) = self.state.repos.iter().find(|repo| repo.id == id) else {
            return;
        };
        ui.separator();
        let path = repo.repository.work_tree().to_path_buf();
        if views::actions::item(ui, (icon::COPY, "Copy its path"), Ok(())) {
            ui.ctx().copy_text(path.display().to_string());
        }
        // Its folder opens here only: one on another computer has none here to open.
        if repo.git.host().ssh().is_none()
            && views::actions::item(ui, (icon::FOLDER_OPEN, "Open its folder"), Ok(()))
        {
            actions.push(Action::Reveal(path, crate::desktop::Reveal::Open));
        }
    }
}

const fn palette(visuals: &egui::Visuals) -> Palette {
    if visuals.dark_mode {
        Palette::DARK
    } else {
        Palette::LIGHT
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    /// The tabs' panes in order, as names: `H` Home, `P` Preferences, a repository its number.
    fn names(tree: &Tree<Pane>) -> Vec<String> {
        in_order(tree)
            .into_iter()
            .map(|pane| match pane {
                Pane::Home => "H".to_owned(),
                Pane::Profile => "Pr".to_owned(),
                Pane::Preferences => "P".to_owned(),
                Pane::Repo(id) => id.to_string(),
            })
            .collect()
    }

    #[test]
    fn reopened_tabs_take_their_places_whichever_lands_first() {
        // Places 0..4 saved; repository ids 10.. stand for them, landing scrambled, with
        // Preferences and a repository of this run (99) opened meanwhile.
        for landing in [[3, 0, 2, 1], [0, 1, 2, 3], [3, 2, 1, 0], [1, 3, 0, 2]] {
            let mut tree = initial();
            let mut places = HashMap::new();
            for (step, place) in landing.into_iter().enumerate() {
                if step == 1 {
                    show(&mut tree, Pane::Preferences);
                    show(&mut tree, Pane::Repo(99));
                    show(&mut tree, Pane::Home);
                }
                let id = 10 + RepoId::try_from(place).unwrap();
                places.insert(id, place);
                insert(
                    &mut tree,
                    Pane::Repo(id),
                    after_restored(&places, place),
                    place == 2,
                );
            }
            assert_eq!(
                names(&tree),
                ["H", "10", "11", "12", "13", "P", "99"],
                "{landing:?}"
            );
            // Home was in front when the one in front landed: it is now.
            assert_eq!(front(&tree), Some(Pane::Repo(12)), "{landing:?}");
        }
    }

    #[test]
    fn the_one_in_front_takes_the_front_only_from_home() {
        let mut tree = initial();
        let mut places = HashMap::new();
        show(&mut tree, Pane::Repo(99));
        places.insert(10, 0);
        insert(&mut tree, Pane::Repo(10), after_restored(&places, 0), true);
        // Opened since and in front: it stays so.
        assert_eq!(front(&tree), Some(Pane::Repo(99)));
        assert_eq!(names(&tree), ["H", "10", "99"]);
    }
}
