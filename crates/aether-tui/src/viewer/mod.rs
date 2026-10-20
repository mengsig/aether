//! Local previews; reading and decoding happen in runner effects.
pub mod cache;
mod files;
pub mod graphics;
mod load;
pub mod probe;
pub mod references;
mod render;
pub use load::{Preview, ReadRequest, read};
pub use render::{Hit, mark_references, render, split};

use references::Reference;
use std::path::PathBuf;

#[derive(Debug)]
pub struct Viewer {
    pub open: bool,
    pub fullscreen: bool,
    pub focused: bool,
    pub share: u16,
    pub scroll: u16,
    pub picker: Vec<Reference>,
    pub cursor: usize,
    pub request: u64,
    pub pending: Option<ReadRequest>,
    pub preview: Option<Result<Preview, String>>,
    pub protocol: probe::Protocol,
    pub cell_pixels: (u16, u16),
    pub source_cell: Option<(
        Option<aether_core::id::AgentId>,
        crate::app::transcript::CellId,
    )>,
}

impl Default for Viewer {
    fn default() -> Self {
        Self {
            open: false,
            fullscreen: false,
            focused: true,
            share: 50,
            scroll: 0,
            picker: Vec::new(),
            cursor: 0,
            request: 0,
            pending: None,
            preview: None,
            protocol: probe::Protocol::HalfBlocks,
            cell_pixels: (0, 0),
            source_cell: None,
        }
    }
}

impl Viewer {
    pub fn path(&self) -> Option<&PathBuf> {
        match self.preview.as_ref()?.as_ref().ok()? {
            Preview::Text { path, .. }
            | Preview::Image { path, .. }
            | Preview::Failed { path, .. }
            | Preview::Outside(path)
            | Preview::Unsupported(path) => Some(path),
        }
    }
}

pub(crate) fn visible(app: &crate::App) -> bool {
    app.viewer.open
        && app.shown_approval().is_none()
        && !app.approval_too_short()
        && !app.bottom.top_view().is_some_and(|view| view.fullscreen())
}

#[cfg(test)]
mod tests;
