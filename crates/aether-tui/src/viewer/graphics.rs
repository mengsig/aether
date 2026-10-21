//! Bounded image decoding and terminal graphics prepared outside the paint loop.

use std::io::{Cursor, Read};
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use base64::Engine;
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, RgbaImage};
use ratatui::text::{Line, Span};

use super::probe::Protocol;

const MAX_FILE_BYTES: u64 = 20 * 1024 * 1024;
const MAX_DIMENSION: u32 = 16_384;
const MAX_PIXELS: u64 = 8 * 1024 * 1024;
const MAX_DECODE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_RENDER_SIDE: u32 = 4096;
const MAX_RENDER_PIXELS: u64 = 1024 * 1024;
const MAX_PROTOCOL_BYTES: usize = 16 * 1024 * 1024;
const KITTY_DELETE: &[u8] = b"\x1b_Ga=d,d=I,i=44670,q=2\x1b\\";

#[derive(Debug)]
pub struct DecodedImage {
    pixels: RgbaImage,
}

impl DecodedImage {
    pub fn width(&self) -> u32 {
        self.pixels.width()
    }

    pub fn height(&self) -> u32 {
        self.pixels.height()
    }
}

/// Read a regular PNG, JPEG, GIF (first frame), or WebP with fixed resource limits.
pub fn load(path: &Path) -> Result<DecodedImage> {
    let path = path.canonicalize().context("resolve image")?;
    load_canonical(&path)
}

pub(super) fn load_canonical(path: &Path) -> Result<DecodedImage> {
    let file = super::files::open_canonical(path)?;
    let metadata = file.metadata().context("read image metadata")?;
    ensure!(metadata.is_file(), "image must be a regular file");
    ensure!(metadata.len() <= MAX_FILE_BYTES, "image exceeds 20 MiB");
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .context("read image bytes")?;
    ensure!(!bytes.is_empty(), "image is empty");
    ensure!(bytes.len() as u64 <= MAX_FILE_BYTES, "image exceeds 20 MiB");
    decode(&bytes).with_context(|| format!("decode image {}", path.display()))
}

pub(super) fn decode(bytes: &[u8]) -> Result<DecodedImage> {
    ensure!(!bytes.is_empty(), "image is empty");
    ensure!(bytes.len() as u64 <= MAX_FILE_BYTES, "image exceeds 20 MiB");
    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    ensure!(
        matches!(
            reader.format(),
            Some(ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Gif | ImageFormat::WebP)
        ),
        "unsupported image format (expected PNG, JPEG, GIF, or WebP)"
    );
    if reader.format() == Some(ImageFormat::WebP) {
        validate_webp(bytes)?;
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    reader.limits(limits);
    let decoder = reader.into_decoder().context("read image header")?;
    let (width, height) = decoder.dimensions();
    ensure!(width > 0 && height > 0, "image dimensions must be positive");
    ensure!(
        width <= MAX_DIMENSION && height <= MAX_DIMENSION,
        "image dimensions exceed 16384 pixels"
    );
    ensure!(
        u64::from(width) * u64::from(height) <= MAX_PIXELS,
        "image exceeds 8 megapixels"
    );
    ensure!(
        decoder.total_bytes() <= MAX_DECODE_BYTES,
        "image decode exceeds 64 MiB"
    );
    let pixels = DynamicImage::from_decoder(decoder)
        .context("decode first image frame")?
        .into_rgba8();
    assert_eq!(pixels.dimensions(), (width, height));
    assert_eq!(
        pixels.as_raw().len() as u64,
        u64::from(width) * u64::from(height) * 4
    );
    Ok(DecodedImage { pixels })
}

fn validate_webp(bytes: &[u8]) -> Result<()> {
    ensure!(bytes.len() >= 12, "truncated WebP header");
    ensure!(
        &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP",
        "invalid WebP header"
    );
    let length = u32::from_le_bytes(bytes[4..8].try_into()?) as usize;
    ensure!(
        length >= 4 && length <= bytes.len() - 8,
        "truncated WebP container"
    );
    validate_webp_chunks(&bytes[12..length + 8], true)
}

fn validate_webp_chunks(mut bytes: &[u8], allow_frames: bool) -> Result<()> {
    while !bytes.is_empty() {
        ensure!(bytes.len() >= 8, "truncated WebP chunk header");
        let tag = &bytes[..4];
        let length = u32::from_le_bytes(bytes[4..8].try_into()?) as usize;
        ensure!(length <= bytes.len() - 8, "truncated WebP chunk");
        let payload = &bytes[8..8 + length];
        match tag {
            b"ANMF" => {
                ensure!(allow_frames, "nested WebP animation frame");
                ensure!(payload.len() >= 16, "truncated WebP animation frame");
                // Recursion is limited to the container and one frame's chunk list.
                validate_webp_chunks(&payload[16..], false)?;
            }
            b"VP8 " | b"VP8L" => {
                let (width, height) = webp_bitstream_dimensions(tag, payload)?;
                ensure!(
                    width > 0 && height > 0,
                    "WebP frame dimensions must be positive"
                );
                ensure!(
                    width <= MAX_DIMENSION && height <= MAX_DIMENSION,
                    "WebP frame dimensions exceed 16384 pixels"
                );
                ensure!(
                    u64::from(width) * u64::from(height) <= MAX_PIXELS,
                    "WebP frame exceeds 8 megapixels"
                );
            }
            _ => {}
        }
        let padded = length + length % 2;
        ensure!(padded <= bytes.len() - 8, "missing WebP chunk padding");
        bytes = &bytes[8 + padded..];
    }
    Ok(())
}

fn webp_bitstream_dimensions(tag: &[u8], payload: &[u8]) -> Result<(u32, u32)> {
    assert!(tag == b"VP8 " || tag == b"VP8L");
    if tag == b"VP8 " {
        ensure!(payload.len() >= 10, "truncated WebP VP8 header");
        ensure!(
            &payload[3..6] == b"\x9d\x01\x2a",
            "invalid WebP VP8 signature"
        );
        let width = u16::from_le_bytes(payload[6..8].try_into()?) & 0x3fff;
        let height = u16::from_le_bytes(payload[8..10].try_into()?) & 0x3fff;
        return Ok((u32::from(width), u32::from(height)));
    }
    ensure!(payload.len() >= 5, "truncated WebP lossless header");
    ensure!(payload[0] == 0x2f, "invalid WebP lossless signature");
    let bits = u32::from_le_bytes(payload[1..5].try_into()?);
    Ok(((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1))
}

/// Two image pixels per cell; alpha is composited against a black background.
pub fn render_half_blocks(image: &DecodedImage, width: u16, height: u16) -> Vec<Line<'static>> {
    if width == 0 || height == 0 {
        return Vec::new();
    }
    let pixels = fit(image, u32::from(width), u32::from(height) * 2);
    let mut lines = Vec::with_capacity(pixels.height().div_ceil(2) as usize);
    for y in (0..pixels.height()).step_by(2) {
        let spans: Vec<_> = (0..pixels.width())
            .map(|x| {
                let top = pixels.get_pixel(x, y).0;
                let bottom = if y + 1 < pixels.height() {
                    Some(pixels.get_pixel(x, y + 1).0)
                } else {
                    None
                };
                Span::styled("▀", crate::ui::theme::image::half_block(top, bottom))
            })
            .collect();
        lines.push(Line::from(spans));
    }
    assert!(lines.len() <= usize::from(height));
    assert!(lines.iter().all(|line| line.width() <= usize::from(width)));
    lines
}

fn fit(image: &DecodedImage, max_width: u32, max_height: u32) -> RgbaImage {
    assert!(max_width > 0 && max_height > 0);
    assert!(image.width() > 0 && image.height() > 0);
    let max_width = max_width.min(MAX_RENDER_SIDE);
    let max_height = max_height.min(MAX_RENDER_SIDE);
    let scale = (f64::from(max_width) / f64::from(image.width()))
        .min(f64::from(max_height) / f64::from(image.height()));
    let mut width = (f64::from(image.width()) * scale).floor().max(1.0) as u32;
    let mut height = (f64::from(image.height()) * scale).floor().max(1.0) as u32;
    let pixels = u64::from(width) * u64::from(height);
    if pixels > MAX_RENDER_PIXELS {
        let scale = (MAX_RENDER_PIXELS as f64 / pixels as f64).sqrt();
        width = (f64::from(width) * scale).floor().max(1.0) as u32;
        height = (f64::from(height) * scale).floor().max(1.0) as u32;
    }
    assert!(width <= max_width && height <= max_height);
    assert!(u64::from(width) * u64::from(height) <= MAX_RENDER_PIXELS);
    // Nearest-neighbour avoids a large intermediate buffer for very tall sources.
    image::imageops::resize(
        &image.pixels,
        width,
        height,
        image::imageops::FilterType::Nearest,
    )
}

/// Encode a pane using measured cell dimensions in physical pixels.
/// The caller positions the cursor at the pane origin and restores it afterwards.
pub fn encode_protocol(
    image: &DecodedImage,
    protocol: Protocol,
    width: u16,
    height: u16,
    cell_width: u16,
    cell_height: u16,
) -> Result<Vec<u8>> {
    ensure!(width > 0 && height > 0, "image pane must be nonempty");
    ensure!(
        cell_width > 0 && cell_height > 0,
        "terminal cell dimensions are unavailable"
    );
    let mut pixel_height = u32::from(height) * u32::from(cell_height);
    if protocol == Protocol::Sixel {
        pixel_height = pixel_height.min(MAX_RENDER_SIDE) / 6 * 6;
        ensure!(pixel_height >= 6, "Sixel pane must fit one six-pixel band");
    }
    let pixels = fit(
        image,
        u32::from(width) * u32::from(cell_width),
        pixel_height,
    );
    let bytes = match protocol {
        Protocol::Kitty => encode_kitty(&pixels, cell_width, cell_height),
        Protocol::Iterm2 => encode_iterm2(&pixels)?,
        Protocol::Sixel => encode_sixel(&pixels),
        Protocol::HalfBlocks => bail!("half blocks are rendered as terminal cells"),
    };
    ensure!(
        bytes.len() <= MAX_PROTOCOL_BYTES,
        "encoded image exceeds 16 MiB"
    );
    assert!(pixels.width() <= u32::from(width) * u32::from(cell_width));
    assert!(pixels.height() <= u32::from(height) * u32::from(cell_height));
    Ok(bytes)
}

/// Delete only the image owned by this viewer, including terminal-side storage.
pub fn cleanup(protocol: Protocol) -> &'static [u8] {
    if protocol == Protocol::Kitty {
        KITTY_DELETE
    } else {
        b""
    }
}

fn encode_kitty(pixels: &RgbaImage, cell_width: u16, cell_height: u16) -> Vec<u8> {
    assert!(cell_width > 0 && cell_height > 0);
    assert!(!pixels.is_empty());
    let payload = base64::engine::general_purpose::STANDARD.encode(pixels.as_raw());
    let columns = pixels.width().div_ceil(u32::from(cell_width));
    let rows = pixels.height().div_ceil(u32::from(cell_height));
    let mut bytes = Vec::with_capacity(payload.len() + 1024);
    bytes.extend_from_slice(KITTY_DELETE);
    let chunks = payload.as_bytes().chunks(4096);
    let last = chunks.len() - 1;
    for (index, chunk) in chunks.enumerate() {
        let more = u8::from(index != last);
        let header = if index == 0 {
            format!(
                "\x1b_Ga=T,f=32,t=d,i=44670,q=2,C=1,s={},v={},c={columns},r={rows},m={more};",
                pixels.width(),
                pixels.height()
            )
        } else {
            format!("\x1b_Gm={more};")
        };
        bytes.extend_from_slice(header.as_bytes());
        bytes.extend_from_slice(chunk);
        bytes.extend_from_slice(b"\x1b\\");
    }
    assert!(bytes.ends_with(b"\x1b\\"));
    bytes
}

fn encode_iterm2(pixels: &RgbaImage) -> Result<Vec<u8>> {
    assert!(pixels.width() > 0 && pixels.height() > 0);
    assert!(u64::from(pixels.width()) * u64::from(pixels.height()) <= MAX_RENDER_PIXELS);
    let mut png = Cursor::new(Vec::new());
    pixels
        .write_to(&mut png, ImageFormat::Png)
        .context("encode image PNG")?;
    let payload = base64::engine::general_purpose::STANDARD.encode(png.get_ref());
    let header = format!(
        "\x1b]1337;File=inline=1;size={};width={}px;height={}px;preserveAspectRatio=1;doNotMoveCursor=1:",
        png.get_ref().len(),
        pixels.width(),
        pixels.height()
    );
    let mut bytes = header.into_bytes();
    bytes.extend_from_slice(payload.as_bytes());
    bytes.push(7);
    Ok(bytes)
}

fn encode_sixel(pixels: &RgbaImage) -> Vec<u8> {
    assert!(pixels.width() > 0 && pixels.height() > 0);
    assert!(u64::from(pixels.width()) * u64::from(pixels.height()) <= MAX_RENDER_PIXELS);
    let mut output =
        format!("\x1bP0;1;0q\"1;1;{};{}", pixels.width(), pixels.height()).into_bytes();
    for color in 0..64 {
        let [red, green, blue] =
            [color >> 4, (color >> 2) & 3, color & 3].map(|value| value * 100 / 3);
        output.extend_from_slice(format!("#{color};2;{red};{green};{blue}").as_bytes());
    }
    for y in (0..pixels.height()).step_by(6) {
        if y > 0 {
            output.push(b'-');
        }
        sixel_band(&mut output, pixels, y);
    }
    output.extend_from_slice(b"\x1b\\");
    output
}

fn sixel_band(output: &mut Vec<u8>, pixels: &RgbaImage, y: u32) {
    assert!(y < pixels.height());
    assert_eq!(y % 6, 0);
    let mut columns = vec![[0u8; 64]; pixels.width() as usize];
    let mut used = 0u64;
    for offset in 0..6.min(pixels.height() - y) {
        for x in 0..pixels.width() {
            let pixel = *pixels.get_pixel(x, y + offset);
            let alpha = u32::from(pixel[3]);
            let [red, green, blue] = [pixel[0], pixel[1], pixel[2]]
                .map(|channel| ((u32::from(channel) * alpha + 127) / 255) as u8 >> 6);
            let color = usize::from((red << 4) | (green << 2) | blue);
            columns[x as usize][color] |= 1 << offset;
            used |= 1 << color;
        }
    }
    let mut first = true;
    for color in 0..64 {
        if used & (1 << color) == 0 {
            continue;
        }
        if !first {
            output.push(b'$');
        }
        first = false;
        output.extend_from_slice(format!("#{color}").as_bytes());
        sixel_runs(output, columns.iter().map(|column| column[color] + 63));
    }
}

fn sixel_runs(output: &mut Vec<u8>, bytes: impl Iterator<Item = u8>) {
    let mut bytes = bytes.peekable();
    while let Some(byte) = bytes.next() {
        debug_assert!((63..=126).contains(&byte));
        let mut count = 1;
        while bytes.peek() == Some(&byte) {
            bytes.next();
            count += 1;
        }
        debug_assert!(count <= MAX_RENDER_SIDE);
        if count >= 4 {
            output.extend_from_slice(format!("!{count}").as_bytes());
            output.push(byte);
        } else {
            output.extend(std::iter::repeat_n(byte, count as usize));
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use image::ImageEncoder;
    use image::Rgba;
    use ratatui::style::{Color, Style};

    fn fixture(width: u32, height: u32, color: [u8; 4]) -> DecodedImage {
        DecodedImage {
            pixels: RgbaImage::from_pixel(width, height, Rgba(color)),
        }
    }

    #[test]
    fn half_blocks_golden_colors_alpha_and_odd_height() {
        let mut image = fixture(1, 3, [255, 0, 0, 255]);
        image.pixels.put_pixel(0, 1, Rgba([0, 255, 0, 128]));
        image.pixels.put_pixel(0, 2, Rgba([0, 0, 255, 255]));
        let lines = render_half_blocks(&image, 1, 2);
        assert_eq!(lines.len(), 2);
        assert_eq!(
            lines[0].spans,
            vec![Span::styled(
                "▀",
                Style::default()
                    .fg(Color::Rgb(255, 0, 0))
                    .bg(Color::Rgb(0, 128, 0))
            )]
        );
        assert_eq!(
            lines[1].spans,
            vec![Span::styled(
                "▀",
                Style::default().fg(Color::Rgb(0, 0, 255)).bg(Color::Black)
            )]
        );
    }

    #[test]
    fn half_blocks_fit_pane_and_handle_empty_pane() {
        let image = fixture(40, 10, [0, 0, 0, 255]);
        let lines = render_half_blocks(&image, 8, 8);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].width(), 8);
        assert!(render_half_blocks(&image, 0, 4).is_empty());
        assert!(render_half_blocks(&image, 4, 0).is_empty());
        let pixels = fit(&fixture(1, 1, [0; 4]), u32::MAX, u32::MAX);
        assert!(u64::from(pixels.width()) * u64::from(pixels.height()) <= MAX_RENDER_PIXELS);
    }

    #[test]
    fn supported_formats_decode_and_gif_uses_first_frame() {
        let source = fixture(2, 2, [255, 0, 0, 255]);
        for format in [ImageFormat::Png, ImageFormat::Jpeg, ImageFormat::WebP] {
            let mut bytes = Cursor::new(Vec::new());
            DynamicImage::ImageRgba8(source.pixels.clone())
                .into_rgb8()
                .write_to(&mut bytes, format)
                .unwrap();
            let decoded = decode(bytes.get_ref()).unwrap();
            assert_eq!((decoded.width(), decoded.height()), (2, 2));
            assert_eq!(decoded.pixels.as_raw().len(), 16);
        }
        let mut bytes = Vec::new();
        let frames = [source, fixture(2, 2, [0, 255, 0, 255])]
            .into_iter()
            .map(|image| image::Frame::new(image.pixels));
        image::codecs::gif::GifEncoder::new(&mut bytes)
            .encode_frames(frames)
            .unwrap();
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded.pixels.get_pixel(0, 0), &Rgba([255, 0, 0, 255]));
    }

    #[test]
    fn invalid_large_and_nonregular_images_are_rejected() {
        assert!(decode(b"not an image").is_err());
        assert!(decode(&[]).is_err());
        let file = tempfile::NamedTempFile::new().unwrap();
        file.as_file().set_len(MAX_FILE_BYTES + 1).unwrap();
        assert!(
            load(file.path())
                .unwrap_err()
                .to_string()
                .contains("20 MiB")
        );
        assert!(load(tempfile::tempdir().unwrap().path()).is_err());
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(
                &vec![0; (MAX_DIMENSION as usize + 1) * 4],
                MAX_DIMENSION + 1,
                1,
                image::ExtendedColorType::Rgba8,
            )
            .unwrap();
        assert!(decode(&bytes).is_err());
    }

    #[test]
    fn webp_embedded_dimensions_and_truncated_chunks_are_rejected_before_decode() {
        let mut chunks = b"VP8X\x0a\0\0\0".to_vec();
        chunks.extend_from_slice(&[0; 10]);
        chunks.extend_from_slice(b"VP8L\x05\0\0\0\x2f");
        chunks.extend_from_slice(&(4095u32 | (4095 << 14)).to_le_bytes());
        chunks.push(0);
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&(chunks.len() as u32 + 4).to_le_bytes());
        bytes.extend_from_slice(b"WEBP");
        bytes.extend_from_slice(&chunks);
        assert!(
            decode(&bytes)
                .unwrap_err()
                .to_string()
                .contains("8 megapixels")
        );
        bytes.pop();
        assert!(
            validate_webp(&bytes)
                .unwrap_err()
                .to_string()
                .contains("truncated")
        );
        assert!(
            validate_webp_chunks(b"ANMF\x10\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0", false).is_err()
        );
    }

    #[test]
    fn protocol_encoders_are_bounded_and_cleanup_is_scoped() {
        let image = fixture(80, 80, [255, 0, 0, 255]);
        let kitty =
            String::from_utf8(encode_protocol(&image, Protocol::Kitty, 2, 2, 8, 16).unwrap())
                .unwrap();
        assert!(kitty.contains("s=16,v=16,c=2,r=1"));
        assert!(kitty.contains("m=0;"));
        let sixel =
            String::from_utf8(encode_protocol(&image, Protocol::Sixel, 2, 2, 8, 16).unwrap())
                .unwrap();
        assert!(sixel.starts_with("\x1bP0;1;0q\"1;1;16;16"));
        assert_eq!(sixel.matches('-').count(), 2);
        assert!(sixel.ends_with("#48!16N\x1b\\"));
        let iterm =
            String::from_utf8(encode_protocol(&image, Protocol::Iterm2, 2, 2, 8, 16).unwrap())
                .unwrap();
        assert!(iterm.contains("width=16px;height=16px"));
        let png = base64::engine::general_purpose::STANDARD
            .decode(iterm.split_once(':').unwrap().1.trim_end_matches('\x07'))
            .unwrap();
        assert_eq!(decode(&png).unwrap().pixels.dimensions(), (16, 16));
        assert_eq!(cleanup(Protocol::Kitty), KITTY_DELETE);
        assert!(cleanup(Protocol::Sixel).is_empty());
    }

    #[test]
    fn invalid_panes_and_missing_cell_dimensions_fail() {
        let image = fixture(1, 1, [0; 4]);
        for (width, height, cell_width, cell_height) in
            [(0, 1, 8, 16), (1, 0, 8, 16), (1, 1, 0, 16), (1, 1, 8, 0)]
        {
            assert!(
                encode_protocol(
                    &image,
                    Protocol::Kitty,
                    width,
                    height,
                    cell_width,
                    cell_height
                )
                .is_err()
            );
        }
        assert!(encode_protocol(&image, Protocol::HalfBlocks, 1, 1, 8, 16).is_err());
        assert!(encode_protocol(&image, Protocol::Sixel, 1, 1, 8, 5).is_err());
    }

    #[test]
    fn kitty_chunks_have_protocol_sized_payloads() {
        let image = fixture(80, 80, [255; 4]);
        let bytes = encode_protocol(&image, Protocol::Kitty, 10, 5, 8, 16).unwrap();
        let text = std::str::from_utf8(&bytes).unwrap();
        let mut joined = String::new();
        for command in text.split("\x1b_G").skip(2) {
            let (_, payload) = command.split_once(';').unwrap();
            let payload = payload.strip_suffix("\x1b\\").unwrap();
            assert!(payload.len() <= 4096);
            joined.push_str(payload);
        }
        assert!(text.contains("\x1b_Gm=1;"));
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(joined)
                .unwrap(),
            image.pixels.into_raw()
        );
    }
}
