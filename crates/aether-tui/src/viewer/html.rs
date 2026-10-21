//! HTML as readable text, or a screenshot from an isolated, offline headless Chromium.
use super::{
    Documents, Preview, ReadRequest, View,
    documents::{RenderKey, Source},
    graphics,
    load::{pane_pixels, picture},
    tools::{read_bounded, run},
};
use crate::ui::{cells::text::clean, theme::Theme};
use anyhow::{Context, Result, ensure};
use html2text::render::RichAnnotation;
use ratatui::text::{Line, Span};
use std::{
    collections::HashMap,
    ffi::OsString,
    io::Read,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

const MAX_HTML_BYTES: u64 = 5 * 1024 * 1024;
const MAX_PNG_BYTES: u64 = 20 * 1024 * 1024;
const MAX_ROWS: usize = 4000;
const MAX_LINKS: usize = 200;
const SCREENSHOT_TIMEOUT: Duration = Duration::from_secs(15);
/// Inherited by the sandboxed frame: no scripts, frames, fetches or files; inline styles only.
const POLICY: &str = "default-src 'none'; img-src data:; style-src 'unsafe-inline' data:; \
     font-src data:";

pub(super) fn preview(
    path: PathBuf,
    request: &ReadRequest,
    documents: &mut Documents,
) -> Result<Preview> {
    let (source, file) = Source::open(path.clone(), MAX_HTML_BYTES, "HTML")?;
    let mut html = Vec::new();
    file.take(MAX_HTML_BYTES + 1)
        .read_to_end(&mut html)
        .context("read HTML")?;
    ensure!(
        html.len() as u64 == source.len,
        "HTML changed while it was read"
    );
    let width = usize::from(request.width.max(10));
    if request.view != View::Rendered {
        let rows = text_lines(&html, width, &request.theme)?;
        return Ok(Preview::Text {
            path,
            rows,
            pages: None,
        });
    }
    let Some(chromium) = documents.tools.chromium.clone() else {
        let mut rows = vec![Line::styled(
            "Headless Chromium is not installed; showing text. Alt+E opens it in a browser.",
            request.theme.aside(),
        )];
        rows.extend(text_lines(&html, width, &request.theme)?);
        return Ok(Preview::Text {
            path,
            rows,
            pages: None,
        });
    };
    let window = window_size(pane_pixels(request));
    let key = RenderKey {
        source,
        page: 0,
        pixels: window,
    };
    let png = documents.render(key, || screenshot(&chromium, &html, window))?;
    let image = graphics::decode(&png).context("decode HTML screenshot")?;
    picture(path, &image, request, None)
}

/// Headings, paragraphs, lists, tables and code as styled rows; links are numbered, not followed.
pub(super) fn text_lines(html: &[u8], width: usize, theme: &Theme) -> Result<Vec<Line<'static>>> {
    assert!(width > 0, "HTML needs a positive width");
    let dom = super::html_dom::parse(html, super::html_dom::PARSE_TIMEOUT)?;
    let config = html2text::config::rich().allow_width_overflow();
    let tree = config
        .dom_to_render_tree(&dom)
        .context("lay out HTML as text")?;
    let tagged = config
        .render_to_lines(tree, width)
        .context("render HTML as text")?;
    let flat: Vec<(usize, &str, &[RichAnnotation])> = tagged
        .iter()
        .enumerate()
        .flat_map(|(row, line)| {
            line.tagged_strings()
                .map(move |piece| (row, piece.s.as_str(), piece.tag.as_slice()))
        })
        .collect();
    let mut rows: Vec<Vec<Span<'static>>> = vec![Vec::new(); tagged.len().min(MAX_ROWS)];
    let mut links: Vec<&str> = Vec::new();
    let mut numbers: HashMap<&str, usize> = HashMap::new();
    for (index, &(row, text, tags)) in flat.iter().enumerate() {
        let Some(spans) = rows.get_mut(row) else {
            break;
        };
        spans.push(piece(text, tags, theme));
        let Some(url) = link(tags) else { continue };
        let continues = flat
            .get(index + 1)
            .is_some_and(|next| link(next.2) == Some(url));
        if !continues {
            let number = *numbers.entry(url).or_insert_with(|| {
                links.push(url);
                links.len()
            });
            spans.push(Span::styled(format!("[{number}]"), theme.aside()));
        }
    }
    let mut lines: Vec<Line<'static>> = rows.into_iter().map(|row| line(row, theme)).collect();
    if tagged.len() > MAX_ROWS {
        lines.push(Line::from(
            "[Preview truncated; Alt+E opens it in a browser]",
        ));
    }
    append_links(&mut lines, &links, theme);
    debug_assert_eq!(links.len(), numbers.len());
    debug_assert!(lines.len() <= MAX_ROWS + MAX_LINKS + 4);
    Ok(lines)
}

fn link(tags: &[RichAnnotation]) -> Option<&str> {
    tags.iter().rev().find_map(|tag| match tag {
        RichAnnotation::Link(url) => Some(url.as_str()),
        _ => None,
    })
}

/// The combined style of a piece's annotations; unannotated text keeps its row style.
fn piece(text: &str, tags: &[RichAnnotation], theme: &Theme) -> Span<'static> {
    let style = tags
        .iter()
        .filter_map(|tag| match tag {
            RichAnnotation::Link(_) => Some(theme.link()),
            RichAnnotation::Strong => Some(theme.strong()),
            RichAnnotation::Emphasis => Some(theme.emphasis()),
            RichAnnotation::Strikeout => Some(theme.strike()),
            RichAnnotation::Code | RichAnnotation::Preformat(_) => Some(theme.code()),
            RichAnnotation::Image(_) => Some(theme.aside()),
            _ => None,
        })
        .reduce(|style, next| style.patch(next));
    match style {
        Some(style) => Span::styled(clean(text), style),
        None => Span::raw(clean(text)),
    }
}

/// html2text marks headings with a `#` prefix; give those rows the heading style.
fn line(spans: Vec<Span<'static>>, theme: &Theme) -> Line<'static> {
    let first = spans
        .first()
        .map(|span| span.content.as_ref())
        .unwrap_or("");
    let marks = first.len() - first.trim_start_matches('#').len();
    let heading = (1..=6).contains(&marks) && first[marks..].starts_with(' ');
    let base = if heading {
        theme.heading()
    } else {
        theme.text()
    };
    Line::from(spans).style(base)
}

fn append_links(lines: &mut Vec<Line<'static>>, links: &[&str], theme: &Theme) {
    if links.is_empty() {
        return;
    }
    lines.push(Line::from(""));
    lines.push(Line::styled("Links (not followed)", theme.heading()));
    for (index, url) in links.iter().take(MAX_LINKS).enumerate() {
        lines.push(Line::from(vec![
            Span::styled(format!("[{}] ", index + 1), theme.aside()),
            Span::styled(clean(url), theme.link()),
        ]));
    }
    if links.len() > MAX_LINKS {
        lines.push(Line::from(format!(
            "… {} more links",
            links.len() - MAX_LINKS
        )));
    }
}

/// A browser viewport with the pane's shape, wide enough for ordinary page layouts.
fn window_size((width, height): (u32, u32)) -> (u32, u32) {
    assert!(width > 0 && height > 0, "pane must be nonempty");
    let window_width = width.clamp(800, 1600);
    let scaled = u64::from(window_width) * u64::from(height) / u64::from(width);
    let window_height = u32::try_from(scaled.clamp(200, 4096)).unwrap_or(4096);
    (window_width, window_height)
}

pub(super) fn screenshot(chromium: &Path, html: &[u8], window: (u32, u32)) -> Result<Vec<u8>> {
    let dir = tempfile::Builder::new()
        .prefix("aether-html-")
        .tempdir()
        .context("create screenshot directory")?;
    let page = dir.path().join("page.html");
    std::fs::write(&page, isolated_page(html)).context("write isolated page")?;
    let shot = dir.path().join("shot.png");
    let mut command = Command::new(chromium);
    command.args(chromium_args(dir.path(), &page, &shot, window));
    for name in ["HOME", "XDG_CONFIG_HOME", "XDG_CACHE_HOME"] {
        command.env(name, dir.path());
    }
    run(command, dir.path(), SCREENSHOT_TIMEOUT)?;
    let png = read_bounded(&shot, MAX_PNG_BYTES + 1).context("Chromium wrote no screenshot")?;
    ensure!(
        png.len() as u64 <= MAX_PNG_BYTES,
        "screenshot exceeds 20 MiB"
    );
    Ok(png)
}

/// The page as `srcdoc` of a sandboxed frame: scripts and meta refresh stay off, the
/// origin is opaque, and the inherited policy refuses every fetch and local file.
pub(super) fn isolated_page(html: &[u8]) -> String {
    let source = String::from_utf8_lossy(html);
    let mut escaped = String::with_capacity(source.len() + source.len() / 8);
    for character in source.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '"' => escaped.push_str("&quot;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            other => escaped.push(other),
        }
    }
    debug_assert!(!escaped.contains('"'));
    format!(
        "<!DOCTYPE html><html><head>\
         <meta http-equiv=\"Content-Security-Policy\" content=\"{POLICY}\">\
         <style>html,body{{margin:0;height:100%;overflow:hidden;background:#fff}}\
         iframe{{border:0;width:100%;height:100%;display:block}}</style>\
         </head><body><iframe sandbox srcdoc=\"{escaped}\"></iframe></body></html>"
    )
}

fn file_url(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut url = String::from("file://");
    for &byte in path.as_os_str().as_bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
            url.push(char::from(byte));
        } else {
            url.push_str(&format!("%{byte:02X}"));
        }
    }
    debug_assert!(url.is_ascii());
    url
}

/// Offline: a dead proxy for every host, loopback included, and no name resolution.
pub(super) fn chromium_args(
    dir: &Path,
    page: &Path,
    shot: &Path,
    (width, height): (u32, u32),
) -> Vec<OsString> {
    assert!(
        page.is_absolute() && shot.is_absolute(),
        "paths must be absolute"
    );
    let url = file_url(page);
    let mut profile = OsString::from("--user-data-dir=");
    profile.push(dir.join("profile"));
    let mut output = OsString::from("--screenshot=");
    output.push(shot);
    let fixed = [
        "--headless",
        "--disable-gpu",
        "--no-first-run",
        "--no-default-browser-check",
        "--disable-extensions",
        "--disable-sync",
        "--disable-background-networking",
        "--disable-component-update",
        "--proxy-server=127.0.0.1:9",
        "--proxy-bypass-list=<-loopback>",
        "--host-resolver-rules=MAP * ~NOTFOUND",
        "--hide-scrollbars",
        // A sandboxed frame in its own process may not have painted when the shot is taken.
        "--disable-features=IsolateSandboxedIframes",
    ];
    let mut args: Vec<OsString> = fixed.iter().map(OsString::from).collect();
    args.push(profile);
    args.push(format!("--window-size={width},{height}").into());
    args.push(output);
    args.push(url.into());
    args
}
