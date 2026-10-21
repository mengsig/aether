//! PDF pages rendered by poppler or MuPDF on a private copy, with a pdftotext fallback.
use super::{
    Documents, Pages, Preview, ReadRequest, View,
    documents::{RenderKey, Source},
    graphics,
    load::{pane_pixels, picture, text_rows},
    tools::{Tools, read_bounded, run},
};
use anyhow::{Context, Result, bail, ensure};
use ratatui::text::Line;
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

const MAX_PDF_BYTES: u64 = 50 * 1024 * 1024;
const MAX_PAGES: u32 = 100_000;
const MAX_PNG_BYTES: u64 = 20 * 1024 * 1024;
const MIN_SIDE: u32 = 200;
const MAX_SIDE: u32 = 2048;
const INFO_TIMEOUT: Duration = Duration::from_secs(10);
const RENDER_TIMEOUT: Duration = Duration::from_secs(20);
const NO_COUNTER: &str = "pdftoppm needs pdfinfo (poppler-utils) to count pages, or install \
     MuPDF (mutool). Alt+E opens this file externally.";
const NO_TOOLS: &str = "No PDF tools found: install poppler-utils (pdftoppm, pdfinfo, pdftotext) \
     or MuPDF (mutool) to preview here. Alt+E opens this file externally.";

/// The open document, copied once so every tool reads the bytes that were checked.
#[derive(Debug)]
pub(crate) struct PdfCopy {
    source: Source,
    dir: tempfile::TempDir,
    pages: Option<u32>,
}

impl PdfCopy {
    fn document(&self) -> PathBuf {
        self.dir.path().join("document.pdf")
    }
}

pub(super) fn preview(
    path: PathBuf,
    request: &ReadRequest,
    documents: &mut Documents,
) -> Result<Preview> {
    let tools = documents.tools.clone();
    let renders = tools.pdftoppm.is_some() || tools.mutool.is_some();
    let counts = tools.pdfinfo.is_some() || tools.mutool.is_some();
    if !renders && tools.pdftotext.is_none() {
        bail!(NO_TOOLS);
    }
    if !counts && tools.pdftotext.is_none() {
        bail!(NO_COUNTER);
    }
    let (source, file) = Source::open(path.clone(), MAX_PDF_BYTES, "PDF")?;
    let (document, total) = open_copy(documents, source.clone(), file)?;
    let Some(total) = total else {
        return unpaged_text(path, &document, request, &tools);
    };
    let pages = Pages {
        current: request.page.clamp(1, total),
        total,
    };
    if request.view == View::Text || !renders {
        let notice = (!renders).then_some("No pdftoppm or mutool: showing the page text.");
        return text_page(path, &document, pages, request, &tools, notice);
    }
    let side = {
        let (width, height) = pane_pixels(request);
        width.max(height).clamp(MIN_SIDE, MAX_SIDE)
    };
    let key = RenderKey {
        source,
        page: pages.current,
        pixels: (side, side),
    };
    let png = documents.render(key, || render_page(&tools, &document, pages.current, side))?;
    let image = graphics::decode(&png).context("decode rendered page")?;
    picture(path, &image, request, Some(pages))
}

/// Reuse the private copy of an unchanged document; otherwise copy it and count its
/// pages, which needs pdfinfo or mutool.
fn open_copy(
    documents: &mut Documents,
    source: Source,
    file: File,
) -> Result<(PathBuf, Option<u32>)> {
    if let Some(copy) = documents.pdf.as_ref().filter(|c| c.source == source) {
        return Ok((copy.document(), copy.pages));
    }
    documents.pdf = None;
    let dir = tempfile::Builder::new()
        .prefix("aether-pdf-")
        .tempdir()
        .context("create PDF work directory")?;
    let target = dir.path().join("document.pdf");
    let mut out = File::create_new(&target).context("create PDF copy")?;
    let copied = std::io::copy(&mut file.take(source.len + 1), &mut out).context("copy PDF")?;
    ensure!(copied == source.len, "PDF changed while it was read");
    let pages = count_pages(&documents.tools, &target, dir.path())?;
    let copy = PdfCopy { source, dir, pages };
    let opened = (copy.document(), copy.pages);
    documents.pdf = Some(copy);
    Ok(opened)
}

fn count_pages(tools: &Tools, document: &Path, dir: &Path) -> Result<Option<u32>> {
    let mut command;
    if let Some(pdfinfo) = &tools.pdfinfo {
        command = Command::new(pdfinfo);
        command.arg(document);
    } else if let Some(mutool) = &tools.mutool {
        command = Command::new(mutool);
        command.arg("info").arg(document);
    } else {
        return Ok(None);
    }
    let info = run(command, dir, INFO_TIMEOUT)?;
    parse_page_count(&String::from_utf8_lossy(&info)).map(Some)
}

pub(super) fn parse_page_count(info: &str) -> Result<u32> {
    let count = info
        .lines()
        .find_map(|line| line.trim().strip_prefix("Pages:"))
        .context("PDF info has no page count")?
        .trim()
        .parse::<u32>()
        .context("PDF page count is not a number")?;
    ensure!(count > 0, "PDF has no pages");
    ensure!(count <= MAX_PAGES, "PDF has more than {MAX_PAGES} pages");
    Ok(count)
}

/// Draw one page so its longer side is `side` pixels, returning the PNG bytes.
fn render_page(tools: &Tools, document: &Path, page: u32, side: u32) -> Result<Vec<u8>> {
    let dir = document.parent().context("PDF copy has no directory")?;
    let output = dir.join("page.png");
    match std::fs::remove_file(&output) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            return Err(error).context("remove previous page image");
        }
        _ => {}
    }
    let command = render_command(tools, document, &output, page, side)?;
    run(command, dir, RENDER_TIMEOUT)?;
    let png = read_bounded(&output, MAX_PNG_BYTES + 1).context("read rendered page")?;
    ensure!(
        png.len() as u64 <= MAX_PNG_BYTES,
        "rendered page exceeds 20 MiB"
    );
    std::fs::remove_file(&output).context("remove rendered page")?;
    Ok(png)
}

pub(super) fn render_command(
    tools: &Tools,
    document: &Path,
    output: &Path,
    page: u32,
    side: u32,
) -> Result<Command> {
    assert!(page > 0, "PDF pages are one-based");
    assert!((MIN_SIDE..=MAX_SIDE).contains(&side));
    let (page, side) = (page.to_string(), side.to_string());
    if let Some(pdftoppm) = &tools.pdftoppm {
        let mut command = Command::new(pdftoppm);
        let prefix = output.with_extension("");
        command.args(["-png", "-singlefile", "-f", &page, "-l", &page]);
        command.args(["-scale-to", &side]).arg(document).arg(prefix);
        return Ok(command);
    }
    let mutool = tools.mutool.as_ref().context("no PDF renderer")?;
    let mut command = Command::new(mutool);
    command.args(["draw", "-q", "-F", "png", "-w", &side, "-h", &side, "-o"]);
    command.arg(output).arg(document).arg(page);
    Ok(command)
}

fn text_page(
    path: PathBuf,
    document: &Path,
    pages: Pages,
    request: &ReadRequest,
    tools: &Tools,
    notice: Option<&str>,
) -> Result<Preview> {
    let mut rows: Vec<Line<'static>> = notice
        .map(|text| Line::styled(text.to_owned(), request.theme.aside()))
        .into_iter()
        .collect();
    let Some(pdftotext) = &tools.pdftotext else {
        rows.push(Line::from(
            "pdftotext is not installed; Alt+T shows the rendered page.",
        ));
        return Ok(Preview::Text {
            path,
            rows,
            pages: Some(pages),
        });
    };
    let page = pages.current.to_string();
    let mut command = Command::new(pdftotext);
    command.args(["-f", &page, "-l", &page, "-layout", "-enc", "UTF-8"]);
    command.arg(document).arg("-");
    let dir = document.parent().context("PDF copy has no directory")?;
    let text = run(command, dir, INFO_TIMEOUT)?;
    let text = String::from_utf8_lossy(&text);
    if text.trim().is_empty() {
        rows.push(Line::from("This page has no extractable text."));
    } else {
        rows.extend(text_rows(&text, false, request));
    }
    Ok(Preview::Text {
        path,
        rows,
        pages: Some(pages),
    })
}

/// pdftotext without a page counter: the document's text in one unpaged view.
fn unpaged_text(
    path: PathBuf,
    document: &Path,
    request: &ReadRequest,
    tools: &Tools,
) -> Result<Preview> {
    let pdftotext = tools.pdftotext.as_ref().context("no pdftotext")?;
    let mut command = Command::new(pdftotext);
    command
        .args(["-layout", "-enc", "UTF-8"])
        .arg(document)
        .arg("-");
    let dir = document.parent().context("PDF copy has no directory")?;
    let text = run(command, dir, INFO_TIMEOUT)?;
    let text = String::from_utf8_lossy(&text);
    let mut rows = vec![Line::styled(
        "No pdfinfo or mutool: showing the document text.",
        request.theme.aside(),
    )];
    if text.trim().is_empty() {
        rows.push(Line::from("This PDF has no extractable text."));
    } else {
        rows.extend(text_rows(&text, false, request));
    }
    Ok(Preview::Text {
        path,
        rows,
        pages: None,
    })
}
