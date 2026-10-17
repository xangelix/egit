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
                let title = self
                    .state
                    .repos
                    .iter()
                    .find(|repo| repo.id == *id)
                    .map_or("Repository", |repo| repo.title.as_str());
                format!("{}  {title}", icon::GIT_BRANCH).into()
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
        match tiles.get_pane(&tile_id) {
            Some(Pane::Repo(id)) => match self.state.repos.iter().find(|repo| repo.id == *id) {
                Some(repo) => {
                    button_response.on_hover_text(repo.repository.work_tree().display().to_string())
                }
                None => button_response,
            },
            _ => button_response,
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

    fn tab_bar_color(&self, visuals: &egui::Visuals) -> Color32 {
        palette(visuals).panel
    }

    fn tab_bg_color(
        &self,
        visuals: &egui::Visuals,
        _tiles: &Tiles<Pane>,
        _tile_id: TileId,
        state: &egui_tiles::TabState,
    ) -> Color32 {
        let palette = palette(visuals);
        if state.active {
            palette.background
        } else {
            palette.panel
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
