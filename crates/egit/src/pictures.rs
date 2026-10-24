//! Images a change replaced, read and decoded off the interface thread to show side by side:
//! each side a blob (through the shared object reader) or the work tree's file, decoded within
//! bounds (an image's size, the memory decoding it may take), and scaled down to at most
//! [`SHOWN`] pixels a side, which is all a screen shows, so its texture stays small.

use std::io::Cursor;
use std::sync::Arc;

use bstr::{BString, ByteSlice as _};
use egit_git::{Git, ObjectId, ObjectKind, Repository};
use egui::ColorImage;

use crate::history::Objects;
use crate::jobs;
use crate::problem::Problem;

/// The longest side an image is kept at, in pixels.
const SHOWN: u32 = 2048;

/// The largest file read as an image.
const LARGEST_FILE: u64 = 64 << 20;

/// The widest and tallest image decoded, and the most memory decoding one may take.
const LARGEST_SIDE: u32 = 20_000;
const LARGEST_ALLOCATION: u64 = 512 << 20;

/// Whether `path` names an image egit shows (by its extension, in any case).
pub(crate) fn is_image(path: &[u8]) -> bool {
    let name = path.rsplit(|&byte| byte == b'/').next().unwrap_or(path);
    let Some((_, extension)) = name.rsplit_once_str(b".") else {
        return false;
    };
    ["png", "jpg", "jpeg", "gif", "webp", "bmp", "ico"]
        .iter()
        .any(|known| extension.eq_ignore_ascii_case(known.as_bytes()))
}

/// Where one side of an image's change is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PictureSource {
    /// A blob.
    Blob(ObjectId),
    /// The work tree's file, at this path from its top.
    WorkTree(BString),
}

/// An image, ready to show.
#[derive(Debug)]
pub(crate) struct Picture {
    /// Its size, in pixels, as stored.
    pub(crate) width: u32,
    pub(crate) height: u32,
    /// Its file's size, in bytes.
    pub(crate) bytes: u64,
    /// Its pixels, scaled down to at most [`SHOWN`] a side.
    pub(crate) pixels: Arc<ColorImage>,
}

/// One side of a change: none (the image was added, or deleted), the image, or why it
/// couldn't be shown.
pub(crate) type Side = Option<Result<Picture, String>>;

/// Both sides of an image's change.
#[derive(Debug)]
pub(crate) struct Pictures {
    pub(crate) before: Side,
    pub(crate) after: Side,
}

/// Reads and decodes both sides.
pub(crate) async fn read_pictures(
    git: Git,
    repository: Arc<Repository>,
    objects: Objects,
    (before, after): (Option<PictureSource>, Option<PictureSource>),
) -> Result<Arc<Pictures>, Problem> {
    jobs::blocking(async move {
        let read = async |source: Option<PictureSource>| {
            let bytes = read_side(&git, &repository, &objects, &source?, LARGEST_FILE).await;
            Some(bytes.and_then(|bytes| decode(&bytes)))
        };
        Ok(Arc::new(Pictures {
            before: read(before).await,
            after: read(after).await,
        }))
    })
    .await
}

/// A side's bytes, refused past `largest`.
pub(crate) async fn read_side(
    git: &Git,
    repository: &Repository,
    objects: &Objects,
    source: &PictureSource,
    largest: u64,
) -> Result<Vec<u8>, String> {
    let too_large = |size: u64| {
        format!(
            "It is {}: larger than egit shows.",
            crate::format::size(size)
        )
    };
    match source {
        PictureSource::Blob(id) => {
            let mut bytes = None;
            objects
                .read(git, repository.work_tree(), &[*id], |_, object| {
                    bytes = object
                        .filter(|object| object.kind == ObjectKind::Blob)
                        .map(|object| object.data.to_vec());
                })
                .await
                .map_err(|error| error.to_string())?;
            let bytes = bytes.ok_or_else(|| format!("{id} isn't in this repository."))?;
            let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
            if size > largest {
                return Err(too_large(size));
            }
            Ok(bytes)
        }
        PictureSource::WorkTree(path) => {
            // Where git runs: on another computer, there.
            let file = repository.work_tree().join(path.to_path_lossy());
            let files = git.files();
            let found = files
                .look(&[&file])
                .await
                .map_err(|error| error.to_string())?
                .pop();
            match found {
                Some(egit_git::Entry::File { size, .. }) if size > largest => {
                    return Err(too_large(size));
                }
                Some(egit_git::Entry::File { .. }) => {}
                _ => return Err(format!("{} isn't a file.", file.display())),
            }
            files.read(&file).await.map_err(|error| error.to_string())
        }
    }
}

/// Decodes an image within bounds and scales it down to [`SHOWN`] pixels a side at most.
fn decode(bytes: &[u8]) -> Result<Picture, String> {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(LARGEST_SIDE);
    limits.max_image_height = Some(LARGEST_SIDE);
    limits.max_alloc = Some(LARGEST_ALLOCATION);
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| error.to_string())?;
    reader.limits(limits);
    let decoded = reader.decode().map_err(|error| error.to_string())?;
    let (width, height) = (decoded.width(), decoded.height());
    let shown = if width > SHOWN || height > SHOWN {
        decoded.thumbnail(SHOWN, SHOWN)
    } else {
        decoded
    };
    let rgba = shown.to_rgba8();
    let size = [
        usize::try_from(rgba.width()).unwrap_or(0),
        usize::try_from(rgba.height()).unwrap_or(0),
    ];
    Ok(Picture {
        width,
        height,
        bytes: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        pixels: Arc::new(ColorImage::from_rgba_unmultiplied(size, rgba.as_raw())),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PNG of `width` by `height` pixels, each `rgba`.
    fn png(width: u32, height: u32, rgba: [u8; 4]) -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(width, height, image::Rgba(rgba));
        let mut bytes = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        bytes
    }

    #[test]
    fn images_are_known_by_their_extension() {
        assert!(is_image(b"assets/logo.PNG"));
        assert!(is_image(b"photo.jpeg"));
        assert!(is_image(b"a.b/icon.ico"));
        assert!(!is_image(b"notes.txt"));
        assert!(!is_image(b"png"));
        assert!(!is_image(b"folder.png/file"));
    }

    #[test]
    fn images_are_decoded_and_large_ones_scaled_down() {
        let small = decode(&png(3, 2, [255, 0, 0, 128])).unwrap();
        assert_eq!((small.width, small.height), (3, 2));
        assert_eq!(small.pixels.size, [3, 2]);
        let large = decode(&png(4000, 1000, [0, 0, 255, 255])).unwrap();
        assert_eq!((large.width, large.height), (4000, 1000));
        assert_eq!(large.pixels.size, [2048, 512]);
        assert!(decode(b"not an image").is_err());
    }
}
