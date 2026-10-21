//! Local previews; reading and decoding happen in runner effects.
pub mod cache;
mod documents;
pub(crate) mod files;
pub mod graphics;
mod html;
mod html_dom;
mod load;
mod pdf;
pub mod probe;
pub mod references;
mod render;
pub mod tools;
pub use documents::Documents;
pub use load::{Pages, Preview, ReadRequest, View, read};
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
    pub fn external_path(&self) -> Option<&PathBuf> {
        match self.preview.as_ref()?.as_ref().ok()? {
            Preview::Outside(_) => None,
            _ => self.path(),
        }
    }

    pub fn content_rows(&self) -> usize {
        if !self.picker.is_empty() {
            return self.picker.len();
        }
        match &self.preview {
            Some(Ok(Preview::Text { rows, .. } | Preview::Image { rows, .. })) => rows.len(),
            Some(Ok(Preview::Outside(_) | Preview::Unsupported(_))) => 2,
            _ => 1,
        }
    }

    pub fn pages(&self) -> Option<Pages> {
        match self.preview.as_ref()?.as_ref().ok()? {
            Preview::Text { pages, .. } | Preview::Image { pages, .. } => *pages,
            _ => None,
        }
    }

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
        && !app.board.focused()
        && app.shown_approval().is_none()
        && !app.approval_too_short()
        && !app.bottom.top_view().is_some_and(|view| view.fullscreen())
}

pub(crate) fn hides_composer(app: &crate::App) -> bool {
    visible(app) && app.viewer.fullscreen
}

#[cfg(test)]
mod document_tests;
#[cfg(test)]
pub(crate) mod fixtures;
#[cfg(test)]
mod tests;
