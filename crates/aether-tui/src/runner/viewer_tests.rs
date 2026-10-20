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
        "Here is [the preview](sample.png), and [notes](notes.md).",
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
                        let result = tokio::task::spawn_blocking(move || crate::viewer::read(&request)).await?
                            .map_err(|e| format!("{e:#}"));
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
        Event::Mouse(mouse) => app.dispatch_mouse(mouse, cache),
        event => map_terminal_event(event)
            .map(|e| dispatch_app_event(app, cache, e))
            .unwrap_or_default(),
    }
}

#[test]
#[cfg(unix)]
fn viewer_pty() {
    let dir = tempfile::tempdir().unwrap();
    image::RgbaImage::from_fn(8, 8, |x, y| {
        image::Rgba([(x * 31) as u8, (y * 31) as u8, 180, 255])
    })
    .save(dir.path().join("sample.png"))
    .unwrap();
    std::fs::write(dir.path().join("notes.md"), "# Notes\nA local preview.").unwrap();
    let script =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/viewer_pty.py");
    let result = std::process::Command::new("python3")
        .arg(script)
        .arg(std::env::current_exe().unwrap())
        .arg(dir.path())
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stdout).contains("viewer PTY passed"));
}

#[test]
fn unsupported_terminal_restore_does_not_emit_kitty_commands() {
    let mut out = Vec::new();
    write_restore(&mut out, false).unwrap();
    assert!(out.starts_with(b"\x1b[?2026l"));
    assert!(!out.windows(3).any(|bytes| bytes == b"\x1b_G"));
}
