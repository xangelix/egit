//! A list of files as a tree of their folders (`file_tree`), drawn by egui-table-kit only for
//! the rows on screen: a folder's caret, icon, name, and how many files it holds, a click
//! folding or unfolding it; a file's change, name, and whatever its list adds at the right
//! (its lines, a button). A folder's whole path is on hover, and folds are kept by path, so
//! they hold when the list is read again.
//!
//! A tree is made again only when its list is replaced, in a job past 5,000 files; the last
//! one keeps showing meanwhile.

use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::Arc;

use bstr::ByteSlice as _;
use egui::{Align, Color32, Label, Layout, Response, RichText, Ui};
use egui_async::Bind;
use egui_table_kit::error::TableError;
use egui_table_kit::interaction::RowEventKind;
use egui_table_kit::operations::{HeaderIter, Row, RowCallback, TableCell, TableProvider};
use egui_table_kit::state::TableState;
use egui_table_kit::table::TableKit;

use crate::file_tree::{FileTree, Node};
use crate::format::count;
use crate::problem::Problem;
use crate::theme::{Palette, icon};

/// Where a tree's files come from: shared, so a job can make the tree.
pub(crate) trait TreeSource: Send + Sync + 'static {
    /// How many files.
    fn files(&self) -> usize;
    /// File `entry`'s path (`/` between folders).
    fn path(&self, entry: usize) -> &[u8];
    /// How it changed, in a word ("Modified"), as its list names it.
    fn change(&self, entry: usize) -> &'static str;
}

/// More files than this are arranged in a job.
const IN_PLACE: usize = 5_000;

/// How far each level is indented: a caret's width.
const INDENT: f32 = 14.0;

/// A row, for what its list adds at its right.
pub(crate) enum TreeRow<'t> {
    /// A folder: its node in the tree, whose files `FileTree::files_in` gives.
    Folder(&'t FileTree, u32),
    /// A file: its entry.
    File(usize),
}

/// What the tree was asked.
pub(crate) struct TreeShown<E> {
    /// A file opened (its entry).
    pub(crate) opened: Option<usize>,
    /// What a list's own part of a row asked.
    pub(crate) asked: Option<E>,
}

/// A list of files shown as a tree, between frames.
#[derive(Debug)]
pub(crate) struct TreeTable<S> {
    id: String,
    /// The tree shown, and the list it was made of.
    shown: Option<(Arc<S>, Arc<FileTree>)>,
    /// A tree of a newer list, being made.
    making: Bind<(Arc<S>, Arc<FileTree>), Problem>,
    /// The list asked for last.
    wanted: Option<Arc<S>>,
    /// The folders folded, by their whole paths.
    folded: HashSet<Vec<u8>>,
    /// The rows shown: nodes of the tree.
    rows: Vec<u32>,
    /// Whether the rows must be worked out again (a new tree, a fold).
    stale: bool,
    state: TableState,
}

impl<S: TreeSource> TreeTable<S> {
    pub(crate) fn new(id: String) -> Self {
        let state = TableState::new(id.as_str(), 0);
        Self {
            id,
            shown: None,
            making: Bind::new(true),
            wanted: None,
            folded: HashSet::new(),
            rows: Vec::new(),
            stale: false,
            state,
        }
    }

    /// Makes the tree of `source` unless it is the one wanted already; takes in one made.
    fn sync(&mut self, source: &Arc<S>) {
        if self.making.is_finished()
            && let Some(Ok(made)) = self.making.take()
            && self
                .wanted
                .as_ref()
                .is_some_and(|wanted| Arc::ptr_eq(wanted, &made.0))
        {
            self.shown = Some(made);
            self.stale = true;
        }
        if self
            .wanted
            .as_ref()
            .is_some_and(|wanted| Arc::ptr_eq(wanted, source))
        {
            return;
        }
        self.wanted = Some(Arc::clone(source));
        if source.files() <= IN_PLACE {
            let tree = FileTree::new(source.files(), |entry| source.path(entry));
            self.shown = Some((Arc::clone(source), Arc::new(tree)));
            self.making = Bind::new(true);
            self.stale = true;
        } else {
            let source = Arc::clone(source);
            self.making.request(crate::jobs::blocking(move || {
                let tree = FileTree::new(source.files(), |entry| source.path(entry));
                Ok((source, Arc::new(tree)))
            }));
        }
    }

    /// Draws `source`'s files as a tree, `height` tall at most; `trailing` draws each row's
    /// end (a file's lines, a button) and may ask something.
    pub(crate) fn show<E>(
        &mut self,
        ui: &mut Ui,
        source: &Arc<S>,
        height: f32,
        mut trailing: impl FnMut(&mut Ui, &S, TreeRow<'_>, Color32) -> Option<E>,
    ) -> TreeShown<E> {
        let mut shown = TreeShown {
            opened: None,
            asked: None,
        };
        self.sync(source);
        let Some((files, tree)) = self.shown.clone() else {
            arranging(ui, source.files());
            return shown;
        };
        if self.stale {
            self.stale = false;
            tree.visible(|entry| files.path(entry), &self.folded, &mut self.rows);
            self.state.active_rows = (0..self.rows.len()).collect();
            self.state.selected_rows.clear();
        }
        let palette = Palette::of(ui.ctx());
        let provider = TreeRows {
            files: &*files,
            tree: &tree,
            rows: &self.rows,
        };
        let (rows, folded) = (&self.rows, &self.folded);
        let table = TableKit::new(self.id.as_str(), &provider, &mut self.state);
        // A scroll area of its own: the list's place in the same spot means nothing here.
        let output = ui
            .push_id(("tree", self.id.as_str()), |ui| {
                crate::tables::tree_list(table, ui)
                    .with_max_height(Some(height))
                    .show_with_output(ui, |ui, cell, _, text_color| {
                        let index = *rows.get(usize::try_from(cell.row_nr).ok()?)?;
                        let node = tree.node(index)?;
                        let place = Place {
                            files: &*files,
                            tree: &tree,
                            index,
                            node,
                        };
                        let is_folded = node.is_folder()
                            && folded.contains(node.folder_path(files.path(entry_of(node))));
                        let (response, asked) =
                            cell_ui(ui, &place, is_folded, text_color, palette, &mut trailing);
                        if asked.is_some() {
                            shown.asked = asked;
                        }
                        Some(response)
                    })
            })
            .inner;
        match output {
            Ok(output) => {
                let clicked = output.events.iter().find(|event| {
                    matches!(event.kind, RowEventKind::Clicked | RowEventKind::Activated)
                });
                if let Some(clicked) = clicked
                    && let Some(&index) = self.rows.get(clicked.row_index)
                    && let Some(node) = tree.node(index)
                {
                    if node.is_folder() {
                        let path = node.folder_path(files.path(entry_of(node))).to_vec();
                        if !self.folded.remove(&path) {
                            self.folded.insert(path);
                        }
                        self.stale = true;
                    } else {
                        shown.opened = Some(entry_of(node));
                    }
                }
            }
            Err(error) => {
                ui.label(
                    RichText::new(format!("The list couldn't be drawn: {error}"))
                        .color(palette.danger),
                );
            }
        }
        shown
    }
}

/// A node's entry, as an index.
fn entry_of(node: &Node) -> usize {
    usize::try_from(node.entry).unwrap_or(usize::MAX)
}

/// A row being drawn: the tree, its node, and the files it was made of.
struct Place<'a, S> {
    files: &'a S,
    tree: &'a FileTree,
    index: u32,
    node: &'a Node,
}

/// Says the tree is being made, in a row of the list's size.
fn arranging(ui: &mut Ui, files: usize) {
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), crate::tables::ROW_HEIGHT),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.add_space(crate::tables::CELL_INSET);
            ui.spinner();
            ui.label(crate::widgets::muted(
                ui,
                format!(
                    "Arranging {} files in their folders…",
                    count(u64::try_from(files).unwrap_or(u64::MAX))
                ),
            ));
        },
    );
}

/// A row: indented by its depth, a folder's caret, icon, and name, or a file's change and
/// name; at its right what `trailing` adds, and a folder's count of files.
fn cell_ui<S: TreeSource, E>(
    ui: &mut Ui,
    place: &Place<'_, S>,
    folded: bool,
    text_color: Color32,
    palette: Palette,
    trailing: &mut impl FnMut(&mut Ui, &S, TreeRow<'_>, Color32) -> Option<E>,
) -> (Response, Option<E>) {
    let Place {
        files,
        tree,
        index,
        node,
    } = *place;
    let path = files.path(entry_of(node));
    let name = node.name(path).to_str_lossy();
    let label = |text: Cow<'_, str>, color| {
        Label::new(RichText::new(text).color(color))
            .selectable(false)
            .truncate()
    };
    let mut asked = None;
    let room = ui.available_size();
    let response = ui
        .allocate_ui_with_layout(room, Layout::right_to_left(Align::Center), |ui| {
            let row = if node.is_folder() {
                TreeRow::Folder(tree, index)
            } else {
                TreeRow::File(entry_of(node))
            };
            asked = trailing(ui, files, row, text_color);
            if node.is_folder() {
                ui.add(
                    Label::new(
                        RichText::new(count(u64::from(node.files)))
                            .small()
                            .color(palette.muted),
                    )
                    .selectable(false),
                );
            }
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.add_space(INDENT * f32::from(node.depth));
                if node.is_folder() {
                    let caret = if folded {
                        icon::CARET_RIGHT
                    } else {
                        icon::CARET_DOWN
                    };
                    let glyphs = format!("{caret} {}", icon::FOLDER_SIMPLE);
                    ui.add(label(Cow::Owned(glyphs), palette.muted));
                    ui.add(label(name, text_color))
                        .on_hover_text(node.folder_path(path).to_str_lossy());
                } else {
                    crate::tables::status_mark(ui, files.change(entry_of(node)), palette);
                    let shown = ui.add(label(name, text_color));
                    if node.depth > 0 {
                        shown.on_hover_text(path.to_str_lossy());
                    }
                }
            });
        })
        .response;
    (response, asked)
}

/// A tree's rows, for egui-table-kit.
struct TreeRows<'a, S> {
    files: &'a S,
    tree: &'a FileTree,
    rows: &'a [u32],
}

impl<S: TreeSource> TreeRows<'_, S> {
    /// Row `row`'s words: a folder's path and how many files it holds, a file's path.
    fn text(&self, row: usize) -> Option<Cow<'_, str>> {
        let node = self.tree.node(*self.rows.get(row)?)?;
        let path = self.files.path(entry_of(node));
        Some(if node.is_folder() {
            Cow::Owned(format!(
                "{}/, {} {}",
                node.folder_path(path).to_str_lossy(),
                count(u64::from(node.files)),
                if node.files == 1 { "file" } else { "files" }
            ))
        } else {
            path.to_str_lossy()
        })
    }
}

/// One row of a tree.
struct TreeRowCells<'a>(Cow<'a, str>);

impl Row for TreeRowCells<'_> {
    fn cell(&self, col_index: usize) -> Option<TableCell<'_>> {
        (col_index == 0).then(|| (Cow::Borrowed(self.0.as_ref()), None))
    }

    fn column_count(&self) -> usize {
        1
    }
}

impl<S: TreeSource> TableProvider for TreeRows<'_, S> {
    fn column_count(&self) -> usize {
        1
    }

    fn header(&self, index: usize) -> Option<Cow<'_, str>> {
        (index == 0).then_some(Cow::Borrowed("File"))
    }

    fn headers(&self) -> HeaderIter<'_> {
        HeaderIter::new(self)
    }

    fn row_count(&self) -> usize {
        self.rows.len()
    }

    fn cell_at(
        &self,
        row_index: usize,
        col_index: usize,
    ) -> Result<Option<TableCell<'_>>, TableError> {
        Ok((col_index == 0)
            .then(|| self.text(row_index))
            .flatten()
            .map(|text| (text, None)))
    }

    fn for_row_at(&self, index: usize, f: &mut RowCallback<'_>) -> Result<(), TableError> {
        self.text(index)
            .map_or(Ok(()), |text| f(&TreeRowCells(text)))
    }

    fn for_selected_rows(
        &self,
        state: &TableState,
        f: &mut RowCallback<'_>,
    ) -> Result<(), TableError> {
        for index in &state.selected_rows {
            self.for_row_at(usize::try_from(index).unwrap_or(usize::MAX), f)?;
        }
        Ok(())
    }

    fn for_all_rows(&self, f: &mut RowCallback<'_>) -> Result<(), TableError> {
        for row in 0..self.rows.len() {
            self.for_row_at(row, f)?;
        }
        Ok(())
    }
}
