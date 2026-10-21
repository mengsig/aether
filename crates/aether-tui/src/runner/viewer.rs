use super::LoopEvent;
use crate::{
    app::AppEvent,
    viewer::{Documents, Preview, ReadRequest, probe::Protocol},
};
use std::io::Write;
use std::path::PathBuf;
use tokio::sync::mpsc;

#[derive(Debug)]
pub(super) struct Reader {
    latest: tokio::sync::watch::Sender<Option<ReadRequest>>,
    task: tokio::task::JoinHandle<()>,
}

impl Reader {
    pub fn new(tx: mpsc::UnboundedSender<LoopEvent>) -> Self {
        let (latest, mut rx) = tokio::sync::watch::channel::<Option<ReadRequest>>(None);
        let task = tokio::spawn(async move {
            // Requests run one at a time, so the page cache moves into each read and back.
            let mut documents = None;
            while rx.changed().await.is_ok() {
                let Some(request) = rx.borrow_and_update().clone() else {
                    continue;
                };
                let id = request.id;
                let mut owned = documents.take();
                let result = tokio::task::spawn_blocking(move || {
                    let documents = owned.get_or_insert_with(Documents::detect);
                    let result = crate::viewer::read(&request, documents);
                    (result, owned)
                })
                .await;
                let result = match result {
                    Ok((result, owned)) => {
                        documents = owned;
                        result.map_err(|error| format!("{error:#}"))
                    }
                    Err(error) => Err(format!("preview worker failed: {error}")),
                };
                if tx
                    .send(LoopEvent::App(AppEvent::PreviewReady { id, result }))
                    .is_err()
                {
                    break;
                }
            }
        });
        Self { latest, task }
    }

    pub fn read(&self, request: ReadRequest) {
        self.latest.send_replace(Some(request));
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub(super) fn spawn_external(tx: mpsc::UnboundedSender<LoopEvent>, path: PathBuf) {
    tokio::spawn(async move {
        let result = open_external(path).await;
        if let Err(error) = result {
            let _ = tx.send(LoopEvent::App(AppEvent::EffectFailed {
                action: "open file externally".into(),
                error: format!("{error:#}"),
            }));
        }
    });
}

async fn open_external(path: PathBuf) -> anyhow::Result<()> {
    anyhow::ensure!(path.is_absolute(), "external opener needs an absolute path");
    let _file = crate::viewer::files::open_canonical(&path)?;
    let program = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let mut child = tokio::process::Command::new(program)
        .arg(&path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let status = tokio::time::timeout(std::time::Duration::from_secs(10), child.wait()).await??;
    anyhow::ensure!(status.success(), "{program} failed: {status}");
    Ok(())
}

pub(super) fn placement(app: &crate::App) -> Option<(u64, ratatui::layout::Rect, Protocol)> {
    if !crate::viewer::visible(app) || !app.viewer.picker.is_empty() {
        return None;
    }
    let Some(Ok(Preview::Image { protocol, .. })) = &app.viewer.preview else {
        return None;
    };
    if *protocol == Protocol::HalfBlocks {
        return None;
    }
    let (_, area) = crate::viewer::split(
        ratatui::layout::Rect::new(0, 0, app.size.0, app.size.1),
        &app.viewer,
    );
    if area.width < 3 || area.height < 4 {
        return None;
    }
    Some((app.viewer.request, area, *protocol))
}

pub(super) fn paint(out: &mut impl Write, app: &crate::App) -> std::io::Result<()> {
    let Some((_, area, _)) = placement(app) else {
        return Ok(());
    };
    let Some(Ok(Preview::Image { bytes, .. })) = &app.viewer.preview else {
        return Ok(());
    };
    debug_assert!(area.right() <= app.size.0);
    debug_assert!(area.bottom() <= app.size.1);
    write!(out, "\x1b7\x1b[{};{}H", area.y + 2, area.x + 2)?;
    out.write_all(bytes)?;
    out.write_all(b"\x1b8")?;
    out.flush()
}
