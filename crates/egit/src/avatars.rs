//! People's pictures, drawn in place of their initials once fetched (the preferences can keep
//! to initials): GitHub's (the account an email belongs to, as GitHub shows commits' authors),
//! else Gravatar's (`egit_forge::PictureSources`).
//!
//! A picture is fetched once, on a worker thread (two of them, the newest request first: the
//! rows on screen), cropped square and scaled to 128 × 128 at once, and kept in a cache on disk
//! (with a marker for someone without one, asked again after a week). Then it is scaled down
//! to 64, 32, and 16 pixels too (egui's renderer makes no mipmaps: a picture drawn at a
//! quarter of its size would shimmer), each level masked to a circle, and the four laid in one
//! tile of an atlas: one texture holds everyone's, so every avatar on screen is a quad sampling
//! it, never a texture switch, and each is drawn from the smallest level at least its size on
//! screen. The atlas holds [`SLOTS`] people; the one drawn least recently makes room.

use std::collections::{HashMap, VecDeque};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::task::{Poll, Waker};
use std::time::{Duration, SystemTime};

use web_time::Instant;

use egit_forge::{Fetched, PictureSources, Pictures};
use egit_git::sync::Mutex;
use egui::{Color32, ColorImage, Context, Id, Rect, TextureHandle, TextureId, TextureOptions};
use image::imageops::FilterType;
use image::{DynamicImage, RgbaImage};

/// The size a picture is kept at, once fetched.
pub(crate) const MASTER: u32 = 128;

/// Each level of a tile: its size, and where it lies in the tile.
const LEVELS: [(u16, [usize; 2]); 4] = [
    (128, [0, 0]),
    (64, [128, 0]),
    (32, [128, 64]),
    (16, [160, 64]),
];

/// A person's tile: the 128-pixel level, the others beside it.
const TILE: [usize; 2] = [192, 128];

/// The atlas: 10 tiles across, 8 down.
const ATLAS: [usize; 2] = [2048, 1024];
const COLUMNS: usize = 10;

/// How many people the atlas holds at once.
pub(crate) const SLOTS: usize = 80;

/// How many requests wait at most: older ones are dropped (asked again if drawn again).
const QUEUED: usize = 128;

/// How many pictures go into the atlas a frame at most.
const UPLOADS_A_FRAME: usize = 8;

/// How long someone without a picture is believed to have none.
const MISSING_FOR: Duration = Duration::from_hours(7 * 24);

/// How long a failed fetch (offline, say) waits before being tried again.
const FAILED_FOR: Duration = Duration::from_mins(10);

/// The largest picture decoded (a service sends 128 pixels; anything far larger is wrong).
const LARGEST_SIDE: u32 = 2048;

/// A request for someone's picture.
struct Request {
    key: u64,
    email: String,
}

/// What a worker found.
enum Landed {
    Picture {
        key: u64,
        tile: ColorImage,
    },
    Missing(u64),
    Failed(u64),
    /// Dropped from the queue before it was looked at.
    Dropped(u64),
}

/// What is known of someone's picture.
#[derive(Clone, Copy, Debug)]
enum Entry {
    Asked,
    Ready { slot: usize },
    None { until: Instant },
}

/// The requests waiting, the newest first, and the workers waiting for one.
#[derive(Default)]
struct Queue {
    waiting: Mutex<Waiting>,
}

#[derive(Default)]
struct Waiting {
    requests: VecDeque<Request>,
    workers: Vec<Waker>,
}

impl Queue {
    /// The newest request, once there is one.
    async fn next(&self) -> Request {
        std::future::poll_fn(|context| {
            let mut waiting = self.waiting.lock();
            if let Some(request) = waiting.requests.pop_front() {
                return Poll::Ready(request);
            }
            if !waiting
                .workers
                .iter()
                .any(|worker| worker.will_wake(context.waker()))
            {
                waiting.workers.push(context.waker().clone());
            }
            Poll::Pending
        })
        .await
    }
}

/// The pictures known, the atlas, and the way to the workers.
struct State {
    enabled: bool,
    atlas: Option<TextureHandle>,
    entries: HashMap<u64, Entry>,
    /// Each slot's person and the frame it was last drawn in.
    slots: Vec<Option<(u64, u64)>>,
    frame: u64,
    queue: Arc<Queue>,
    landed: Receiver<Landed>,
    sender: Sender<Landed>,
}

/// The pictures of a window's people, shared through its context.
#[derive(Clone)]
struct Avatars(Arc<Mutex<State>>);

fn id() -> Id {
    Id::new("egit-avatars")
}

fn avatars(ctx: &Context) -> Option<Avatars> {
    ctx.data(|data| data.get_temp::<Avatars>(id()))
}

/// Starts fetching pictures for `ctx`'s window from `sources` (through `relay`, in a browser),
/// kept in `cache` (when there is one): two workers for the life of the process (threads; in a
/// browser, the page's tasks).
pub(crate) fn install(
    ctx: &Context,
    sources: &PictureSources,
    cache: Option<&Path>,
    relay: Option<egit_git::Relay>,
) {
    let queue = Arc::new(Queue::default());
    let (sender, landed) = mpsc::channel();
    let state = State {
        enabled: true,
        atlas: None,
        entries: HashMap::new(),
        slots: vec![None; SLOTS],
        frame: 0,
        queue: Arc::clone(&queue),
        landed,
        sender: sender.clone(),
    };
    let pictures = Pictures::new(relay);
    let cache = cache.map(Path::to_path_buf);
    if let Some(cache) = &cache {
        forget_older(cache);
    }
    for worker in 0..2 {
        let (queue, sender, ctx) = (
            Arc::clone(&queue),
            sender.clone(),
            crate::repaint::Repaint::of(ctx),
        );
        let (sources, pictures, cache) = (sources.clone(), pictures.clone(), cache.clone());
        let spawned = egit_git::exec::spawn(&format!("avatars {worker}"), async move {
            let fetching = Fetching {
                sources: &sources,
                pictures: &pictures,
                cache: cache.as_deref(),
            };
            loop {
                let request = queue.next().await;
                let landed = fetching.picture(&request).await;
                if sender.send(landed).is_err() {
                    return;
                }
                ctx.request_repaint();
            }
        });
        if let Err(error) = spawned {
            tracing::warn!(%error, "an avatar worker couldn't be started");
        }
    }
    ctx.data_mut(|data| data.insert_temp(id(), Avatars(Arc::new(Mutex::new(state)))));
}

/// Draws pictures, or keeps to initials.
pub(crate) fn set_enabled(ctx: &Context, enabled: bool) {
    if let Some(avatars) = avatars(ctx) {
        avatars.0.lock().enabled = enabled;
    }
}

/// Each frame, before drawing: takes in what the workers found, a few into the atlas.
pub(crate) fn poll(ctx: &Context) {
    let Some(avatars) = avatars(ctx) else {
        return;
    };
    let mut state = avatars.0.lock();
    state.frame = state.frame.wrapping_add(1);
    for _ in 0..UPLOADS_A_FRAME {
        let Ok(landed) = state.landed.try_recv() else {
            break;
        };
        match landed {
            Landed::Picture { key, tile } => state.place(ctx, key, tile),
            Landed::Missing(key) => {
                let until = later(MISSING_FOR);
                state.entries.insert(key, Entry::None { until });
            }
            Landed::Failed(key) => {
                let until = later(FAILED_FOR);
                state.entries.insert(key, Entry::None { until });
            }
            Landed::Dropped(key) => {
                state.entries.remove(&key);
            }
        }
    }
}

/// `email`'s picture to draw `diameter` points across, when it is in: the atlas and the part
/// of it (the smallest level at least that many pixels). Asks for it otherwise.
pub(crate) fn picture(ctx: &Context, email: &str, diameter: f32) -> Option<(TextureId, Rect)> {
    let email = email.trim();
    if email.is_empty() || !email.contains('@') {
        return None;
    }
    let avatars = avatars(ctx)?;
    let mut state = avatars.0.lock();
    if !state.enabled {
        return None;
    }
    let key = key_of(email);
    match state.entries.get(&key).copied() {
        Some(Entry::Ready { slot }) => {
            let frame = state.frame;
            if let Some(Some((_, drawn))) = state.slots.get_mut(slot) {
                *drawn = frame;
            }
            let atlas = state.atlas.as_ref()?.id();
            Some((atlas, uv(slot, diameter * ctx.pixels_per_point())))
        }
        Some(Entry::Asked) => None,
        Some(Entry::None { until }) if until > Instant::now() => None,
        Some(Entry::None { .. }) | None => {
            state.entries.insert(key, Entry::Asked);
            state.ask(key, email);
            None
        }
    }
}

/// What is known of `email`'s picture (tests): "ready", "asked", or "none"; `None` before it
/// was asked for.
#[cfg(test)]
pub(crate) fn known(ctx: &Context, email: &str) -> Option<&'static str> {
    let avatars = avatars(ctx)?;
    let state = avatars.0.lock();
    state
        .entries
        .get(&key_of(email.trim()))
        .map(|entry| match entry {
            Entry::Ready { .. } => "ready",
            Entry::Asked => "asked",
            Entry::None { .. } => "none",
        })
}

impl State {
    /// Puts a request at the queue's front, dropping the oldest past [`QUEUED`].
    fn ask(&self, key: u64, email: &str) {
        let mut waiting = self.queue.waiting.lock();
        waiting.requests.push_front(Request {
            key,
            email: email.to_owned(),
        });
        while waiting.requests.len() > QUEUED {
            if let Some(dropped) = waiting.requests.pop_back() {
                let _ = self.sender.send(Landed::Dropped(dropped.key));
            }
        }
        let worker = waiting.workers.pop();
        drop(waiting);
        if let Some(worker) = worker {
            worker.wake();
        }
    }

    /// Lays `tile` into a free slot, or the one drawn least recently (its person forgotten).
    fn place(&mut self, ctx: &Context, key: u64, tile: ColorImage) {
        let free = self.slots.iter().position(Option::is_none);
        let slot = free.or_else(|| {
            self.slots
                .iter()
                .enumerate()
                .filter_map(|(at, held)| held.map(|(_, drawn)| (at, drawn)))
                .filter(|(_, drawn)| *drawn != self.frame)
                .min_by_key(|(_, drawn)| *drawn)
                .map(|(at, _)| at)
        });
        let Some(slot) = slot else {
            // Every slot drawn this frame: asked again when drawn next.
            self.entries.remove(&key);
            return;
        };
        if let Some(Some((evicted, _))) = self.slots.get(slot) {
            self.entries.remove(evicted);
        }
        let atlas = self.atlas.get_or_insert_with(|| {
            ctx.load_texture(
                "avatars",
                ColorImage::filled(ATLAS, Color32::TRANSPARENT),
                TextureOptions::LINEAR,
            )
        });
        atlas.set_partial(tile_origin(slot), tile, TextureOptions::LINEAR);
        if let Some(held) = self.slots.get_mut(slot) {
            *held = Some((key, self.frame));
        }
        self.entries.insert(key, Entry::Ready { slot });
    }
}

/// `wait` from now.
fn later(wait: Duration) -> Instant {
    let now = Instant::now();
    now.checked_add(wait).unwrap_or(now)
}

/// An email's key: FNV-1a of it in lower case (allocating nothing to look it up).
fn key_of(email: &str) -> u64 {
    email.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte.to_ascii_lowercase())).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Where `slot`'s tile starts in the atlas.
fn tile_origin(slot: usize) -> [usize; 2] {
    let (row, column) = (
        slot.checked_div(COLUMNS).unwrap_or(0),
        slot.checked_rem(COLUMNS).unwrap_or(0),
    );
    let [width, height] = TILE;
    [column.saturating_mul(width), row.saturating_mul(height)]
}

/// The part of the atlas holding `slot`'s level for a picture `pixels` across on screen: the
/// smallest at least that large (the largest beyond it).
fn uv(slot: usize, pixels: f32) -> Rect {
    let (size, [x, y]) = LEVELS
        .iter()
        .rev()
        .find(|(size, _)| f32::from(*size) >= pixels)
        .or_else(|| LEVELS.first())
        .copied()
        .unwrap_or((128, [0, 0]));
    let [left, top] = tile_origin(slot);
    let to_f32 = |at: usize| f32::from(u16::try_from(at).unwrap_or(u16::MAX));
    let [width, height] = ATLAS.map(to_f32);
    let size = f32::from(size);
    let min = egui::pos2(
        to_f32(left.saturating_add(x)) / width,
        to_f32(top.saturating_add(y)) / height,
    );
    Rect::from_min_size(min, egui::vec2(size / width, size / height))
}

/// A worker's way to a picture.
struct Fetching<'a> {
    sources: &'a PictureSources,
    pictures: &'a Pictures,
    cache: Option<&'a Path>,
}

impl Fetching<'_> {
    /// `request`'s picture: from the cache, else fetched (and cached).
    async fn picture(&self, request: &Request) -> Landed {
        let key = request.key;
        let files = self.cache.map(|cache| {
            (
                cache.join(format!("{key:016x}.png")),
                cache.join(format!("{key:016x}.none")),
            )
        });
        if let Some((kept, none)) = &files {
            if fresh(none) {
                return Landed::Missing(key);
            }
            if let Some(master) = std::fs::read(kept).ok().and_then(|bytes| decode(&bytes)) {
                return Landed::Picture {
                    key,
                    tile: tile(&master),
                };
            }
        }
        match self
            .pictures
            .find(self.sources, &request.email, MASTER)
            .await
        {
            Ok(Fetched::Found(bytes)) => {
                let Some(master) = decode(&bytes) else {
                    return Landed::Missing(key);
                };
                if let Some((kept, _)) = &files {
                    keep(&master, kept);
                }
                Landed::Picture {
                    key,
                    tile: tile(&master),
                }
            }
            Ok(Fetched::Missing) => {
                if let Some((_, none)) = &files {
                    let _ = std::fs::create_dir_all(none.parent().unwrap_or(none));
                    let _ = std::fs::write(none, b"");
                }
                Landed::Missing(key)
            }
            Err(error) => {
                tracing::debug!(%error, "a picture couldn't be fetched");
                Landed::Failed(key)
            }
        }
    }
}

/// Whether a "has none" marker is younger than [`MISSING_FOR`].
fn fresh(marker: &Path) -> bool {
    std::fs::metadata(marker)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|made| SystemTime::now().duration_since(made).ok())
        .is_some_and(|age| age < MISSING_FOR)
}

/// A picture's bytes decoded, cropped to its middle square, and scaled to [`MASTER`] pixels.
fn decode(bytes: &[u8]) -> Option<RgbaImage> {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(LARGEST_SIDE);
    limits.max_image_height = Some(LARGEST_SIDE);
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    reader.limits(limits);
    let decoded: DynamicImage = reader.decode().ok()?;
    let (width, height) = (decoded.width(), decoded.height());
    let side = width.min(height);
    if side == 0 {
        return None;
    }
    let square = decoded.crop_imm(
        width.saturating_sub(side) >> 1,
        height.saturating_sub(side) >> 1,
        side,
        side,
    );
    Some(if side == MASTER {
        square.to_rgba8()
    } else {
        image::imageops::resize(&square.to_rgba8(), MASTER, MASTER, FilterType::Lanczos3)
    })
}

/// Keeps `master` in the cache (written beside, then moved in: a reader never sees half).
fn keep(master: &RgbaImage, path: &Path) {
    let Some(folder) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(folder).is_err() {
        return;
    }
    let writing = path.with_extension(format!("{}.part", std::process::id()));
    let kept = master
        .save_with_format(&writing, image::ImageFormat::Png)
        .is_ok()
        && std::fs::rename(&writing, path).is_ok();
    if !kept {
        let _ = std::fs::remove_file(&writing);
    }
}

/// `master`'s tile: it and its halves down to 16 pixels, each masked to a circle (a pixel
/// inside the edge, so sampling never reaches a neighbour's).
fn tile(master: &RgbaImage) -> ColorImage {
    let mut tile = ColorImage::filled(TILE, Color32::TRANSPARENT);
    let [tile_width, _] = TILE;
    let mut level = master.clone();
    for (size, [left, top]) in LEVELS {
        if level.width() != u32::from(size) {
            level = image::imageops::resize(
                &level,
                u32::from(size),
                u32::from(size),
                FilterType::Triangle,
            );
        }
        let rows = tile
            .pixels
            .chunks_mut(tile_width)
            .skip(top)
            .take(usize::from(size));
        for (y, (row, source)) in rows.zip(level.rows()).enumerate() {
            for (x, (pixel, from)) in row.iter_mut().skip(left).zip(source).enumerate() {
                let [red, green, blue, alpha] = from.0;
                let color = Color32::from_rgba_unmultiplied(red, green, blue, alpha);
                *pixel = color.gamma_multiply(coverage((x, y), size));
            }
        }
    }
    tile
}

/// How much of pixel `(x, y)` a circle `size` across covers, its edge a pixel in.
fn coverage((x, y): (usize, usize), size: u16) -> f32 {
    let at = |n: usize| f32::from(u16::try_from(n).unwrap_or(u16::MAX)) + 0.5;
    let half = f32::from(size) / 2.0;
    let (dx, dy) = (at(x) - half, at(y) - half);
    let distance = dx.hypot(dy);
    (half - 1.0 - distance + 0.5).clamp(0.0, 1.0)
}

/// The cache's version: what a picture is (GitHub's first, since 2) has changed, so earlier
/// answers aren't taken (and are removed, `forget_older`).
const CACHE_VERSION: &str = "2";

/// Where pictures are kept on this computer: the platform's cache folder's
/// `egit/pictures/<version>`.
pub(crate) fn cache_folder(home: Option<&Path>) -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    let base = home.map(|home| home.join("Library/Caches"));
    #[cfg(windows)]
    let base = {
        let _ = home;
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    };
    #[cfg(not(any(target_os = "macos", windows)))]
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|folder| folder.is_absolute())
        .or_else(|| home.map(|home| home.join(".cache")));
    base.map(|base| base.join("egit").join("pictures").join(CACHE_VERSION))
}

/// Removes the pictures an earlier version kept (in the folder above `cache`), in the
/// background.
fn forget_older(cache: &Path) {
    let Some(pictures) = cache.parent().map(Path::to_path_buf) else {
        return;
    };
    let spawned = egit_git::exec::spawn("avatars' clean-up", async move {
        let Ok(entries) = std::fs::read_dir(&pictures) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let kept = path
                .extension()
                .is_some_and(|extension| extension == "png" || extension == "none");
            if kept && path.is_file() {
                let _ = std::fs::remove_file(&path);
            }
        }
    });
    if let Err(error) = spawned {
        tracing::debug!(%error, "the old pictures couldn't be cleared");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_are_chosen_no_smaller_than_drawn() {
        let size = |pixels: f32| {
            let uv = uv(0, pixels);
            uv.width() * 2048.0
        };
        assert!((size(18.0) - 32.0).abs() < 0.01);
        assert!((size(16.0) - 16.0).abs() < 0.01);
        assert!((size(36.0) - 64.0).abs() < 0.01);
        assert!((size(100.0) - 128.0).abs() < 0.01);
        assert!((size(400.0) - 128.0).abs() < 0.01, "the largest beyond it");
        // Slot 11: the second row's second tile.
        assert_eq!(tile_origin(11), [192, 128]);
    }

    #[test]
    fn a_tile_holds_every_level_round() {
        let master = RgbaImage::from_pixel(128, 128, image::Rgba([200, 40, 40, 255]));
        let tile = tile(&master);
        let at = |x: usize, y: usize| tile.pixels.get(y * 192 + x).copied().unwrap();
        // The middle of each level is the picture; its corners are clear.
        for (size, [left, top]) in LEVELS {
            let size = usize::from(size);
            assert_eq!(at(left + (size >> 1), top + (size >> 1)).a(), 255, "{size}");
            assert_eq!(at(left, top), Color32::TRANSPARENT, "{size}");
            assert_eq!(at(left + size - 1, top + size - 1), Color32::TRANSPARENT);
        }
        assert_eq!(key_of("Ada@Example.com"), key_of("ada@example.com"));
    }

    #[test]
    fn pictures_are_cropped_square_and_scaled() {
        let wide = RgbaImage::from_pixel(300, 200, image::Rgba([1, 2, 3, 255]));
        let mut bytes = Vec::new();
        DynamicImage::ImageRgba8(wide)
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        let master = decode(&bytes).unwrap();
        assert_eq!(master.dimensions(), (MASTER, MASTER));
        assert!(decode(b"not a picture").is_none());
    }
}
