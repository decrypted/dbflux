use super::{
    CellBytes, ImageKind, ImageRefusal, MAX_HEX_BYTES, MAX_IMAGE_BYTES, check_size, decode_image,
    image_from_base64_text,
};
use base64::Engine as _;
use std::io::Cursor;

const ALL: [ImageKind; 5] = [
    ImageKind::Png,
    ImageKind::Jpeg,
    ImageKind::Gif,
    ImageKind::WebP,
    ImageKind::Bmp,
];

/// A real `width` × `height` image of `kind`, written by the `image` crate.
pub(crate) fn encoded(kind: ImageKind, width: u32, height: u32) -> Vec<u8> {
    let image = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(width, height, |x, y| {
        image::Rgb([(x * 40) as u8, (y * 40) as u8, 128])
    }));
    let mut bytes = Cursor::new(Vec::new());
    image
        .write_to(&mut bytes, kind.decoder_format())
        .expect("the image crate encodes every supported format");
    bytes.into_inner()
}

#[test]
fn each_format_is_recognized_by_its_magic_bytes() {
    for kind in ALL {
        assert_eq!(
            ImageKind::sniff(&encoded(kind, 3, 2)),
            Some(kind),
            "{kind:?}"
        );
    }
}

#[test]
fn bytes_that_are_not_an_image_are_not_recognized() {
    let mut noise = Vec::new();
    let mut seed = 0x2545_F491_u32;
    for _ in 0..512 {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        noise.push(seed as u8);
    }

    for bytes in [
        noise.as_slice(),
        b"",
        b"\x89PN",
        b"plain text in a blob column",
        // Starts with "BM" like a bitmap, but no DIB header follows.
        b"BMW of the month: a car, not a bitmap",
        b"RIFF\x24\x00\x00\x00WAVEfmt ",
        b"%PDF-1.7",
    ] {
        assert_eq!(ImageKind::sniff(bytes), None, "{bytes:?}");
    }
}

#[test]
fn each_format_decodes_with_its_stored_size() {
    for kind in ALL {
        let decoded = decode_image(kind, &encoded(kind, 3, 2)).expect("a whole image decodes");
        assert_eq!((decoded.width, decoded.height), (3, 2), "{kind:?}");
    }
}

/// A value cut short by a size limit starts like the image but ends early;
/// the panel says so instead of drawing a partial picture.
#[test]
fn a_cut_image_is_reported_as_incomplete() {
    for kind in ALL {
        let bytes = encoded(kind, 24, 24);
        let cut = &bytes[..bytes.len() * 2 / 3];

        assert!(kind.is_complete(&bytes), "{kind:?} whole");
        assert!(!kind.is_complete(cut), "{kind:?} cut");
        assert_eq!(
            decode_image(kind, cut).unwrap_err(),
            ImageRefusal::Incomplete,
            "{kind:?}"
        );
    }
}

#[test]
fn trailing_zero_padding_keeps_a_gif_whole() {
    let mut bytes = encoded(ImageKind::Gif, 4, 4);
    bytes.extend_from_slice(&[0, 0, 0]);

    assert!(ImageKind::Gif.is_complete(&bytes));
}

#[test]
fn an_image_above_the_byte_cap_is_refused_before_decoding() {
    let refusal = check_size(MAX_IMAGE_BYTES + 1).unwrap_err();

    assert_eq!(
        refusal,
        ImageRefusal::TooManyBytes {
            len: MAX_IMAGE_BYTES + 1
        }
    );
    assert!(refusal.message(ImageKind::Png).contains("PNG"));
}

/// A small file that declares a huge canvas is refused from its header, so
/// a decompression bomb never gets its pixels allocated.
#[test]
fn an_image_above_the_pixel_cap_is_refused_from_its_header() {
    let mut bytes = encoded(ImageKind::Gif, 2, 2);
    // The logical screen size of a GIF sits at 6..10, little endian, and
    // carries no checksum.
    bytes[6..8].copy_from_slice(&8000u16.to_le_bytes());
    bytes[8..10].copy_from_slice(&8000u16.to_le_bytes());

    assert_eq!(
        decode_image(ImageKind::Gif, &bytes).unwrap_err(),
        ImageRefusal::TooManyPixels {
            width: 8000,
            height: 8000
        }
    );
}

#[test]
fn a_large_cell_keeps_only_what_the_hex_view_prints() {
    let value = vec![0xAB; MAX_IMAGE_BYTES + 10];
    let cell = CellBytes::new(&value);

    assert_eq!(cell.bytes.len(), MAX_HEX_BYTES);
    assert_eq!(cell.total_len, MAX_IMAGE_BYTES + 10);
    assert!(cell.hex_is_cut());
}

#[test]
fn base64_text_holding_an_image_is_recognized() {
    let png = encoded(ImageKind::Png, 2, 2);
    let base64 = base64::engine::general_purpose::STANDARD.encode(&png);

    for text in [
        format!("data:image/png;base64,{base64}"),
        base64.clone(),
        // Wrapped at 76 columns, as `encode(..., 'base64')` writes it.
        base64
            .as_bytes()
            .chunks(76)
            .map(|line| std::str::from_utf8(line).unwrap())
            .collect::<Vec<_>>()
            .join("\n"),
    ] {
        let cell = image_from_base64_text(&text).expect("the text holds a PNG");
        assert_eq!(cell.bytes.as_ref(), png.as_slice());
    }
}

#[test]
fn ordinary_text_is_not_taken_for_a_base64_image() {
    let text_base64 = base64::engine::general_purpose::STANDARD
        .encode(b"just some text that happens to be base64 encoded");

    for text in [
        "hello world, this is a sentence",
        r#"{"id": 1, "name": "image.png"}"#,
        "data:text/plain;base64,aGVsbG8gd29ybGQgdGhpcyBpcyB0ZXh0",
        text_base64.as_str(),
    ] {
        assert_eq!(image_from_base64_text(text), None, "{text}");
    }
}
