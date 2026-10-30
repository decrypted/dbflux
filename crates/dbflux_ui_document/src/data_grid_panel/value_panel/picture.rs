//! Images held by a cell: recognizing them, checking they are whole, and
//! decoding them within fixed bounds.
//!
//! A cell is recognized by its leading magic bytes only, never by its column
//! name or type: a `bytea` of PNGs and a `BLOB` of anything else are told
//! apart by what they hold. Everything here is pure; the value panel runs the
//! decode on the background executor.
//!
//! SVG is left out on purpose. gpui rasterizes it at its declared size, so a
//! tiny document claiming a huge canvas would allocate without bound, and its
//! usvg options resolve `<image href>` against the local filesystem.

use base64::Engine as _;
use gpui::RenderImage;
use image::metadata::Orientation;
use image::{DynamicImage, Frame, ImageDecoder, ImageError, ImageReader, Limits};
use std::io::Cursor;
use std::sync::Arc;

/// Encoded size above which a cell is not decoded. The bytes are already in
/// memory, but decoding them costs time and several times their size again.
pub(crate) const MAX_IMAGE_BYTES: usize = 32 * 1024 * 1024;

/// Pixel count above which a cell is not decoded: the RGBA buffer alone
/// would pass 128 MiB. Checked from the header, before any pixel is read,
/// so a decompression bomb is refused for the price of its header.
pub(crate) const MAX_IMAGE_MEGAPIXELS: u64 = 32;

/// What the decoder may allocate at once. It covers 16-bit channels at the
/// pixel cap; decoders that track their allocations stop at it instead of
/// running the app out of memory.
const MAX_DECODE_ALLOC: u64 = 320 * 1024 * 1024;

/// Longest edge uploaded to the GPU. The panel is far narrower, and a
/// smaller texture stays inside every GPU's size limit.
const MAX_DISPLAY_EDGE: u32 = 4096;

/// How many bytes of a binary cell the hex view prints. A hex dump is about
/// seven times the size of its input, so a large blob is cut rather than
/// handed to the editor whole.
pub(crate) const MAX_HEX_BYTES: usize = 64 * 1024;

/// A raster format the panel can draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ImageKind {
    Png,
    Jpeg,
    Gif,
    WebP,
    Bmp,
}

impl ImageKind {
    /// The format `bytes` start with, if they start like an image.
    pub(crate) fn sniff(bytes: &[u8]) -> Option<Self> {
        const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        const JPEG: &[u8] = &[0xFF, 0xD8, 0xFF];

        if bytes.starts_with(PNG) {
            Some(Self::Png)
        } else if bytes.starts_with(JPEG) {
            Some(Self::Jpeg)
        } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
            Some(Self::Gif)
        } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP".as_slice()) {
            Some(Self::WebP)
        } else if is_bmp_header(bytes) {
            Some(Self::Bmp)
        } else {
            None
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Png => "PNG",
            Self::Jpeg => "JPEG",
            Self::Gif => "GIF",
            Self::WebP => "WebP",
            Self::Bmp => "BMP",
        }
    }

    pub(crate) fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Gif => "gif",
            Self::WebP => "webp",
            Self::Bmp => "bmp",
        }
    }

    pub(crate) fn gpui_format(self) -> gpui::ImageFormat {
        match self {
            Self::Png => gpui::ImageFormat::Png,
            Self::Jpeg => gpui::ImageFormat::Jpeg,
            Self::Gif => gpui::ImageFormat::Gif,
            Self::WebP => gpui::ImageFormat::Webp,
            Self::Bmp => gpui::ImageFormat::Bmp,
        }
    }

    fn decoder_format(self) -> image::ImageFormat {
        match self {
            Self::Png => image::ImageFormat::Png,
            Self::Jpeg => image::ImageFormat::Jpeg,
            Self::Gif => image::ImageFormat::Gif,
            Self::WebP => image::ImageFormat::WebP,
            Self::Bmp => image::ImageFormat::Bmp,
        }
    }

    /// Whether `bytes` run to the end of the image.
    ///
    /// A value cut short by a column or driver size limit still starts with
    /// valid magic, and some decoders fill the missing rows with gray rather
    /// than fail, so the end of the format is checked before decoding.
    pub(crate) fn is_complete(self, bytes: &[u8]) -> bool {
        match self {
            Self::Png => rfind(bytes, b"IEND").is_some_and(|at| at + 8 <= bytes.len()),
            // The last scan has to be followed by an end-of-image marker; an
            // EXIF thumbnail ends earlier, before the main image's scans.
            Self::Jpeg => rfind(bytes, &[0xFF, 0xDA])
                .and_then(|scan| bytes.get(scan..))
                .is_some_and(|tail| find(tail, &[0xFF, 0xD9]).is_some()),
            Self::Gif => bytes.iter().rev().find(|byte| **byte != 0) == Some(&0x3B),
            Self::WebP => le_u32(bytes, 4).is_some_and(|size| size as usize + 8 <= bytes.len()),
            Self::Bmp => match le_u32(bytes, 2) {
                Some(0) | None => {
                    le_u32(bytes, 10).is_some_and(|data| (data as usize) < bytes.len())
                }
                Some(size) => size as usize <= bytes.len(),
            },
        }
    }
}

/// "BM" alone is two printable letters, so the DIB header size that follows
/// the file header must also be one of the sizes BMP writers emit.
fn is_bmp_header(bytes: &[u8]) -> bool {
    bytes.starts_with(b"BM")
        && le_u32(bytes, 14).is_some_and(|size| matches!(size, 12 | 40 | 52 | 56 | 64 | 108 | 124))
}

fn le_u32(bytes: &[u8], at: usize) -> Option<u32> {
    let slice: [u8; 4] = bytes.get(at..at + 4)?.try_into().ok()?;
    Some(u32::from_le_bytes(slice))
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn rfind(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .rposition(|window| window == needle)
}

/// Why a cell that starts like an image is not drawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ImageRefusal {
    TooManyBytes {
        len: usize,
    },
    TooManyPixels {
        width: u32,
        height: u32,
    },
    /// The decoder asked for more memory than [`MAX_DECODE_ALLOC`].
    TooMuchMemory,
    Incomplete,
    Invalid(String),
}

impl ImageRefusal {
    /// The notice the panel shows above the hex view in place of the image.
    pub(crate) fn message(&self, kind: ImageKind) -> String {
        let format = kind.label();
        match self {
            Self::TooManyBytes { len } => dbflux_i18n::t!(
                "components.value_panel.image.too_many_bytes",
                format = format,
                size = crate::buckets_table::format_bytes(*len as u64),
                limit = crate::buckets_table::format_bytes(MAX_IMAGE_BYTES as u64)
            ),
            Self::TooManyPixels { width, height } => dbflux_i18n::t!(
                "components.value_panel.image.too_many_pixels",
                format = format,
                width = width,
                height = height,
                limit = MAX_IMAGE_MEGAPIXELS
            ),
            Self::TooMuchMemory => {
                dbflux_i18n::t!(
                    "components.value_panel.image.too_much_memory",
                    format = format
                )
            }
            Self::Incomplete => {
                dbflux_i18n::t!("components.value_panel.image.incomplete", format = format)
            }
            Self::Invalid(error) => dbflux_i18n::t!(
                "components.value_panel.image.invalid",
                format = format,
                error = error
            ),
        }
    }
}

/// A decoded image ready to draw, with the size it was stored at.
#[derive(Clone, Debug)]
pub(crate) struct DecodedImage {
    /// BGRA pixels, scaled down to [`MAX_DISPLAY_EDGE`] when larger.
    pub image: Arc<RenderImage>,
    pub width: u32,
    pub height: u32,
}

/// Refuses values the panel must not decode, without reading their pixels.
pub(crate) fn check_size(byte_len: usize) -> Result<(), ImageRefusal> {
    if byte_len > MAX_IMAGE_BYTES {
        return Err(ImageRefusal::TooManyBytes { len: byte_len });
    }
    Ok(())
}

fn check_pixels(width: u32, height: u32) -> Result<(), ImageRefusal> {
    if u64::from(width) * u64::from(height) > MAX_IMAGE_MEGAPIXELS * 1024 * 1024 {
        return Err(ImageRefusal::TooManyPixels { width, height });
    }
    Ok(())
}

/// Decodes `bytes` as a `kind` image, refusing it when it is too large,
/// truncated or unreadable. Slow on large images; run it off the UI thread.
///
/// The panel draws the decoded pixels itself rather than handing gpui the
/// encoded bytes, which gpui would decode again with no limits, every frame
/// of an animated GIF included. Animated images show their first frame.
pub(crate) fn decode_image(kind: ImageKind, bytes: &[u8]) -> Result<DecodedImage, ImageRefusal> {
    check_size(bytes.len())?;

    if !kind.is_complete(bytes) {
        return Err(ImageRefusal::Incomplete);
    }

    let mut limits = Limits::default();
    limits.max_alloc = Some(MAX_DECODE_ALLOC);

    let mut reader = ImageReader::with_format(Cursor::new(bytes), kind.decoder_format());
    reader.limits(limits);

    let mut decoder = reader.into_decoder().map_err(refusal)?;
    let (width, height) = decoder.dimensions();
    check_pixels(width, height)?;

    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut image = DynamicImage::from_decoder(decoder).map_err(refusal)?;
    image.apply_orientation(orientation);

    let (width, height) = (image.width(), image.height());
    if width.max(height) > MAX_DISPLAY_EDGE {
        image = image.thumbnail(MAX_DISPLAY_EDGE, MAX_DISPLAY_EDGE);
    }

    let mut pixels = image.into_rgba8();
    // gpui uploads sprites as BGRA.
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }

    Ok(DecodedImage {
        image: Arc::new(RenderImage::new(vec![Frame::new(pixels)])),
        width,
        height,
    })
}

fn refusal(error: ImageError) -> ImageRefusal {
    match error {
        ImageError::Limits(_) => ImageRefusal::TooMuchMemory,
        other => ImageRefusal::Invalid(other.to_string()),
    }
}

/// The image a text cell spells out in base64, either as a
/// `data:image/...;base64,` URI or as bare base64.
///
/// Only the first few characters are decoded to look for a magic, so
/// ordinary text costs next to nothing. An image whose text passes the size
/// cap keeps just that prefix, enough for the panel to name its format and
/// refuse it; `total_len` says how large it is.
pub(crate) fn image_from_base64_text(text: &str) -> Option<CellBytes> {
    // Enough characters for 18 bytes, which covers the BMP check at 14..18.
    const PREFIX_CHARS: usize = 24;

    let text = text.trim();
    let payload = match text.strip_prefix("data:") {
        Some(rest) => {
            let (media, data) = rest.split_once(',')?;
            let media = media.to_ascii_lowercase();
            if !media.starts_with("image/") || !media.ends_with(";base64") {
                return None;
            }
            data
        }
        None => text,
    };

    let head: String = payload
        .chars()
        .filter(|character| !character.is_ascii_whitespace())
        .take(PREFIX_CHARS)
        .collect();
    if head.len() < PREFIX_CHARS {
        return None;
    }

    let engine = base64::engine::general_purpose::STANDARD;
    let prefix = engine.decode(&head).ok()?;
    ImageKind::sniff(&prefix)?;

    let total_len = payload.len() / 4 * 3;
    if check_size(total_len).is_err() {
        return Some(CellBytes {
            bytes: prefix.into(),
            total_len,
        });
    }

    let compact: String = payload
        .chars()
        .filter(|character| !character.is_ascii_whitespace())
        .collect();
    let bytes = engine.decode(compact).ok()?;
    Some(CellBytes::new(&bytes))
}

/// The raw bytes of a cell, as much of them as the panel may use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CellBytes {
    /// Every byte when the value is within [`MAX_IMAGE_BYTES`], otherwise
    /// only what the hex view prints: the panel copies the value each time
    /// the cursor moves, and nothing past the cap is ever decoded or saved.
    pub bytes: Arc<[u8]>,
    pub total_len: usize,
}

impl CellBytes {
    pub(crate) fn new(value: &[u8]) -> Self {
        let kept = if check_size(value.len()).is_ok() {
            value
        } else {
            value.get(..MAX_HEX_BYTES).unwrap_or(value)
        };

        Self {
            bytes: kept.into(),
            total_len: value.len(),
        }
    }

    /// The hex dump the panel shows for these bytes, cut at
    /// [`MAX_HEX_BYTES`].
    pub(crate) fn hex_dump(&self) -> String {
        let shown = self.bytes.get(..MAX_HEX_BYTES).unwrap_or(&self.bytes);
        crate::data_grid_panel::render::format_hex_dump(shown)
    }

    /// Whether the hex view prints only the start of the value.
    pub(crate) fn hex_is_cut(&self) -> bool {
        self.total_len > MAX_HEX_BYTES
    }
}

#[cfg(test)]
pub(super) mod tests;
