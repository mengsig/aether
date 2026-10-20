//! Image pixels retain their source colours independently of UI theme tokens.

use ratatui::style::{Color, Style};

pub(crate) fn half_block(top: [u8; 4], bottom: Option<[u8; 4]>) -> Style {
    Style::default()
        .fg(composite(top))
        .bg(bottom.map(composite).unwrap_or(Color::Black))
}

fn composite([red, green, blue, alpha]: [u8; 4]) -> Color {
    let [red, green, blue] = [red, green, blue]
        .map(|channel| ((u32::from(channel) * u32::from(alpha) + 127) / 255) as u8);
    Color::Rgb(red, green, blue)
}
