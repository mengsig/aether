//! Worker-owned state for PDF and HTML previews: tools, the open copy and rendered pages.
use super::tools::Tools;
use anyhow::{Context, Result, ensure};
use std::{collections::VecDeque, fs::File, path::PathBuf, sync::Arc, time::SystemTime};

const MAX_RENDERS: usize = 6;
const MAX_RENDER_BYTES: usize = 48 * 1024 * 1024;

/// Identity of a file as it was opened; a change in size or mtime is a new document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Source {
    pub path: PathBuf,
    pub len: u64,
    pub modified: Option<SystemTime>,
}

impl Source {
    /// Open the canonical path without following symlinks and enforce a size limit.
    pub fn open(path: PathBuf, limit: u64, kind: &str) -> Result<(Self, File)> {
        let file = super::files::open_canonical(&path).with_context(|| format!("open {kind}"))?;
        let metadata = file
            .metadata()
            .with_context(|| format!("read {kind} metadata"))?;
        ensure!(metadata.is_file(), "{kind} must be a regular file");
        ensure!(metadata.len() > 0, "{kind} is empty");
        ensure!(
            metadata.len() <= limit,
            "{kind} exceeds {} MiB",
            limit / (1024 * 1024)
        );
        let source = Self {
            path,
            len: metadata.len(),
            modified: metadata.modified().ok(),
        };
        Ok((source, file))
    }
}

/// One rendered view: page 0 is an HTML screenshot; pixels are the requested box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RenderKey {
    pub source: Source,
    pub page: u32,
    pub pixels: (u32, u32),
}

#[derive(Debug)]
pub struct Documents {
    pub(super) tools: Tools,
    pub(super) pdf: Option<super::pdf::PdfCopy>,
    renders: VecDeque<(RenderKey, Arc<[u8]>)>,
}

impl Documents {
    pub fn new(tools: Tools) -> Self {
        Self {
            tools,
            pdf: None,
            renders: VecDeque::new(),
        }
    }

    pub fn detect() -> Self {
        Self::new(Tools::detect())
    }

    /// The PNG bytes for `key`, rendered by `render` only on a miss.
    pub(super) fn render(
        &mut self,
        key: RenderKey,
        render: impl FnOnce() -> Result<Vec<u8>>,
    ) -> Result<Arc<[u8]>> {
        if let Some(index) = self.renders.iter().position(|(k, _)| *k == key) {
            let entry = self
                .renders
                .remove(index)
                .context("cached render vanished")?;
            let png = entry.1.clone();
            self.renders.push_back(entry);
            return Ok(png);
        }
        let png: Arc<[u8]> = render()?.into();
        ensure!(!png.is_empty(), "renderer produced an empty image");
        self.renders.push_back((key, png.clone()));
        while self.renders.len() > MAX_RENDERS || self.render_bytes() > MAX_RENDER_BYTES {
            self.renders.pop_front();
        }
        debug_assert!(self.renders.len() <= MAX_RENDERS);
        Ok(png)
    }

    fn render_bytes(&self) -> usize {
        self.renders.iter().map(|(_, png)| png.len()).sum()
    }

    #[cfg(test)]
    pub(super) fn cached_renders(&self) -> usize {
        self.renders.len()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]
    use super::*;

    fn key(page: u32) -> RenderKey {
        RenderKey {
            source: Source {
                path: PathBuf::from("/doc.pdf"),
                len: 1,
                modified: None,
            },
            page,
            pixels: (10, 10),
        }
    }

    #[test]
    fn renders_are_reused_and_the_oldest_page_is_evicted() {
        let mut documents = Documents::new(Tools::default());
        let mut calls = 0;
        for page in 1..=MAX_RENDERS as u32 + 1 {
            documents
                .render(key(page), || {
                    calls += 1;
                    Ok(vec![1; 4])
                })
                .unwrap();
        }
        assert_eq!(calls, MAX_RENDERS + 1);
        assert_eq!(documents.cached_renders(), MAX_RENDERS);
        documents
            .render(key(2), || panic!("page 2 is cached"))
            .unwrap();
        let rendered = documents.render(key(1), || Ok(vec![2; 4])).unwrap();
        assert_eq!(&rendered[..], &[2; 4]);
        assert!(documents.render(key(9), || Ok(Vec::new())).is_err());
    }
}
