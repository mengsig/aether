use super::{Documents, graphics, probe::Protocol, references};
use anyhow::{Context, bail, ensure};
use ratatui::text::Line;
use std::io::Read;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub struct ReadRequest {
    pub id: u64,
    pub target: String,
    pub workspace: PathBuf,
    pub approved_outside: Option<PathBuf>,
    pub width: u16,
    pub height: u16,
    pub protocol: Protocol,
    pub cell_pixels: (u16, u16),
    pub theme: crate::ui::theme::Theme,
    /// One-based PDF page; the worker clamps it to the document.
    pub page: u32,
    pub view: View,
}

/// Which form of a PDF or HTML file to show; `Auto` is a PDF image or HTML text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum View {
    #[default]
    Auto,
    Text,
    Rendered,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pages {
    pub current: u32,
    pub total: u32,
}

#[derive(Debug)]
pub enum Preview {
    Text {
        path: PathBuf,
        rows: Vec<Line<'static>>,
        pages: Option<Pages>,
    },
    Image {
        path: PathBuf,
        rows: Vec<Line<'static>>,
        bytes: Vec<u8>,
        protocol: Protocol,
        pages: Option<Pages>,
    },
    Outside(PathBuf),
    Unsupported(PathBuf),
    Failed {
        path: PathBuf,
        error: String,
    },
}

pub fn read(request: &ReadRequest, documents: &mut Documents) -> anyhow::Result<Preview> {
    ensure!(request.width > 0 && request.height > 0, "pane is too small");
    let workspace = request
        .workspace
        .canonicalize()
        .context("resolve agent workspace")?;
    ensure!(workspace.is_dir(), "agent workspace is not a directory");
    let path = references::resolve(&request.target, &workspace)?;
    if !path.starts_with(&workspace) && request.approved_outside.as_ref() != Some(&path) {
        return Ok(Preview::Outside(path));
    }
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let result = match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" => graphics::load_canonical(&path)
            .and_then(|image| picture(path.clone(), &image, request, None)),
        "pdf" => super::pdf::preview(path.clone(), request, documents),
        "html" | "htm" => super::html::preview(path.clone(), request, documents),
        "svg" => Ok(Preview::Unsupported(path.clone())),
        _ => text(path.clone(), ext == "md" || ext == "markdown", request),
    };
    Ok(
        result.unwrap_or_else(|error: anyhow::Error| Preview::Failed {
            path,
            error: format!("{error:#}"),
        }),
    )
}

fn text(path: PathBuf, markdown: bool, request: &ReadRequest) -> anyhow::Result<Preview> {
    const LIMIT: u64 = 1024 * 1024;
    let file = super::files::open_canonical(&path).context("open preview")?;
    ensure!(file.metadata()?.is_file(), "preview needs a regular file");
    let mut bytes = Vec::new();
    file.take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .context("read preview")?;
    ensure!(bytes.len() as u64 <= LIMIT, "text preview exceeds 1 MiB");
    if bytes.contains(&0) {
        bail!("binary file: use Alt+E to open externally");
    }
    let text = String::from_utf8(bytes).context("text preview needs UTF-8")?;
    let rows = text_rows(&text, markdown, request);
    Ok(Preview::Text {
        path,
        rows,
        pages: None,
    })
}

/// Clean and lay out at most 64 KiB / 4,000 lines, noting any truncation.
pub(super) fn text_rows(text: &str, markdown: bool, request: &ReadRequest) -> Vec<Line<'static>> {
    let clean = crate::ui::cells::text::clean(text);
    let mut end = clean.len().min(64 * 1024);
    while !clean.is_char_boundary(end) {
        end -= 1;
    }
    let mut lines = clean[..end].lines();
    let limited = lines.by_ref().take(4000).collect::<Vec<_>>().join("\n");
    let truncated = end < clean.len() || lines.next().is_some();
    let mut rows = if markdown {
        crate::ui::markdown::render(
            &limited,
            usize::from(request.width.max(10)),
            request.theme.text(),
            &request.theme,
        )
    } else {
        verbatim_rows(
            &limited,
            request.theme.code(),
            usize::from(request.width.max(10)),
        )
    };
    if truncated {
        rows.push(Line::from(
            "[Preview truncated; Alt+E opens the complete file]",
        ));
    }
    rows
}

/// Hard-wrap `text` at `width` columns keeping every leading space: code and
/// plain text must show as written, so nothing is trimmed at a row's start.
pub(super) fn verbatim_rows(
    text: &str,
    style: ratatui::style::Style,
    width: usize,
) -> Vec<Line<'static>> {
    use unicode_segmentation::UnicodeSegmentation;
    use unicode_width::UnicodeWidthStr;
    debug_assert!(width > 0);
    let mut rows = Vec::new();
    for source in text.split('\n') {
        let mut row = String::new();
        let mut used = 0;
        for grapheme in source.graphemes(true) {
            let cell = grapheme.width();
            if used + cell > width && used > 0 {
                rows.push(Line::styled(std::mem::take(&mut row), style));
                used = 0;
            }
            row.push_str(grapheme);
            used += cell;
        }
        rows.push(Line::styled(row, style));
    }
    debug_assert!(!rows.is_empty());
    rows
}

/// Lay out a decoded image for the request's pane and terminal protocol.
pub(super) fn picture(
    path: PathBuf,
    image: &graphics::DecodedImage,
    request: &ReadRequest,
    pages: Option<Pages>,
) -> anyhow::Result<Preview> {
    let protocol = if request.cell_pixels.0 == 0 || request.cell_pixels.1 == 0 {
        Protocol::HalfBlocks
    } else {
        request.protocol
    };
    let bytes = if protocol == Protocol::HalfBlocks {
        Vec::new()
    } else {
        graphics::encode_protocol(
            image,
            protocol,
            request.width,
            request.height,
            request.cell_pixels.0,
            request.cell_pixels.1,
        )?
    };
    let rows = if protocol == Protocol::HalfBlocks {
        graphics::render_half_blocks(image, request.width, request.height)
    } else {
        Vec::new()
    };
    Ok(Preview::Image {
        path,
        rows,
        bytes,
        protocol,
        pages,
    })
}

/// The pane's size in device pixels; half blocks count two pixels per row.
pub(super) fn pane_pixels(request: &ReadRequest) -> (u32, u32) {
    let (cell_width, cell_height) = match request.cell_pixels {
        (0, _) | (_, 0) => (1, 2),
        (width, height) => (u32::from(width), u32::from(height)),
    };
    let pixels = (
        u32::from(request.width.max(1)) * cell_width,
        u32::from(request.height.max(1)) * cell_height,
    );
    debug_assert!(pixels.0 > 0 && pixels.1 > 0);
    pixels
}
