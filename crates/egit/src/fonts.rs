//! System fonts for what egui's own don't cover: CJK, and most scripts besides Latin, Greek,
//! and Cyrillic. A view notes each character it draws without a glyph ([`note_missing`],
//! [`note_text`]); a job looks for a system font that has it; the font is added to both
//! families as a fallback, once.
//!
//! Fonts are looked for in the system's font directories (XDG's; macOS's and Windows' own), CJK
//! families first, then monospace ones,
//! then regular weights; in a collection, the face covering the most is kept, preferring a
//! monospace face and the user's language (`LANG`: Japanese, Korean, simplified or traditional
//! Chinese). A face must have outlines for the glyphs (egui can't draw colour bitmaps). Files are
//! read while looked through and dropped unless kept; a kept font lives as long as the window, so
//! its bytes are leaked once into a `'static` slice, which egui borrows (owned font data, it
//! copies on every rebuild of its fonts).

use std::borrow::Cow;
use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::epaint::text::{FontInsert, FontPriority, InsertFontFamily};
use egui::{Context, FontData, FontFamily, FontId, Id, Ui};
use egui_async::Bind;
use parking_lot::Mutex;
use skrifa::MetadataProvider as _;
use skrifa::raw::FileRef;
use skrifa::string::StringId;

use crate::jobs;
use crate::problem::Problem;

/// The most fonts one lookup keeps.
const MAX_KEPT: usize = 4;

/// The deepest a font directory is searched.
const MAX_DEPTH: usize = 8;

/// Characters drawn without a glyph, shared through the context.
#[derive(Clone, Debug, Default)]
struct Missing(Arc<Mutex<BTreeSet<char>>>);

fn missing_id() -> Id {
    Id::new("egit missing glyphs")
}

/// Notes that `c` was drawn without a glyph.
pub(crate) fn note_missing(ctx: &Context, c: char) {
    let missing = ctx.data_mut(|data| {
        data.get_temp_mut_or_default::<Missing>(missing_id())
            .clone()
    });
    missing.0.lock().insert(c);
}

/// Notes the characters of `text` that no font has (nothing to check for ASCII).
pub(crate) fn note_text(ui: &Ui, font: &FontId, text: &str) {
    if text.is_ascii() {
        return;
    }
    let missing: Vec<char> = ui.fonts_mut(|fonts| {
        text.chars()
            .filter(|c| !c.is_ascii() && !fonts.has_glyph(font, *c))
            .collect()
    });
    for c in missing {
        note_missing(ui.ctx(), c);
    }
}

/// How many fonts egui is using: a view caching glyph advances drops them when it changes. (A
/// font added takes effect at the next frame's start, and only then does this count it.)
pub(crate) fn generation(ctx: &Context) -> usize {
    ctx.fonts(|fonts| fonts.definitions().font_data.len())
}

/// A system font kept for its glyphs.
#[derive(Debug)]
struct Kept {
    path: PathBuf,
    index: u32,
    bytes: &'static [u8],
}

/// The fallback fonts: what was looked for, and what was kept.
#[derive(Debug)]
pub(crate) struct Fallbacks {
    /// Characters looked for already (found or not: none is looked for twice).
    looked_for: HashSet<char>,
    /// The fonts kept: their files and faces.
    kept: HashSet<(PathBuf, u32)>,
    lookup: Bind<Vec<Kept>, Problem>,
}

impl Default for Fallbacks {
    fn default() -> Self {
        Self {
            looked_for: HashSet::new(),
            kept: HashSet::new(),
            lookup: Bind::new(true),
        }
    }
}

impl Fallbacks {
    /// Adds the fonts a finished lookup kept; starts one for the characters noted since, when
    /// none is under way.
    pub(crate) fn poll(&mut self, ctx: &Context) {
        if let Some(found) = self.lookup.take() {
            match found {
                Ok(fonts) => {
                    for font in fonts {
                        add(ctx, &font);
                        self.kept.insert((font.path, font.index));
                    }
                }
                Err(problem) => tracing::warn!(%problem, "no system font was looked through"),
            }
        }
        if self.lookup.is_pending() {
            return;
        }
        let missing = ctx.data_mut(|data| {
            data.get_temp_mut_or_default::<Missing>(missing_id())
                .clone()
        });
        let wanted: BTreeSet<char> = std::mem::take(&mut *missing.0.lock())
            .into_iter()
            .filter(|c| self.looked_for.insert(*c))
            .collect();
        if wanted.is_empty() {
            return;
        }
        let kept = self.kept.clone();
        self.lookup
            .request(jobs::blocking(async move { Ok(look_for(wanted, &kept)) }));
    }
}

/// Adds `font` to every family, after every other font.
fn add(ctx: &Context, font: &Kept) {
    let name = format!("{}#{}", font.path.display(), font.index);
    tracing::info!(font = %name, "using a system font for characters egui's fonts lack");
    let data = FontData {
        font: Cow::Borrowed(font.bytes),
        index: font.index,
        tweak: egui::FontTweak::default(),
    };
    let families = std::iter::once(FontFamily::Monospace)
        .chain(crate::theme::Weight::ALL.map(crate::theme::Weight::family))
        .map(|family| InsertFontFamily {
            family,
            priority: FontPriority::Lowest,
        })
        .collect();
    ctx.add_font(FontInsert::new(&name, data, families));
}

/// Looks through the system's fonts for faces with glyphs for `wanted`, keeping up to
/// [`MAX_KEPT`] (the files of `kept` aren't read again).
fn look_for(mut wanted: BTreeSet<char>, kept: &HashSet<(PathBuf, u32)>) -> Vec<Kept> {
    let language = cjk_language();
    let mut found = Vec::new();
    for path in candidates() {
        if wanted.is_empty() || found.len() >= MAX_KEPT {
            break;
        }
        if kept.iter().any(|(kept, _)| *kept == path) {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let Some((index, covered)) = best_face(&bytes, &wanted, language) else {
            continue;
        };
        for c in &covered {
            wanted.remove(c);
        }
        found.push(Kept {
            path,
            index,
            // Leaked on purpose: a kept font lives as long as the window (see the module).
            bytes: Box::leak(bytes.into_boxed_slice()),
        });
    }
    if !wanted.is_empty() {
        tracing::debug!(?wanted, "no system font has glyphs for these characters");
    }
    found
}

/// A face of a font file, and how well it serves.
struct Choice {
    /// How many wanted characters it covers, whether it is monospace, and whether it is in
    /// the user's language: more of each is better, in that order.
    rank: (usize, bool, bool),
    index: u32,
    covered: Vec<char>,
}

/// The face of a font file covering the most of `wanted` (preferring monospace, then
/// `language`), and the characters it covers.
fn best_face(bytes: &[u8], wanted: &BTreeSet<char>, language: &str) -> Option<(u32, Vec<char>)> {
    let file = FileRef::new(bytes).ok()?;
    let mut best: Option<Choice> = None;
    for (index, face) in file.fonts().enumerate() {
        let Ok(face) = face else {
            continue;
        };
        let charmap = face.charmap();
        let outlines = face.outline_glyphs();
        let covered: Vec<char> = wanted
            .iter()
            .copied()
            .filter(|c| {
                charmap
                    .map(*c)
                    .is_some_and(|glyph| outlines.get(glyph).is_some())
            })
            .collect();
        if covered.is_empty() {
            continue;
        }
        let family = face
            .localized_strings(StringId::FAMILY_NAME)
            .english_or_first()
            .map(|name| name.to_string())
            .unwrap_or_default();
        let rank = (
            covered.len(),
            family.contains("Mono"),
            !language.is_empty() && family.ends_with(language),
        );
        if best.as_ref().is_none_or(|best| rank > best.rank) {
            best = Some(Choice {
                rank,
                index: u32::try_from(index).unwrap_or(0),
                covered,
            });
        }
    }
    best.map(|best| (best.index, best.covered))
}

/// Which CJK face a collection should offer first, from the user's language: `JP`, `KR`,
/// `SC`, `TC`, `HK`, or none.
fn cjk_language() -> &'static str {
    let locale = ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .find_map(|key| std::env::var(key).ok().filter(|value| !value.is_empty()))
        .unwrap_or_default();
    match locale.get(..5).unwrap_or(&locale) {
        "zh_TW" | "zh_MO" => "TC",
        "zh_HK" => "HK",
        _ if locale.starts_with("zh") => "SC",
        _ if locale.starts_with("ko") => "KR",
        _ if locale.starts_with("ja") => "JP",
        _ => "",
    }
}

/// Every font file in the system's font directories, best candidates first.
fn candidates() -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut seen = HashSet::new();
    for dir in font_dirs() {
        collect_fonts(&dir, 0, &mut seen, &mut files);
    }
    files.sort_by_cached_key(|path| (rank(path), path.clone()));
    files
}

/// Where macOS installs fonts: the system's, everyone's, and the user's.
#[cfg(target_os = "macos")]
fn font_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    [
        PathBuf::from("/System/Library/Fonts"),
        PathBuf::from("/Library/Fonts"),
    ]
    .into_iter()
    .chain(home.map(|home| home.join("Library/Fonts")))
    .collect()
}

/// Where Windows installs fonts: the system's (`%WINDIR%\Fonts`) and the user's.
#[cfg(windows)]
fn font_dirs() -> Vec<PathBuf> {
    let windows =
        std::env::var_os("WINDIR").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    std::iter::once(windows.join("Fonts"))
        .chain(local.map(|local| local.join(r"Microsoft\Windows\Fonts")))
        .collect()
}

/// Where fonts are installed: `$XDG_DATA_HOME/fonts`, `~/.fonts`, and each of
/// `$XDG_DATA_DIRS`' `fonts`.
#[cfg(not(any(target_os = "macos", windows)))]
fn font_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|home| home.join(".local/share")));
    let data_dirs = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".to_owned());
    data_home
        .map(|dir| dir.join("fonts"))
        .into_iter()
        .chain(home.map(|home| home.join(".fonts")))
        .chain(
            data_dirs
                .split(':')
                .filter(|dir| !dir.is_empty())
                .map(|dir| Path::new(dir).join("fonts")),
        )
        .collect()
}

/// Adds the font files under `dir` to `files`, once per directory.
fn collect_fonts(dir: &Path, depth: usize, seen: &mut HashSet<PathBuf>, files: &mut Vec<PathBuf>) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(canonical) = dir.canonicalize() else {
        return;
    };
    if !seen.insert(canonical) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_fonts(&path, depth.saturating_add(1), seen, files);
        } else if path.extension().is_some_and(|extension| {
            ["ttf", "otf", "ttc", "otc"]
                .iter()
                .any(|kind| extension.eq_ignore_ascii_case(kind))
        }) {
            files.push(path);
        }
    }
}

/// How good a candidate a font file is, by its name (lower is better): regular weights before
/// styled ones; CJK families, then monospace, then Noto and `DejaVu`, then the rest.
fn rank(path: &Path) -> u8 {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let styled = [
        "bold",
        "black",
        "light",
        "thin",
        "medium",
        "heavy",
        "italic",
        "oblique",
        "semi",
        "extra",
        "demi",
        "condensed",
        "serif",
    ]
    .iter()
    .any(|style| name.contains(style));
    // CJK families: free ones, then macOS's and Windows'.
    let family: u8 = if [
        "cjk",
        "han",
        "wqy",
        "droidsansfallback",
        "sarasa",
        "pingfang",
        "hiragino",
        "msyh",
        "msjh",
        "yugoth",
        "meiryo",
        "malgun",
    ]
    .iter()
    .any(|cjk| name.contains(cjk))
    {
        0
    } else if ["mono", "menlo", "consola"]
        .iter()
        .any(|mono| name.contains(mono))
    {
        1
    } else if name.starts_with("noto") || name.starts_with("dejavu") {
        2
    } else {
        3
    };
    if styled {
        family.saturating_add(4)
    } else {
        family
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regular_cjk_and_monospace_families_come_first() {
        let ranked = |name: &str| rank(Path::new(name));
        assert!(ranked("NotoSansCJK-Regular.ttc") < ranked("DejaVuSansMono.ttf"));
        assert!(ranked("DejaVuSansMono.ttf") < ranked("NotoSans-Regular.ttf"));
        assert!(ranked("NotoSans-Regular.ttf") < ranked("Ubuntu-R.ttf"));
        assert!(ranked("Ubuntu-R.ttf") < ranked("NotoSansCJK-Bold.ttc"));
        assert!(ranked("NotoSansCJK-Regular.ttc") < ranked("NotoSerifCJK-Regular.ttc"));
    }

    #[test]
    fn a_font_that_isnt_one_offers_nothing() {
        let wanted = BTreeSet::from(['漢']);
        assert_eq!(best_face(b"not a font", &wanted, "JP"), None);
    }

    /// The CJK collection Ubuntu and Arch install, when this machine has it.
    #[test]
    fn a_cjk_collection_offers_its_monospace_face_in_the_users_language() {
        let path = candidates()
            .into_iter()
            .find(|path| path.ends_with("NotoSansCJK-Regular.ttc"));
        let Some(path) = path else {
            return;
        };
        let bytes = std::fs::read(path).unwrap_or_default();
        let wanted = BTreeSet::from(['漢', 'あ', '한', 'A']);
        let (index, covered) = best_face(&bytes, &wanted, "KR").unwrap_or_default();
        assert_eq!(covered.len(), 4);
        let file = FileRef::new(&bytes).ok();
        let family = file
            .and_then(|file| file.fonts().nth(usize::try_from(index).unwrap_or(0)))
            .and_then(Result::ok)
            .and_then(|face| {
                face.localized_strings(StringId::FAMILY_NAME)
                    .english_or_first()
                    .map(|name| name.to_string())
            });
        assert_eq!(family.as_deref(), Some("Noto Sans Mono CJK KR"));
    }
}
