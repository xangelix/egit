//! Changed files as a tree of their folders: each folder before what it holds, folders before
//! files, a chain of folders holding only one folder shown as one (`src/core`), and folded
//! folders' contents skipped.
//!
//! A tree is made once per list of paths, in one sort and one pass, and holds no path of its
//! own: its nodes are ranges of the paths the caller keeps (24 bytes a node). Which rows show
//! is worked out again only when a folder is folded or unfolded, in one pass that skips what
//! folded folders hold.

use std::cmp::Ordering;
use std::collections::HashSet;

/// One row of a tree: a folder or a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Node {
    /// How deep it is (a chain of folders shown as one counts once).
    pub(crate) depth: u16,
    /// Whether it is shown as one with the folder before it (`core` of `src/core`).
    merged: bool,
    /// The entry whose path holds its name (a folder's: the first file it holds).
    pub(crate) entry: u32,
    /// Its name's bytes in that path, `start` up to `end`; a folder's whole path is the path's
    /// bytes up to `end`.
    start: u32,
    end: u32,
    /// A folder's: the node after everything in it.
    pub(crate) after: u32,
    /// A folder's: how many files it holds (none for a file).
    pub(crate) files: u32,
}

impl Node {
    /// Whether it is a folder.
    pub(crate) const fn is_folder(&self) -> bool {
        self.files > 0
    }

    /// Its name, in `path` (its entry's path).
    pub(crate) fn name<'p>(&self, path: &'p [u8]) -> &'p [u8] {
        path.get(span(self.start)..span(self.end))
            .unwrap_or_default()
    }

    /// A folder's whole path (`src/core`), in `path` (its entry's path).
    pub(crate) fn folder_path<'p>(&self, path: &'p [u8]) -> &'p [u8] {
        path.get(..span(self.end)).unwrap_or_default()
    }
}

/// A position as an index.
fn span(at: u32) -> usize {
    usize::try_from(at).unwrap_or(usize::MAX)
}

/// An index as a position (no list comes near 4 billion entries).
fn at(index: usize) -> u32 {
    u32::try_from(index).unwrap_or(u32::MAX)
}

/// Some paths as a tree.
#[derive(Debug, Default)]
pub(crate) struct FileTree {
    nodes: Vec<Node>,
}

/// A folder open while a tree is made.
struct Open {
    /// Its node.
    node: usize,
    /// Where its path's prefix (`src/core/`) ends in its entry's path.
    prefix: usize,
    /// How many files had been placed when it opened.
    placed: u32,
}

impl FileTree {
    /// The tree of `count` entries' paths (`path_of` gives entry `i`'s, `/` between folders).
    pub(crate) fn new<'p>(count: usize, path_of: impl Fn(usize) -> &'p [u8]) -> Self {
        let mut order: Vec<u32> = (0..at(count)).collect();
        order.sort_unstable_by(|&a, &b| tree_order(path_of(span(a)), path_of(span(b))));
        let mut nodes: Vec<Node> = Vec::with_capacity(count.saturating_add(count >> 2));
        let mut open: Vec<Open> = Vec::new();
        let mut placed = 0_u32;
        for entry in order {
            let path = path_of(span(entry));
            // Close the folders this path isn't in: the order keeps a folder's files together.
            while let Some(folder) = open.last() {
                let first = nodes.get(folder.node).map_or(entry, |node| node.entry);
                let prefix = path_of(span(first))
                    .get(..folder.prefix)
                    .unwrap_or_default();
                if path.starts_with(prefix) {
                    break;
                }
                close(&mut nodes, &mut open, placed);
            }
            // Open the folders it is in that aren't yet.
            let mut from = open.last().map_or(0, |folder| folder.prefix);
            while let Some(slash) = path
                .get(from..)
                .and_then(|rest| rest.iter().position(|&byte| byte == b'/'))
            {
                let end = from.saturating_add(slash);
                nodes.push(node(entry, (from, end), open.len()));
                open.push(Open {
                    node: nodes.len().saturating_sub(1),
                    prefix: end.saturating_add(1),
                    placed,
                });
                from = end.saturating_add(1);
            }
            nodes.push(node(entry, (from, path.len()), open.len()));
            placed = placed.saturating_add(1);
        }
        while !open.is_empty() {
            close(&mut nodes, &mut open, placed);
        }
        merge_chains(&mut nodes);
        Self { nodes }
    }

    /// Node `index`.
    pub(crate) fn node(&self, index: u32) -> Option<&Node> {
        self.nodes.get(span(index))
    }

    /// The rows shown, into `rows` (each a node's index): every node but what folded folders
    /// hold (`folded`: their whole paths) and folders shown as one with the one before.
    pub(crate) fn visible<'p>(
        &self,
        path_of: impl Fn(usize) -> &'p [u8],
        folded: &HashSet<Vec<u8>>,
        rows: &mut Vec<u32>,
    ) {
        rows.clear();
        let mut index = 0;
        while let Some(node) = self.nodes.get(index) {
            let next = index.saturating_add(1);
            if node.merged {
                index = next;
                continue;
            }
            rows.push(at(index));
            index = if node.is_folder()
                && !folded.is_empty()
                && folded.contains(node.folder_path(path_of(span(node.entry))))
            {
                span(node.after)
            } else {
                next
            };
        }
    }

    /// The entries a folder (node `index`) holds, in the tree's order.
    pub(crate) fn files_in(&self, index: u32) -> impl Iterator<Item = u32> + '_ {
        let after = self.node(index).map_or(0, |node| node.after);
        self.nodes
            .get(span(index).saturating_add(1)..span(after))
            .unwrap_or_default()
            .iter()
            .filter(|node| !node.is_folder())
            .map(|node| node.entry)
    }

    /// How many nodes it has.
    #[cfg(test)]
    pub(crate) const fn len(&self) -> usize {
        self.nodes.len()
    }
}

/// A node named by bytes `name` of entry `entry`'s path, inside `depth` folders.
fn node(entry: u32, (start, end): (usize, usize), depth: usize) -> Node {
    Node {
        depth: u16::try_from(depth).unwrap_or(u16::MAX),
        merged: false,
        entry,
        start: at(start),
        end: at(end),
        after: 0,
        files: 0,
    }
}

/// Closes the last folder opened: what it holds ends here, and how many files that is.
fn close(nodes: &mut [Node], open: &mut Vec<Open>, placed: u32) {
    let after = at(nodes.len());
    if let Some(folder) = open.pop()
        && let Some(node) = nodes.get_mut(folder.node)
    {
        node.after = after;
        node.files = placed.saturating_sub(folder.placed);
    }
}

/// Shows each folder holding only one folder as one with it (`src` and `core` as
/// `src/core`), and sets every node's depth as shown.
fn merge_chains(nodes: &mut [Node]) {
    // Each folder open: its `after`, the depth of what it holds, and the node showing it.
    let mut open: Vec<(u32, u16, usize)> = Vec::new();
    for index in 0..nodes.len() {
        while open
            .last()
            .is_some_and(|&(after, _, _)| span(after) <= index)
        {
            open.pop();
        }
        let depth = open.last().map_or(0, |&(_, depth, _)| depth);
        let Some(node) = nodes.get(index).copied() else {
            continue;
        };
        if !node.is_folder() {
            if let Some(file) = nodes.get_mut(index) {
                file.depth = depth;
            }
            continue;
        }
        // A folder right after its parent and ending with it is all the parent holds.
        let parent = open.last().copied();
        let only_child = index
            .checked_sub(1)
            .and_then(|before| nodes.get(before))
            .is_some_and(|before| before.is_folder() && before.after == node.after);
        match parent {
            Some((_, _, shown)) if only_child => {
                if let Some(folder) = nodes.get_mut(index) {
                    folder.merged = true;
                }
                if let Some(showing) = nodes.get_mut(shown) {
                    showing.end = node.end;
                }
                open.push((node.after, depth, shown));
            }
            _ => {
                if let Some(folder) = nodes.get_mut(index) {
                    folder.depth = depth;
                }
                open.push((node.after, depth.saturating_add(1), index));
            }
        }
    }
}

/// How two paths sort in a tree: by their folders, then names, a folder before a file where
/// they part (bytes compared, as git compares paths).
fn tree_order(a: &[u8], b: &[u8]) -> Ordering {
    let mut a_parts = a.split(|&byte| byte == b'/');
    let mut b_parts = b.split(|&byte| byte == b'/');
    loop {
        match (a_parts.next(), b_parts.next()) {
            (Some(a_part), Some(b_part)) => {
                if a_part == b_part {
                    continue;
                }
                let a_folder = a_parts.clone().next().is_some();
                let b_folder = b_parts.clone().next().is_some();
                return b_folder.cmp(&a_folder).then_with(|| a_part.cmp(b_part));
            }
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (None, None) => return Ordering::Equal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATHS: [&str; 9] = [
        "src/core/b.c",
        "README",
        "src/main.rs",
        "docs/x/y/z.md",
        "a.txt",
        "src/core/a.c",
        "lib/one/a",
        "lib/two/b",
        "lib/two/deep/c",
    ];

    fn tree() -> FileTree {
        FileTree::new(PATHS.len(), |index| PATHS[index].as_bytes())
    }

    /// The rows shown, each its name and depth.
    fn shown(tree: &FileTree, folded: &[&str]) -> Vec<(String, u16)> {
        let folded: HashSet<Vec<u8>> = folded.iter().map(|path| path.as_bytes().to_vec()).collect();
        let mut rows = Vec::new();
        tree.visible(|index| PATHS[index].as_bytes(), &folded, &mut rows);
        rows.iter()
            .map(|&row| {
                let node = tree.node(row).unwrap();
                let path = PATHS[span(node.entry)].as_bytes();
                (
                    String::from_utf8(node.name(path).to_vec()).unwrap(),
                    node.depth,
                )
            })
            .collect()
    }

    #[test]
    fn folders_come_first_and_lone_folders_join_their_parents() {
        let tree = tree();
        let rows = shown(&tree, &[]);
        let expected = [
            ("docs/x/y", 0),
            ("z.md", 1),
            ("lib", 0),
            ("one", 1),
            ("a", 2),
            ("two", 1),
            ("deep", 2),
            ("c", 3),
            ("b", 2),
            ("src", 0),
            ("core", 1),
            ("a.c", 2),
            ("b.c", 2),
            ("main.rs", 1),
            ("README", 0),
            ("a.txt", 0),
        ];
        let expected: Vec<(String, u16)> = expected
            .iter()
            .map(|&(name, depth)| (name.to_owned(), depth))
            .collect();
        assert_eq!(rows, expected);
    }

    #[test]
    fn folders_count_their_files_and_fold() {
        let tree = tree();
        let mut rows = Vec::new();
        tree.visible(|index| PATHS[index].as_bytes(), &HashSet::new(), &mut rows);
        let folder = |name: &str| {
            rows.iter()
                .copied()
                .find(|&row| {
                    let node = tree.node(row).unwrap();
                    node.is_folder()
                        && node.name(PATHS[span(node.entry)].as_bytes()) == name.as_bytes()
                })
                .unwrap()
        };
        assert_eq!(tree.node(folder("src")).unwrap().files, 3);
        assert_eq!(tree.node(folder("core")).unwrap().files, 2);
        assert_eq!(tree.node(folder("docs/x/y")).unwrap().files, 1);
        let held: Vec<&str> = tree
            .files_in(folder("src"))
            .map(|entry| PATHS[span(entry)])
            .collect();
        assert_eq!(held, ["src/core/a.c", "src/core/b.c", "src/main.rs"]);

        let names: Vec<String> = shown(&tree, &["src", "lib/two", "docs/x/y"])
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(
            names,
            [
                "docs/x/y", "lib", "one", "a", "two", "src", "README", "a.txt"
            ]
        );
    }

    #[test]
    fn nothing_and_a_lone_file_make_small_trees() {
        let empty = FileTree::new(0, |_| b"".as_slice());
        assert_eq!(empty.len(), 0);
        let one = FileTree::new(1, |_| b"only".as_slice());
        assert_eq!(one.len(), 1);
        assert!(!one.node(0).unwrap().is_folder());
    }
}
