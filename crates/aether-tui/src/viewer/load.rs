use super::{graphics, probe::Protocol, references};
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
}

#[derive(Debug)]
pub enum Preview {
    Text {
        path: PathBuf,
        rows: Vec<Line<'static>>,
    },
    Image {
        path: PathBuf,
        rows: Vec<Line<'static>>,
        bytes: Vec<u8>,
        protocol: Protocol,
    },
    Outside(PathBuf),
    Unsupported(PathBuf),
    Failed {
        path: PathBuf,
        error: String,
    },
}

pub fn read(request: &ReadRequest) -> anyhow::Result<Preview> {
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
        "png" | "jpg" | "jpeg" | "gif" | "webp" => image(path.clone(), request),
        "pdf" | "svg" | "html" | "htm" => Ok(Preview::Unsupported(path.clone())),
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
    let clean = crate::ui::cells::text::clean(&text);
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
        crate::ui::cells::text::wrap_text(
            &limited,
            request.theme.code(),
            usize::from(request.width.max(10)),
            Vec::new(),
            Vec::new(),
        )
    };
    if truncated {
        rows.push(Line::from(
            "[Preview truncated; Alt+E opens the complete file]",
        ));
    }
    Ok(Preview::Text { path, rows })
}

fn image(path: PathBuf, request: &ReadRequest) -> anyhow::Result<Preview> {
    let image = graphics::load_canonical(&path)?;
    let protocol = if request.cell_pixels.0 == 0 || request.cell_pixels.1 == 0 {
        Protocol::HalfBlocks
    } else {
        request.protocol
    };
    let bytes = if protocol == Protocol::HalfBlocks {
        Vec::new()
    } else {
        graphics::encode_protocol(
            &image,
            protocol,
            request.width,
            request.height,
            request.cell_pixels.0,
            request.cell_pixels.1,
        )?
    };
    let rows = if protocol == Protocol::HalfBlocks {
        graphics::render_half_blocks(&image, request.width, request.height)
    } else {
        Vec::new()
    };
    Ok(Preview::Image {
        path,
        rows,
        bytes,
        protocol,
    })
}
