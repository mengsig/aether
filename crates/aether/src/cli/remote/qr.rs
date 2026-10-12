//! The pairing URL as a QR code a phone camera reads off the terminal.

use qrcode::render::unicode::Dense1x2;
use qrcode::{EcLevel, QrCode};

/// Longest URL rendered. A pairing URL is an origin and a short code; more
/// than this would not fit an 80-column terminal anyway.
const MAX_URL_BYTES: usize = 512;

/// `url` as half-block rows, dark on light with a quiet zone, so it scans
/// on a dark terminal as well as a light one.
pub(super) fn render(url: &str) -> anyhow::Result<String> {
    anyhow::ensure!(!url.is_empty(), "there is no pairing URL to show");
    anyhow::ensure!(
        url.len() <= MAX_URL_BYTES,
        "the pairing URL is {} bytes; at most {MAX_URL_BYTES} fit a terminal",
        url.len()
    );
    let code = QrCode::with_error_correction_level(url.as_bytes(), EcLevel::L)
        .map_err(|e| anyhow::anyhow!("could not encode the pairing URL as a QR code: {e}"))?;
    Ok(code
        .render::<Dense1x2>()
        .dark_color(Dense1x2::Light)
        .light_color(Dense1x2::Dark)
        .quiet_zone(true)
        .build())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pairing_url_renders_as_a_square_that_fits_a_terminal() {
        let rendered = render("https://box.tail1234.ts.net/pair#code=7K2M-9QXD").expect("renders");
        let rows: Vec<&str> = rendered.lines().collect();
        let width = rows[0].chars().count();
        assert!(rows.iter().all(|row| row.chars().count() == width));
        assert!(width <= 80, "{width} columns");
        // Two modules per character row, so the block is about half as tall.
        assert!(rows.len() * 2 >= width - 1 && rows.len() * 2 <= width + 1);
    }

    #[test]
    fn an_empty_or_oversized_url_is_refused() {
        assert!(render("").is_err());
        assert!(render(&"x".repeat(MAX_URL_BYTES + 1)).is_err());
    }
}
