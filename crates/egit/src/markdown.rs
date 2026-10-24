//! Markdown previewed, as a `.md` file's change reads once rendered: each side's text read off
//! the interface thread (a blob, or the work tree's file; at most [`LARGEST`]) and split there
//! into its blocks, then drawn by `egui_commonmark` (pulldown-cmark) in a scroll area of its own,
//! in egit's look, a block at a time and only those in view, so a long document costs what a
//! screen shows.
//!
//! A block is a top-level element (a paragraph, a heading, a code block, a table), or one item
//! of a top-level list (an ordered one's keeping its number); each is drawn as a document of
//! its own, with the link reference definitions it may use. Its height is guessed from its
//! lines until it is drawn (or measured ahead of the view, a little each frame), and the view
//! stays where it is as guesses turn into heights above it. Links go through egit's opener,
//! which opens web pages only; one to a heading (`#install`) scrolls to it, as GitHub names
//! headings. Nothing is fetched from the web: an image there is a link to it (a picture in a
//! document can tell its author who read it).
//!
//! Before and after are compared block by block ([`egit_text::kept`]): the blocks that changed
//! are tinted (removed, added; the change in view stronger), a line marks where the other side
//! has blocks this one hasn't, and each side scrolls to a change as it is stepped to, the first
//! as the documents land.

use std::ops::Range;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egit_git::{Git, Repository};
use egui::{Color32, Rect, ScrollArea, TextStyle, Ui, UiBuilder, pos2, vec2};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

use crate::history::Objects;
use crate::jobs;
use crate::pictures::{PictureSource, read_side};
use crate::problem::Problem;
use crate::theme::{Palette, icon};

/// The largest document previewed, in bytes.
pub(crate) const LARGEST: u64 = 1 << 20;

/// A document's text sizes: its body, and its largest heading (the others step down to the
/// body's).
const BODY: f32 = 14.0;
const HEADING: f32 = 24.0;

/// How long a frame may spend measuring blocks out of view, so the scroll bar settles.
const MEASURING: Duration = Duration::from_millis(2);

/// The room left of a compared document's blocks, where a changed one's bar goes.
const GUTTER: f32 = 10.0;
/// A changed block's bar's width.
const BAR: f32 = 3.0;
/// How far a changed block's tint reaches above and below its text.
const TINT: f32 = 3.0;

/// Whether `path` names a Markdown file (by its extension, in any case).
pub(crate) fn is_markdown(path: &[u8]) -> bool {
    let name = path.rsplit(|&byte| byte == b'/').next().unwrap_or(path);
    let Some(dot) = name.iter().rposition(|&byte| byte == b'.') else {
        return false;
    };
    let extension = name.get(dot.saturating_add(1)..).unwrap_or_default();
    ["md", "markdown", "mdown", "mkd", "mkdn", "mdwn"]
        .iter()
        .any(|known| extension.eq_ignore_ascii_case(known.as_bytes()))
}

/// A document, ready to draw: its text (made ready: [`prepared`]), its blocks, the link
/// reference definitions they may use, its headings' anchors, and its size as read; compared
/// with the other side of its change, which side it is and where that has blocks it hasn't.
#[derive(Debug)]
pub(crate) struct Document {
    text: String,
    blocks: Vec<Block>,
    references: String,
    /// Each heading's anchor (as GitHub names it) and its block.
    anchors: Vec<(String, usize)>,
    pub(crate) bytes: u64,
    sided: Sided,
    /// Where the other side has blocks this one hasn't: the block they would come before (or
    /// the count of blocks, at the end), and their change; in order.
    gaps: Vec<(usize, usize)>,
}

/// Which side of a change a document is, for the colours its changes take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sided {
    /// Not compared: a document added, or deleted.
    Alone,
    Before,
    After,
}

/// A block of a document: its place in the text, its lines (for a guess at its height), the
/// room before it, and the change it is part of, if it changed. An ordered list's item is
/// written again with its number, as it is drawn alone.
#[derive(Debug)]
struct Block {
    range: Range<usize>,
    numbered: Option<String>,
    lines: u32,
    room: Room,
    change: Option<usize>,
}

/// The room before a block, as `egui_commonmark` leaves it between elements drawn together.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Room {
    /// The document's first.
    None,
    /// A list's item after another.
    Item,
    /// Anything else.
    Block,
    /// A heading.
    Heading,
}

impl Room {
    /// Its height, in lines of the body's text.
    const fn lines(self) -> f32 {
        match self {
            Self::None => 0.0,
            Self::Item => 0.15,
            Self::Block => 0.6,
            Self::Heading => 1.0,
        }
    }
}

impl Document {
    /// `text` made ready to draw and split into its blocks; `bytes` as read.
    pub(crate) fn new(text: &str, bytes: u64) -> Self {
        let text = prepared(text);
        let (blocks, references, anchors) = split(&text);
        Self {
            text,
            blocks,
            references,
            anchors,
            bytes,
            sided: Sided::Alone,
            gaps: Vec::new(),
        }
    }

    /// Block `block`'s own text, as it reads (an ordered list's item with its number).
    fn source<'a>(&'a self, block: &'a Block) -> &'a str {
        block
            .numbered
            .as_deref()
            .unwrap_or_else(|| self.text.get(block.range.clone()).unwrap_or_default())
    }

    /// Block `index`'s text, as it is drawn.
    fn block_text(&self, index: usize) -> Option<String> {
        let block = self.blocks.get(index)?;
        let mut text = block.numbered.clone().unwrap_or_else(|| {
            self.text
                .get(block.range.clone())
                .unwrap_or_default()
                .to_owned()
        });
        // The definitions a reference-style link names, written after it.
        if !self.references.is_empty() && text.contains(']') {
            text.push_str("\n\n");
            text.push_str(&self.references);
        }
        Some(text)
    }

    /// How many blocks it has.
    #[cfg(test)]
    const fn blocks(&self) -> usize {
        self.blocks.len()
    }

    /// The room before block `index`, in lines of the body's text.
    fn room(&self, index: usize) -> f32 {
        self.blocks
            .get(index)
            .map_or(0.0, |block| block.room.lines())
    }
}

/// One side of a change: none (the document was added, or deleted), the document, or why it
/// couldn't be read.
pub(crate) type Side = Option<Result<Document, String>>;

/// Both sides of a document's change, and how they differ.
#[derive(Debug)]
pub(crate) struct Documents {
    pub(crate) before: Side,
    pub(crate) after: Side,
    /// The blocks that changed, in order; none unless both sides were read.
    pub(crate) changes: Vec<Change>,
}

/// Blocks that changed together: those removed (of the side before) and added (of the side
/// after); either may be none, where the other side alone has blocks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Change {
    pub(crate) before: Range<usize>,
    pub(crate) after: Range<usize>,
}

/// Compares two documents block by block: marks each block that changed with its change,
/// notes where the other side has blocks one hasn't, and returns the changes.
fn compare(before: &mut Document, after: &mut Document) -> Vec<Change> {
    let (old_kept, new_kept) = {
        let old: Vec<&str> = before
            .blocks
            .iter()
            .map(|block| before.source(block))
            .collect();
        let new: Vec<&str> = after
            .blocks
            .iter()
            .map(|block| after.source(block))
            .collect();
        egit_text::kept(&old, &new)
    };
    let mut changes = Vec::new();
    let (mut old, mut new) = (0, 0);
    while old < old_kept.len() || new < new_kept.len() {
        let unkept = |kept: &[bool], at: usize| kept.get(at).is_some_and(|kept| !kept);
        if !unkept(&old_kept, old) && !unkept(&new_kept, new) {
            // Both kept: the same block.
            old = old.saturating_add(1);
            new = new.saturating_add(1);
            continue;
        }
        let run = |kept: &[bool], from: usize| {
            from..kept
                .iter()
                .skip(from)
                .position(|kept| *kept)
                .map_or(kept.len(), |kept| from.saturating_add(kept))
        };
        let change = Change {
            before: run(&old_kept, old),
            after: run(&new_kept, new),
        };
        (old, new) = (change.before.end, change.after.end);
        changes.push(change);
    }
    before.sided = Sided::Before;
    after.sided = Sided::After;
    for (index, change) in changes.iter().enumerate() {
        for (document, range) in [(&mut *before, &change.before), (&mut *after, &change.after)] {
            if range.is_empty() {
                document.gaps.push((range.start, index));
            }
            for block in document.blocks.get_mut(range.clone()).unwrap_or_default() {
                block.change = Some(index);
            }
        }
    }
    changes
}

/// Reads both sides, and splits each into its blocks.
pub(crate) async fn read_documents(
    git: Git,
    repository: Arc<Repository>,
    objects: Objects,
    (before, after): (Option<PictureSource>, Option<PictureSource>),
) -> Result<Arc<Documents>, Problem> {
    jobs::blocking(move || {
        let read = |source: Option<PictureSource>| {
            source.map(|source| {
                let bytes = read_side(&git, &repository, &objects, &source, LARGEST)?;
                Ok(Document::new(
                    &String::from_utf8_lossy(&bytes),
                    u64::try_from(bytes.len()).unwrap_or(u64::MAX),
                ))
            })
        };
        let (mut before, mut after) = (read(before), read(after));
        let changes = match (&mut before, &mut after) {
            (Some(Ok(before)), Some(Ok(after))) => compare(before, after),
            _ => Vec::new(),
        };
        Ok(Arc::new(Documents {
            before,
            after,
            changes,
        }))
    })
    .await
}

/// `text` made ready to draw: every image a link to it, an image's icon and its words its text
/// (nothing is fetched to draw a document), and a task's box an icon (`egui_commonmark` draws a
/// checkbox nothing names).
pub(crate) fn prepared(text: &str) -> String {
    let mut replaced: Vec<(Range<usize>, String)> = Vec::new();
    let mut image: Option<(Range<usize>, String, String)> = None;
    for (event, range) in Parser::new_ext(text, options()).into_offset_iter() {
        match event {
            Event::Start(Tag::Image { dest_url, .. }) if image.is_none() => {
                image = Some((range, dest_url.into_string(), String::new()));
            }
            Event::Text(words) | Event::Code(words) => {
                if let Some((_, _, alt)) = &mut image {
                    alt.push_str(&words);
                }
            }
            Event::End(TagEnd::Image) => {
                if let Some((range, url, alt)) = image.take() {
                    let words = if alt.trim().is_empty() {
                        "image"
                    } else {
                        alt.trim()
                    };
                    let link = format!(
                        "[{} {}](<{}>)",
                        icon::IMAGE,
                        escaped(words),
                        url.replace(['<', '>'], "")
                    );
                    replaced.push((range, link));
                }
            }
            Event::TaskListMarker(done) if image.is_none() => {
                let mark = if done {
                    icon::CHECK_SQUARE
                } else {
                    icon::SQUARE
                };
                replaced.push((range, mark.to_owned()));
            }
            _ => {}
        }
    }
    if replaced.is_empty() {
        return text.to_owned();
    }
    let mut made = String::with_capacity(text.len());
    let mut at = 0;
    for (range, with) in replaced {
        made.push_str(text.get(at..range.start).unwrap_or_default());
        made.push_str(&with);
        at = range.end;
    }
    made.push_str(text.get(at..).unwrap_or_default());
    made
}

/// What [`split`] makes: the blocks, the link reference definitions, the headings' anchors.
type Split = (Vec<Block>, String, Vec<(String, usize)>);

/// `text`'s blocks: its top-level elements, a top-level list's items one by one (an ordered
/// one's numbered as it was); the link reference definitions it holds, written out; and its
/// headings' anchors.
fn split(text: &str) -> Split {
    let mut blocks = Vec::new();
    let mut anchors: Vec<(String, usize)> = Vec::new();
    let mut depth = 0_usize;
    let mut start = 0;
    // The top-level list under way: its next item's number, if ordered.
    let mut list: Option<Option<u64>> = None;
    let mut heading: Option<String> = None;
    // The top-level list's items so far.
    let mut items = 0_usize;
    let mut parsed = Parser::new_ext(text, options()).into_offset_iter();
    for (event, range) in parsed.by_ref() {
        match event {
            Event::Start(tag) => {
                match (&tag, depth, list) {
                    (Tag::List(first), 0, _) => list = Some(*first),
                    (Tag::Item, 1, Some(_)) | (_, 0, _) => start = range.start,
                    _ => {}
                }
                if depth == 0 && matches!(tag, Tag::Heading { .. }) {
                    heading = Some(String::new());
                }
                depth = depth.saturating_add(1);
            }
            Event::End(tag) => {
                depth = depth.saturating_sub(1);
                match (tag, depth, list) {
                    (TagEnd::Item, 1, Some(number)) => {
                        let item = start..range.end;
                        let numbered = number.map(|number| numbered(text, item.clone(), number));
                        list = Some(number.map(|number| number.saturating_add(1)));
                        let room = if items == 0 {
                            room(&blocks, Room::Block)
                        } else {
                            Room::Item
                        };
                        items = items.saturating_add(1);
                        blocks.push(block(text, item, numbered, room));
                    }
                    (TagEnd::List(_), 0, _) => {
                        list = None;
                        items = 0;
                    }
                    (_, 0, _) => {
                        let kind = heading.take().map_or(Room::Block, |words| {
                            anchors.push((anchor(&words, &anchors), blocks.len()));
                            Room::Heading
                        });
                        blocks.push(block(text, start..range.end, None, room(&blocks, kind)));
                    }
                    _ => {}
                }
            }
            Event::Text(words) | Event::Code(words) => {
                if let Some(heading) = &mut heading {
                    heading.push_str(&words);
                }
            }
            _ if depth == 0 => {
                let room = room(&blocks, Room::Block);
                blocks.push(block(text, range, None, room));
            }
            _ => {}
        }
    }
    let mut references = String::new();
    for (_, definition) in parsed.reference_definitions().iter() {
        references.push_str(
            text.get(definition.span.clone())
                .unwrap_or_default()
                .trim_end(),
        );
        references.push('\n');
    }
    (blocks, references, anchors)
}

/// The room before a block of `kind` after `blocks`: none before the first.
const fn room(blocks: &[Block], kind: Room) -> Room {
    if blocks.is_empty() { Room::None } else { kind }
}

/// A block of `text` over `range`, but the blank lines and spaces ending it, `room` before it.
fn block(text: &str, range: Range<usize>, numbered: Option<String>, room: Room) -> Block {
    let kept = text
        .get(range.clone())
        .map_or(0, |text| text.trim_end().len());
    let range = range.start..range.start.saturating_add(kept);
    let numbered = numbered.map(|numbered| numbered.trim_end().to_owned());
    let lines = text
        .get(range.clone())
        .map_or(1, |text| text.lines().count().max(1));
    Block {
        range,
        numbered,
        lines: u32::try_from(lines).unwrap_or(u32::MAX),
        room,
        change: None,
    }
}

/// An ordered list's item over `range` with its number written as `number`: drawn alone, its
/// number is the list's first.
fn numbered(text: &str, range: Range<usize>, number: u64) -> String {
    let item = text.get(range).unwrap_or_default();
    let indent = item.len().saturating_sub(item.trim_start().len());
    let rest = item.get(indent..).unwrap_or_default();
    let digits = rest
        .len()
        .saturating_sub(rest.trim_start_matches(|c: char| c.is_ascii_digit()).len());
    format!(
        "{}{number}{}",
        item.get(..indent).unwrap_or_default(),
        rest.get(digits..).unwrap_or_default()
    )
}

/// A heading's anchor as GitHub names it: lower case, its spaces hyphens, punctuation left out,
/// and a number after one already taken (`-1`, `-2`).
fn anchor(words: &str, taken: &[(String, usize)]) -> String {
    let base: String = words
        .trim()
        .to_lowercase()
        .chars()
        .filter_map(|character| match character {
            ' ' => Some('-'),
            '-' | '_' => Some(character),
            character if character.is_alphanumeric() => Some(character),
            _ => None,
        })
        .collect();
    let mut anchor = base.clone();
    let mut again = 0_u32;
    while taken.iter().any(|(name, _)| *name == anchor) {
        again = again.saturating_add(1);
        anchor = format!("{base}-{again}");
    }
    anchor
}

/// Words written into a link's text so Markdown takes them as they are.
fn escaped(words: &str) -> String {
    let mut out = String::with_capacity(words.len());
    for character in words.chars() {
        if matches!(character, '\\' | '[' | ']' | '*' | '_' | '`' | '<' | '>') {
            out.push('\\');
        }
        out.push(character);
    }
    out
}

/// The Markdown read: GitHub's tables, task lists, strikethrough, and footnotes, as
/// `egui_commonmark` reads it.
fn options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_FOOTNOTES
}

/// A document's preview's state: `egui_commonmark`'s, each block's height (guessed until drawn)
/// at the width they were measured at, a line of its body's height, where to scroll next frame,
/// and the block to scroll to once heights are known.
#[derive(Default)]
pub(crate) struct Preview {
    cache: CommonMarkCache,
    heights: Vec<(f32, bool)>,
    width: f32,
    line: f32,
    scroll_to: Option<f32>,
    target: Option<usize>,
}

impl std::fmt::Debug for Preview {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Preview").finish_non_exhaustive()
    }
}

/// Both sides' previews, and the change in view.
#[derive(Debug, Default)]
pub(crate) struct Previews {
    pub(crate) before: Preview,
    pub(crate) after: Preview,
    /// The change in view: the first once the documents land, then as stepped to.
    change: Option<usize>,
}

impl Previews {
    /// The change in view, while there are any.
    pub(crate) const fn change(&self) -> Option<usize> {
        self.change
    }

    /// Shows change `index`: each side scrolled to it.
    pub(crate) fn go(&mut self, documents: &Documents, index: usize) {
        let Some(change) = documents.changes.get(index) else {
            return;
        };
        self.change = Some(index);
        self.before.target = Some(change.before.start);
        self.after.target = Some(change.after.start);
    }

    /// Whether both sides have measured every block and scrolled where asked.
    #[cfg(test)]
    pub(crate) fn settled(&self) -> bool {
        [&self.before, &self.after].into_iter().all(|preview| {
            preview.target.is_none()
                && preview.scroll_to.is_none()
                && preview.heights.iter().all(|(_, measured)| *measured)
        })
    }

    /// Goes to the first change as documents land; read again, the change in view stays (or
    /// the last, if there are fewer now) and nothing scrolls.
    pub(crate) fn follow(&mut self, documents: &Documents) {
        match self.change {
            None => self.go(documents, 0),
            Some(index) if index >= documents.changes.len() => {
                self.change = documents.changes.len().checked_sub(1);
            }
            Some(_) => {}
        }
    }
}

/// Draws `document` as it reads, in a scroll area of its own (`id`), only the blocks in view,
/// change `current` marked strongest; returns the pages its links asked for, for egit's opener
/// (egui's own output loses them).
pub(crate) fn show(
    ui: &mut Ui,
    preview: &mut Preview,
    document: &Document,
    (current, id): (Option<usize>, impl egui::AsIdSalt),
) -> Vec<String> {
    // A change stepped to: its first block a line below the top, once there are heights.
    if let Some(block) = preview.target {
        if preview.heights.len() == document.blocks.len() {
            preview.target = None;
            preview.scroll_to = Some((top_of(preview, block) - preview.line).max(0.0));
        } else {
            ui.ctx().request_repaint();
        }
    }
    let commands = ui.ctx().output(|output| output.commands.len());
    let mut area = ScrollArea::vertical()
        .id_salt(id)
        .auto_shrink([false, false]);
    if let Some(offset) = preview.scroll_to.take() {
        area = area.vertical_scroll_offset(offset);
    }
    let shown = area.show_viewport(ui, |ui, viewport| {
        // A document's scale: headings stepping up from its body.
        let style = ui.style_mut();
        for (text_style, size) in [(TextStyle::Body, BODY), (TextStyle::Heading, HEADING)] {
            if let Some(font) = style.text_styles.get_mut(&text_style) {
                font.size = size;
            }
        }
        blocks(ui, preview, document, (viewport, current))
    });
    // Blocks above the view that turned out taller or shorter than guessed: the view stays.
    if shown.inner != 0.0 && preview.scroll_to.is_none() {
        preview.scroll_to = Some((shown.state.offset.y + shown.inner).max(0.0));
        ui.ctx().request_repaint();
    }
    let pages = ui.ctx().output_mut(|output| {
        let mut pages = Vec::new();
        let added = output
            .commands
            .split_off(commands.min(output.commands.len()));
        for command in added {
            match command {
                egui::OutputCommand::OpenUrl(open) => pages.push(open.url),
                command => output.commands.push(command),
            }
        }
        pages
    });
    // A link to a heading scrolls to it; the rest are for the opener.
    pages
        .into_iter()
        .filter(|page| {
            let Some(name) = page.strip_prefix('#') else {
                return true;
            };
            if let Some(&(_, block)) = document.anchors.iter().find(|(anchor, _)| anchor == name) {
                preview.scroll_to = Some(top_of(preview, block));
                ui.ctx().request_repaint();
            }
            false
        })
        .collect()
}

/// Draws the blocks `viewport` shows (and measures some beyond it), change `current` marked
/// strongest; returns how much those above its top grew, to keep the view where it is.
fn blocks(
    ui: &mut Ui,
    preview: &mut Preview,
    document: &Document,
    (viewport, current): (Rect, Option<usize>),
) -> f32 {
    let gutter = if document.sided == Sided::Alone {
        0.0
    } else {
        GUTTER
    };
    let width = ui.available_width() - gutter;
    let line = ui.text_style_height(&TextStyle::Body) * 1.4;
    preview.line = line;
    let mut grew = 0.0;
    if preview.heights.len() != document.blocks.len() || (preview.width - width).abs() > 0.5 {
        // At a new width the block at the view's top stays there, as far into it as it was.
        let anchor = (preview.heights.len() == document.blocks.len())
            .then(|| block_at(preview, viewport.min.y));
        // Guessed again: a line of a block's source, a line drawn.
        preview.heights = document
            .blocks
            .iter()
            .map(|block| {
                let lines = f32::from(u16::try_from(block.lines).unwrap_or(u16::MAX));
                ((lines + block.room.lines()) * line, false)
            })
            .collect();
        preview.width = width;
        if let Some((index, within)) = anchor {
            let height = preview
                .heights
                .get(index)
                .map_or(0.0, |(height, _)| *height);
            grew = top_of(preview, index) + within.min(height) - viewport.min.y;
        }
    }
    let total: f32 = preview.heights.iter().map(|(height, _)| height).sum();
    let place = Place {
        origin: ui.min_rect().min,
        gutter,
        width,
        line,
        current,
        palette: Palette::of(ui.ctx()),
    };
    ui.set_height(total);
    let mut top = 0.0;
    let mut index = 0;
    // Blocks wholly above the view.
    let mut above = 0;
    while let Some(&(height, _)) = preview.heights.get(index) {
        if top > viewport.max.y {
            break;
        }
        let drawn = if top + height >= viewport.min.y {
            gap(ui, document, place, (index, top));
            draw_block(ui, preview, document, (index, top), (place, false))
        } else {
            above = index.saturating_add(1);
            height
        };
        if top < viewport.min.y {
            grew += drawn - height;
        }
        top += drawn;
        index = index.saturating_add(1);
    }
    if index == document.blocks.len() {
        gap(ui, document, place, (index, top));
    }
    grew + measure_ahead(ui, preview, document, (place, index, above))
}

/// The block at `y` from the document's top, and how far into it `y` is.
fn block_at(preview: &Preview, y: f32) -> (usize, f32) {
    let mut top = 0.0;
    for (index, (height, _)) in preview.heights.iter().enumerate() {
        if top + height > y {
            return (index, y - top);
        }
        top += height;
    }
    (preview.heights.len(), y - top)
}

/// Where a document's blocks go and how: its top left, the room left of them (a compared
/// document's), their width, a line of its body's height, the change marked strongest, and
/// the colours.
#[derive(Clone, Copy)]
struct Place {
    origin: egui::Pos2,
    gutter: f32,
    width: f32,
    line: f32,
    current: Option<usize>,
    palette: Palette,
}

/// Draws block `index` at `top` (or, `sizing`, lays it out unseen) below its room, tinted when
/// it changed; notes and returns its height, its room with it.
fn draw_block(
    ui: &mut Ui,
    preview: &mut Preview,
    document: &Document,
    (index, top): (usize, f32),
    (place, sizing): (Place, bool),
) -> f32 {
    let Some(text) = document.block_text(index) else {
        return 0.0;
    };
    let room = document.room(index) * place.line;
    let rect = Rect::from_min_size(
        pos2(place.origin.x + place.gutter, place.origin.y + top + room),
        vec2(place.width, f32::INFINITY),
    );
    let change = document
        .blocks
        .get(index)
        .and_then(|block| block.change)
        .filter(|_| !sizing);
    // Behind the text, painted once its height is known.
    let tint = change.map(|_| ui.painter().add(egui::Shape::Noop));
    let mut builder = UiBuilder::new().max_rect(rect).id_salt((sizing, index));
    if sizing {
        builder = builder.sizing_pass().invisible();
    }
    let drawn = ui
        .scope_builder(builder, |ui| {
            viewer().show(ui, &mut preview.cache, &text);
        })
        .response
        .rect;
    if let (Some(change), Some(tint)) = (change, tint) {
        let of = |index: Option<usize>| {
            index
                .and_then(|index| document.blocks.get(index))
                .and_then(|block| block.change)
        };
        // One band with the blocks of its change next to it (the room between them tinted
        // too, once); a little past its text where the change begins or ends.
        let from = if of(index.checked_sub(1)) == Some(change) {
            top
        } else {
            top + (room - TINT).max(0.0)
        };
        let to = if of(index.checked_add(1)) == Some(change) {
            0.0
        } else {
            TINT
        };
        let band = Rect::from_min_max(
            pos2(place.origin.x, place.origin.y + from),
            pos2(
                place.origin.x + place.gutter + place.width,
                drawn.bottom() + to,
            ),
        );
        let tints = tints(document.sided, place.palette);
        let fill = if place.current == Some(change) {
            tints.strong
        } else {
            tints.soft
        };
        ui.painter()
            .set(tint, egui::Shape::rect_filled(band, 0.0, fill));
        ui.painter().rect_filled(
            Rect::from_min_size(band.min, vec2(BAR, band.height())),
            0.0,
            tints.bar,
        );
    }
    let height = drawn.height() + room;
    if let Some(measured) = preview.heights.get_mut(index) {
        *measured = (height, true);
    }
    height
}

/// A changed block's colours: behind it, behind the change in view, and its bar.
struct Tints {
    soft: Color32,
    strong: Color32,
    bar: Color32,
}

/// The colours a side's changed blocks take: removed before, added after.
const fn tints(sided: Sided, palette: Palette) -> Tints {
    match sided {
        Sided::Before => Tints {
            soft: palette.removed,
            strong: palette.removed_word,
            bar: palette.danger,
        },
        Sided::After | Sided::Alone => Tints {
            soft: palette.added,
            strong: palette.added_word,
            bar: palette.ok,
        },
    }
}

/// Marks where the other side has blocks this one hasn't, before block `index` (at `top`): a
/// line in the room above it, in the colour of what that side did (stronger for the change in
/// view).
fn gap(ui: &Ui, document: &Document, place: Place, (index, top): (usize, f32)) {
    let Ok(found) = document.gaps.binary_search_by_key(&index, |(at, _)| *at) else {
        return;
    };
    let change = document.gaps.get(found).map(|(_, change)| *change);
    let other = match document.sided {
        Sided::Before => Sided::After,
        Sided::After | Sided::Alone => Sided::Before,
    };
    let room = document.room(index) * place.line;
    let thickness = if place.current == change { 3.0 } else { 2.0 };
    let y = place.origin.y + top + (room / 2.0).max(thickness);
    ui.painter().hline(
        place.origin.x..=place.origin.x + place.gutter + place.width,
        y,
        egui::Stroke::new(thickness, tints(other, place.palette).bar),
    );
}

/// Measures blocks not yet drawn from `from` on, as long as [`MEASURING`] allows, laid out
/// unseen where they belong: the scroll bar settles as the guesses are replaced. Returns how
/// much the first `above` (wholly above the view) grew, to keep the view where it is.
fn measure_ahead(
    ui: &mut Ui,
    preview: &mut Preview,
    document: &Document,
    (place, from, above): (Place, usize, usize),
) -> f32 {
    let started = Instant::now();
    let unmeasured: Vec<usize> = (from..preview.heights.len())
        .chain(0..from)
        .filter(|index| {
            preview
                .heights
                .get(*index)
                .is_some_and(|(_, measured)| !measured)
        })
        .take(64)
        .collect();
    let mut grew = 0.0;
    for index in unmeasured {
        if started.elapsed() > MEASURING {
            ui.ctx().request_repaint();
            break;
        }
        let guessed = preview
            .heights
            .get(index)
            .map_or(0.0, |(height, _)| *height);
        let top = top_of(preview, index);
        let measured = draw_block(ui, preview, document, (index, top), (place, true));
        if index < above {
            grew += measured - guessed;
        }
    }
    grew
}

/// How a block is drawn (an image's words are its link's: no hover needed).
/// A short Markdown text (a pull request's description) drawn whole in `ui`, its HTML comments
/// (which a page doesn't show: a template's notes) left out; the web pages its links asked to
/// open, for egit's opener.
pub(crate) fn show_text(ui: &mut Ui, cache: &mut CommonMarkCache, text: &str) -> Vec<String> {
    let commands = ui.ctx().output(|output| output.commands.len());
    viewer().show(ui, cache, &without_comments(text));
    ui.ctx().output_mut(|output| {
        let mut pages = Vec::new();
        let added = output
            .commands
            .split_off(commands.min(output.commands.len()));
        for command in added {
            match command {
                egui::OutputCommand::OpenUrl(open) if crate::desktop::is_web_page(&open.url) => {
                    pages.push(open.url);
                }
                egui::OutputCommand::OpenUrl(_) => {}
                command => output.commands.push(command),
            }
        }
        pages
    })
}

/// `text` without its HTML comments (`<!-- … -->`).
fn without_comments(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.contains("<!--") {
        return std::borrow::Cow::Borrowed(text);
    }
    let mut kept = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<!--") {
        kept.push_str(rest.get(..start).unwrap_or_default());
        let after = rest.get(start..).unwrap_or_default();
        rest = after
            .find("-->")
            .and_then(|end| after.get(end.saturating_add(3)..))
            .unwrap_or_default();
    }
    kept.push_str(rest);
    std::borrow::Cow::Owned(kept)
}

fn viewer() -> CommonMarkViewer<'static> {
    CommonMarkViewer::new().show_alt_text_on_hover(false)
}

/// Where block `index` starts, from the document's top.
fn top_of(preview: &Preview, index: usize) -> f32 {
    preview
        .heights
        .iter()
        .take(index)
        .map(|(height, _)| height)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_description_s_html_comments_are_left_out() {
        assert_eq!(
            without_comments("Fixes it.\n<!-- Describe the change. -->\nDone.<!-- open"),
            "Fixes it.\n\nDone."
        );
        assert!(matches!(
            without_comments("No comment."),
            std::borrow::Cow::Borrowed("No comment.")
        ));
    }

    #[test]
    fn markdown_files_are_known_by_their_extension() {
        assert!(is_markdown(b"README.md"));
        assert!(is_markdown(b"docs/guide.MARKDOWN"));
        assert!(!is_markdown(b"notes.txt"));
        assert!(!is_markdown(b"md"));
        assert!(!is_markdown(b"folder.md/file"));
    }

    #[test]
    fn images_become_links_to_them() {
        assert_eq!(
            prepared("Look: ![a *red* dot](https://example.com/dot.png \"Dot\") here.\n"),
            format!(
                "Look: [{} a red dot](<https://example.com/dot.png>) here.\n",
                icon::IMAGE
            )
        );
        assert_eq!(
            prepared("![](shots/one.png)\n"),
            format!("[{} image](<shots/one.png>)\n", icon::IMAGE)
        );
        // Code isn't an image.
        let code = "`![x](y.png)`\n\n    ![x](y.png)\n";
        assert_eq!(prepared(code), code);
    }

    #[test]
    fn tasks_boxes_become_icons() {
        assert_eq!(
            prepared("- [ ] write it\n- [x] read it\n"),
            format!(
                "- {} write it\n- {} read it\n",
                icon::SQUARE,
                icon::CHECK_SQUARE
            )
        );
    }

    #[test]
    fn documents_split_into_blocks_and_list_items() {
        let document = Document::new(
            "# Title\n\nA [link][site].\n\n- one\n- two\n  nested text\n\n\
             3. third\n1. fourth\n\n```\ncode\n```\n\n## Title\n\n[site]: https://example.com\n",
            0,
        );
        let texts: Vec<String> = (0..document.blocks())
            .filter_map(|index| document.block_text(index))
            .collect();
        assert_eq!(texts.first().map(String::as_str), Some("# Title"));
        // A reference-style link takes its definition along.
        assert!(
            texts
                .get(1)
                .is_some_and(|text| text.starts_with("A [link][site].")
                    && text.contains("[site]: https://example.com"))
        );
        assert_eq!(texts.get(2).map(String::as_str), Some("- one"));
        assert_eq!(
            texts.get(3).map(String::as_str),
            Some("- two\n  nested text")
        );
        // An ordered list's items keep their numbers, drawn alone.
        assert_eq!(texts.get(4).map(String::as_str), Some("3. third"));
        assert_eq!(texts.get(5).map(String::as_str), Some("4. fourth"));
        assert_eq!(texts.get(6).map(String::as_str), Some("```\ncode\n```"));
        assert_eq!(
            document.anchors,
            [("title".to_owned(), 0), ("title-1".to_owned(), 7)]
        );
        assert_eq!(document.blocks(), 8);
    }

    #[test]
    fn documents_are_compared_block_by_block() {
        let mut before = Document::new("# T\n\nA\n\nB\n\nC\n\n- x\n- y\n", 0);
        let mut after = Document::new("# T\n\nA\n\nB, again\n\nC\n\n- x\n- y\n\nD\n", 0);
        let changes = compare(&mut before, &mut after);
        assert_eq!(
            changes,
            [
                Change {
                    before: 2..3,
                    after: 2..3
                },
                Change {
                    before: 6..6,
                    after: 6..7
                },
            ]
        );
        let marks = |document: &Document| -> Vec<Option<usize>> {
            document.blocks.iter().map(|block| block.change).collect()
        };
        assert_eq!(marks(&before), [None, None, Some(0), None, None, None]);
        assert_eq!(
            marks(&after),
            [None, None, Some(0), None, None, None, Some(1)]
        );
        // The paragraph added at the end leaves a gap at the end of the side before.
        assert_eq!(before.gaps, [(6, 1)]);
        assert_eq!(after.gaps, []);
        assert_eq!((before.sided, after.sided), (Sided::Before, Sided::After));
    }
}
