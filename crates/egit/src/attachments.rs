//! Images attached to an agent's prompt: read from files (picked, or dropped on the window) in
//! a job, checked to be an image Claude reads (PNG, JPEG, GIF, WebP), scaled down where the
//! API wouldn't take it as it is, encoded as it is sent, and given a thumbnail for its chip.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use egit_agents::protocol::Image;
use egui::ColorImage;
use image::{DynamicImage, ImageFormat};

use crate::problem::Problem;

/// The most a file read to attach may hold.
const READ_LIMIT: u64 = 32 << 20;
/// The most an image may be, encoded as it is sent (the API's limit, on the base64 text).
const SENT_LIMIT: usize = 5 << 20;
/// The longest side the API takes; a longer one is scaled down to [`SCALED_SIDE`].
const LONGEST_SIDE: u32 = 8_000;
/// The longest side an image scaled down keeps: the most Claude looks at without scaling it
/// down itself.
const SCALED_SIDE: u32 = 1_568;
/// The most decoding may take, each way and in memory.
const DECODED_SIDE: u32 = 20_000;
const DECODED_BYTES: u64 = 512 << 20;
/// A thumbnail's longest side, in pixels.
const THUMBNAIL: u32 = 64;
/// The most images a prompt carries (the API's limit per request is higher; more than this
/// many at once is a mistake).
pub(crate) const MOST: usize = 20;

/// An image attached to a prompt.
pub(crate) struct Attachment {
    /// Its file's name.
    pub(crate) name: String,
    /// As it is sent.
    pub(crate) image: Arc<Image>,
    /// Its size as sent, in bytes (before base64).
    pub(crate) bytes: u64,
    /// Its size in pixels, as sent.
    pub(crate) size: [u32; 2],
    /// Whether it was scaled down to be sent.
    pub(crate) scaled: bool,
    /// A small picture of it, until its texture is made.
    pub(crate) thumbnail: Option<Arc<ColorImage>>,
    /// Its thumbnail's texture, once drawn.
    pub(crate) texture: Option<egui::TextureHandle>,
}

/// Whether `path` names an image Claude reads, by its extension (for files dropped on the
/// window: patches go elsewhere).
pub(crate) fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            ["png", "jpg", "jpeg", "gif", "webp"]
                .iter()
                .any(|known| extension.eq_ignore_ascii_case(known))
        })
}

/// Reads `paths` as attachments. Blocks: in a job. Fails on the first that can't be attached,
/// saying which and why.
pub(crate) fn read(paths: &[PathBuf]) -> Result<Vec<Attachment>, Problem> {
    paths
        .iter()
        .map(|path| {
            read_one(path).map_err(|why| {
                let name = path.file_name().map_or_else(
                    || path.display().to_string(),
                    |name| name.to_string_lossy().into_owned(),
                );
                Problem::new(format!("{name} couldn't be attached"), format!("{why}."))
            })
        })
        .collect()
}

/// Reads one image.
fn read_one(path: &Path) -> Result<Attachment, String> {
    let size = std::fs::metadata(path)
        .map_err(|error| error.to_string())?
        .len();
    if size > READ_LIMIT {
        return Err(format!(
            "it is {}: images up to {} are attached",
            crate::format::size(size),
            crate::format::size(READ_LIMIT)
        ));
    }
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    let name = path
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
    prepare(name, &bytes)
}

/// An attachment from an image's bytes: as it is when the API takes it, else scaled down.
fn prepare(name: String, bytes: &[u8]) -> Result<Attachment, String> {
    let format = image::guess_format(bytes).map_err(|_| not_readable())?;
    let media_type = match format {
        ImageFormat::Png => "image/png",
        ImageFormat::Jpeg => "image/jpeg",
        ImageFormat::Gif => "image/gif",
        ImageFormat::WebP => "image/webp",
        _ => return Err(not_readable()),
    };
    let decoded = decode(bytes)?;
    let thumbnail = Some(Arc::new(thumbnail(&decoded)));
    let fits = base64_len(bytes.len()) <= SENT_LIMIT
        && decoded.width() <= LONGEST_SIDE
        && decoded.height() <= LONGEST_SIDE;
    if fits {
        return Ok(Attachment {
            name,
            image: Arc::new(Image::of(media_type, bytes)),
            bytes: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
            size: [decoded.width(), decoded.height()],
            scaled: false,
            thumbnail,
            texture: None,
        });
    }
    let scaled = decoded.resize(
        SCALED_SIDE,
        SCALED_SIDE,
        image::imageops::FilterType::Triangle,
    );
    let (media_type, encoded) = encode_smaller(&scaled)?;
    Ok(Attachment {
        name,
        image: Arc::new(Image::of(media_type, &encoded)),
        bytes: u64::try_from(encoded.len()).unwrap_or(u64::MAX),
        size: [scaled.width(), scaled.height()],
        scaled: true,
        thumbnail,
        texture: None,
    })
}

/// Why an image isn't attached: its format.
fn not_readable() -> String {
    "it isn't an image Claude reads (PNG, JPEG, GIF, or WebP)".to_owned()
}

/// Decodes an image within bounds.
fn decode(bytes: &[u8]) -> Result<DynamicImage, String> {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(DECODED_SIDE);
    limits.max_image_height = Some(DECODED_SIDE);
    limits.max_alloc = Some(DECODED_BYTES);
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| error.to_string())?;
    reader.limits(limits);
    reader.decode().map_err(|error| error.to_string())
}

/// A scaled image encoded small enough to send: as a PNG, else (a photograph, say) a JPEG.
fn encode_smaller(scaled: &DynamicImage) -> Result<(&'static str, Vec<u8>), String> {
    let mut png = Vec::new();
    scaled
        .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
        .map_err(|error| error.to_string())?;
    if base64_len(png.len()) <= SENT_LIMIT {
        return Ok(("image/png", png));
    }
    let mut jpeg = Vec::new();
    DynamicImage::ImageRgb8(scaled.to_rgb8())
        .write_to(&mut Cursor::new(&mut jpeg), ImageFormat::Jpeg)
        .map_err(|error| error.to_string())?;
    if base64_len(jpeg.len()) <= SENT_LIMIT {
        return Ok(("image/jpeg", jpeg));
    }
    Err("it is too large to send, even scaled down".to_owned())
}

/// How long `bytes` bytes are in base64.
const fn base64_len(bytes: usize) -> usize {
    bytes.div_ceil(3).saturating_mul(4)
}

/// A thumbnail at most [`THUMBNAIL`] pixels a side (a smaller image as it is).
fn thumbnail(decoded: &DynamicImage) -> ColorImage {
    let small = if decoded.width() <= THUMBNAIL && decoded.height() <= THUMBNAIL {
        decoded.to_rgba8()
    } else {
        decoded.thumbnail(THUMBNAIL, THUMBNAIL).to_rgba8()
    };
    let size = [
        usize::try_from(small.width()).unwrap_or(0),
        usize::try_from(small.height()).unwrap_or(0),
    ];
    ColorImage::from_rgba_unmultiplied(size, small.as_raw())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An image of `width` by `height` pixels of noise, as `format`.
    fn picture(width: u32, height: u32, format: ImageFormat) -> Vec<u8> {
        let mut seed = 0x2545_f491_u32;
        let image = image::RgbImage::from_fn(width, height, |_, _| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let [r, g, b, _] = seed.to_le_bytes();
            image::Rgb([r, g, b])
        });
        let mut bytes = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut bytes), format)
            .unwrap();
        bytes
    }

    #[test]
    fn images_are_attached_as_they_are_when_they_fit() {
        let bytes = picture(30, 20, ImageFormat::Png);
        let attached = prepare("a.png".to_owned(), &bytes).unwrap();
        assert_eq!(attached.image.media_type, "image/png");
        assert_eq!(attached.size, [30, 20]);
        assert!(!attached.scaled);
        assert_eq!(attached.bytes, u64::try_from(bytes.len()).unwrap());
        let thumbnail = attached.thumbnail.unwrap();
        assert_eq!(thumbnail.size, [30, 20]);
        let jpeg = prepare("b.jpg".to_owned(), &picture(10, 10, ImageFormat::Jpeg)).unwrap();
        assert_eq!(jpeg.image.media_type, "image/jpeg");
    }

    #[test]
    fn large_images_are_scaled_down_and_others_refused() {
        // Noise doesn't compress: 2,000 × 1,200 pixels of it are over the limit as a PNG.
        let large = picture(2_000, 1_200, ImageFormat::Png);
        assert!(base64_len(large.len()) > SENT_LIMIT);
        let attached = prepare("large.png".to_owned(), &large).unwrap();
        assert!(attached.scaled);
        assert_eq!(attached.size, [SCALED_SIDE, 941]);
        assert!(attached.image.data.len() <= SENT_LIMIT);
        let thumbnail = attached.thumbnail.unwrap();
        assert_eq!(thumbnail.size, [64, 38]);

        let bmp = picture(4, 4, ImageFormat::Bmp);
        assert!(
            prepare("a.bmp".to_owned(), &bmp)
                .err()
                .is_some_and(|why| why.contains("PNG, JPEG"))
        );
        assert!(prepare("a.txt".to_owned(), b"hello").is_err());
        assert!(is_image(Path::new("shot.PNG")));
        assert!(is_image(Path::new("a/b.webp")));
        assert!(!is_image(Path::new("fix.patch")));
    }
}
