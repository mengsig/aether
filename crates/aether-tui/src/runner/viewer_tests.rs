#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
use super::*;
use anyhow::Context;
use crossterm::event::{KeyCode, KeyModifiers};
use std::io::Write;

#[test]
#[ignore = "PTY/graphics terminal fixture, launched by viewer_pty"]
fn terminal_fixture() {
    if std::env::var_os("AETHER_VIEWER_FIXTURE").is_none() {
        return;
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(fixture()).unwrap();
}

async fn fixture() -> anyhow::Result<()> {
    let workspace = std::path::PathBuf::from(std::env::var("AETHER_VIEWER_WORKSPACE")?);
    let mut app = crate::testing::app_with_transcript(&[]);
    let agent = app.selected.context("fixture needs an agent")?;
    crate::testing::push_message(
        &mut app,
        agent,
        aether_core::model::MessageRole::Assistant,
        "Here is [the preview](sample.png), and [notes](notes.md).\n\
         Also report.pdf and page.html for review.",
    );
    app.options.cwd = Some(workspace);
    app.board.open = false;
    app.approvals.clear();
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen, EnableMouseCapture)?;
    let result = fixture_loop(&mut app).await;
    execute!(stdout(), DisableMouseCapture, LeaveAlternateScreen)?;
    disable_raw_mode()?;
    result
}

async fn fixture_loop(app: &mut App) -> anyhow::Result<()> {
    let mut terminal = Terminal::new(native_backend(stdout()))?;
    let mut cache = ViewCache::default();
    let capabilities = crate::viewer::probe::detect_with_metrics()?;
    app.viewer.protocol = capabilities.protocol;
    app.viewer.cell_pixels = capabilities.cell_pixels.unwrap_or((0, 0));
    if let Ok(pixels) = crossterm::terminal::window_size()
        && pixels.columns > 0
        && pixels.rows > 0
    {
        let measured = (pixels.width / pixels.columns, pixels.height / pixels.rows);
        if measured.0 > 0 && measured.1 > 0 {
            app.viewer.cell_pixels = measured;
        }
    }
    let size = terminal.size()?;
    app.size = (size.width, size.height);
    let mut events = EventStream::new();
    let mut documents = Some(crate::viewer::Documents::detect());
    let until = tokio::time::sleep(Duration::from_secs(25));
    tokio::pin!(until);
    loop {
        draw(&mut terminal, app, &mut cache)?;
        tokio::select! {
            _ = &mut until => break,
            event = events.next() => {
                let Some(event) = event else { break };
                let effects = fixture_event(app,&mut cache,event?);
                for effect in effects {
                    if let Effect::PreviewFile(request) = effect {
                        let id = request.id;
                        let mut owned = documents.take();
                        let (result, owned) = tokio::task::spawn_blocking(move || {
                            let result = owned.as_mut().map(|d| crate::viewer::read(&request, d));
                            (result, owned)
                        }).await?;
                        documents = owned;
                        let result = result.context("fixture documents")?.map_err(|e| format!("{e:#}"));
                        app.reduce(AppEvent::PreviewReady { id,result });
                    }
                }
                if app.quit_requested { break; }
            }
        }
    }
    if std::env::var_os("AETHER_VIEWER_ASSERT_CLOSED").is_some() {
        anyhow::ensure!(!app.viewer.open, "Escape must close the viewer");
        anyhow::ensure!(
            app.viewer.pending.is_none(),
            "closed viewer must discard pending reads"
        );
    }
    stdout().write_all(b"VIEWER_FIXTURE_DONE")?;
    Ok(())
}

fn fixture_event(app: &mut App, cache: &mut ViewCache, event: Event) -> Vec<Effect> {
    match event {
        Event::Key(key)
            if key.code == KeyCode::Char('c') && key.modifiers == KeyModifiers::CONTROL =>
        {
            app.quit_requested = true;
            Vec::new()
        }
        Event::Key(key) if key.code == KeyCode::F(8) => {
            if let Some(id) = app.selected {
                crate::testing::push_message(
                    app,
                    id,
                    aether_core::model::MessageRole::Assistant,
                    "Agent continues streaming while the file is open.",
                );
            }
            Vec::new()
        }
        // F9 reports clickable references so the PTY driver can click them.
        Event::Key(key) if key.code == KeyCode::F(9) => {
            let hits: String = cache
                .file_hits
                .iter()
                .map(|hit| format!("{}@{},{};", hit.reference.target, hit.area.x, hit.area.y))
                .collect();
            let mut out = stdout();
            let reported = write!(out, "HITS[{hits}]END").and_then(|()| out.flush());
            assert!(reported.is_ok(), "fixture output closed");
            Vec::new()
        }
        Event::Mouse(mouse) => app.dispatch_mouse(mouse, cache),
        event => map_terminal_event(event)
            .map(|e| dispatch_app_event(app, cache, e))
            .unwrap_or_default(),
    }
}

#[test]
#[cfg(unix)]
fn viewer_pty() {
    let tools = crate::viewer::tools::Tools::detect();
    run_pty(None, pdf_expectation(&tools));
}

/// Without any PDF tool on PATH the clicked PDF names what to install.
#[test]
#[cfg(unix)]
fn viewer_pty_without_pdf_tools() {
    let empty = tempfile::tempdir().unwrap();
    run_pty(Some(empty.path()), "No PDF tools found");
}

/// poppler without pdftoppm still pages through the text.
#[test]
#[cfg(unix)]
fn viewer_pty_with_text_tools_only() {
    let tools = crate::viewer::tools::Tools::detect();
    let (Some(pdfinfo), Some(pdftotext)) = (tools.pdfinfo, tools.pdftotext) else {
        return;
    };
    let bin = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(pdfinfo, bin.path().join("pdfinfo")).unwrap();
    std::os::unix::fs::symlink(pdftotext, bin.path().join("pdftotext")).unwrap();
    run_pty(Some(bin.path()), "pages");
}

/// Drive the fixture in a PTY; `path` replaces PATH for the TUI and its tools.
fn run_pty(path: Option<&std::path::Path>, pdf_shows: &str) {
    let dir = tempfile::tempdir().unwrap();
    image::RgbaImage::from_fn(8, 8, |x, y| {
        image::Rgba([(x * 31) as u8, (y * 31) as u8, 180, 255])
    })
    .save(dir.path().join("sample.png"))
    .unwrap();
    std::fs::write(dir.path().join("notes.md"), "# Notes\nA local preview.").unwrap();
    let pdf = crate::viewer::fixtures::pdf(&["First page", "Second page"]);
    std::fs::write(dir.path().join("report.pdf"), pdf).unwrap();
    std::fs::write(dir.path().join("page.html"), crate::viewer::fixtures::HTML).unwrap();
    let script =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/viewer_pty.py");
    let python = which("python3").expect("python3 on PATH");
    let mut command = std::process::Command::new(python);
    command
        .arg(script)
        .arg(std::env::current_exe().unwrap())
        .arg(dir.path())
        .env("VIEWER_PDF_SHOWS", pdf_shows);
    if let Some(path) = path {
        command.env("PATH", path);
    }
    let result = command.output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stdout).contains("viewer PTY passed"));
}

fn which(name: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

#[test]
fn unsupported_terminal_restore_does_not_emit_kitty_commands() {
    let mut out = Vec::new();
    write_restore(&mut out, false).unwrap();
    assert!(out.starts_with(b"\x1b[?2026l"));
    assert!(!out.windows(3).any(|bytes| bytes == b"\x1b_G"));
}

/// What the pane shows for a clicked PDF with the tools on this machine.
fn pdf_expectation(tools: &crate::viewer::tools::Tools) -> &'static str {
    let renders = tools.pdftoppm.is_some() || tools.mutool.is_some();
    let counts = tools.pdfinfo.is_some() || tools.mutool.is_some();
    let text = tools.pdftotext.is_some();
    match (renders || text, counts, text) {
        (false, _, _) => "No PDF tools found",
        (true, true, _) => "pages",
        (true, false, true) => "showing the document text",
        (true, false, false) => "pdftoppm needs pdfinfo",
    }
}
