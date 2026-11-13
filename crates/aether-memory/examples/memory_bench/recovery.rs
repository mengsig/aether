//! What survives a restart, and what a damaged store does: refuse to open,
//! fail retrieval, or answer with less than it held.

use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;

use serde::Serialize;

use crate::plan::Plan;
use crate::retrieval::{QueryRow, Ranked, run_queries};
use crate::seed::{Seeded, count_current};
use crate::{BenchResult, open, store_path};

#[derive(Debug, Serialize)]
pub struct RestartRow {
    pub queries: usize,
    /// Queries ranked the same memories in the same order after reopening.
    pub identical: usize,
    pub current_before: usize,
    pub current_after: usize,
}

#[derive(Debug, Serialize)]
pub struct CorruptionRow {
    pub case: &'static str,
    /// `ok`, `error` or `panic`.
    pub open: &'static str,
    /// `ok`, `error`, `panic`, or `skipped` when the store did not open.
    pub retrieval: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Current memories a full listing still returns; `None` when it failed.
    pub recovered: Option<usize>,
    pub expected: usize,
}

/// Reopen the closed store and rank every query again.
pub fn restart(
    dir: &Path,
    plan: &Plan,
    seeded: &Seeded,
    before: &[QueryRow],
) -> BenchResult<RestartRow> {
    let (store, service) = open(dir, &plan.index)?;
    let after = run_queries(&service, plan, seeded)?;
    let current_after = count_current(&store)?;
    debug_assert_eq!(after.len(), before.len());
    let identical = before
        .iter()
        .zip(&after)
        .filter(|(b, a)| same_ranking(&b.ranked, &a.ranked))
        .count();
    Ok(RestartRow {
        queries: before.len(),
        identical,
        current_before: seeded.current,
        current_after,
    })
}

/// Same memories in the same order. Scores may move by the seconds elapsed,
/// through recency, without that being a difference.
fn same_ranking(a: &[Ranked], b: &[Ranked]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.key == y.key)
}

/// One way a store file is damaged. Each runs on its own copy.
struct Case {
    name: &'static str,
    damage: fn(&Path) -> std::io::Result<()>,
}

const CASES: [Case; 6] = [
    // The control: a copy must hold everything, or every other case is moot.
    Case {
        name: "intact_copy",
        damage: |_| Ok(()),
    },
    Case {
        name: "missing_file",
        damage: |path| std::fs::remove_file(path),
    },
    Case {
        name: "truncated_half",
        damage: truncate_half,
    },
    Case {
        name: "zeroed_header",
        damage: |path| overwrite(path, 0, 100),
    },
    Case {
        name: "garbled_middle_page",
        damage: garble_middle_page,
    },
    Case {
        name: "garbled_every_8th_page",
        damage: garble_every_8th_page,
    },
];

pub fn corruption_cases(
    closed: &Path,
    sandbox: &Path,
    plan: &Plan,
    seeded: &Seeded,
) -> BenchResult<Vec<CorruptionRow>> {
    let mut rows = Vec::with_capacity(CASES.len());
    for case in &CASES {
        let dir = sandbox.join(format!("corrupt-{}", case.name));
        std::fs::create_dir_all(&dir)?;
        std::fs::copy(store_path(closed), store_path(&dir))?;
        (case.damage)(&store_path(&dir))?;
        rows.push(probe(case.name, &dir, plan, seeded));
    }
    debug_assert_eq!(rows.len(), CASES.len());
    Ok(rows)
}

/// Open the damaged copy and use it as an agent would, classifying each
/// step. A panic is caught and reported: it is one of the outcomes measured.
fn probe(case: &'static str, dir: &Path, plan: &Plan, seeded: &Seeded) -> CorruptionRow {
    let mut row = CorruptionRow {
        case,
        open: "ok",
        retrieval: "skipped",
        error: None,
        recovered: None,
        expected: seeded.current,
    };
    let opened = match catch_unwind(AssertUnwindSafe(|| open(dir, &plan.index))) {
        Ok(Ok(opened)) => opened,
        Ok(Err(error)) => return failed(row, "error", None, error),
        Err(_) => return failed(row, "panic", None, "panicked while opening".into()),
    };
    let (store, service) = opened;
    let used = catch_unwind(AssertUnwindSafe(|| -> BenchResult<usize> {
        run_queries(&service, plan, seeded)?;
        count_current(&store)
    }));
    row.retrieval = "ok";
    match used {
        Ok(Ok(recovered)) => row.recovered = Some(recovered),
        Ok(Err(error)) => return failed(row, "ok", Some("error"), error),
        Err(_) => return failed(row, "ok", Some("panic"), "panicked while retrieving".into()),
    }
    row
}

fn failed(
    mut row: CorruptionRow,
    open: &'static str,
    retrieval: Option<&'static str>,
    error: Box<dyn std::error::Error>,
) -> CorruptionRow {
    row.open = open;
    row.retrieval = retrieval.unwrap_or("skipped");
    let text = error.to_string();
    row.error = Some(text.lines().next().unwrap_or_default().to_owned());
    row
}

fn truncate_half(path: &Path) -> std::io::Result<()> {
    let file = OpenOptions::new().write(true).open(path)?;
    let len = file.metadata()?.len();
    file.set_len(len / 2)
}

fn overwrite(path: &Path, offset: u64, len: usize) -> std::io::Result<()> {
    let mut file = OpenOptions::new().write(true).open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    file.write_all(&vec![0; len])?;
    file.sync_all()
}

const PAGE: u64 = 4_096;

/// The middle leaf page of the `memories` table written over with a byte
/// pattern: damage to the rows retrieval reads that the file header cannot
/// show. Found by its role in the b-tree, not by file offset, so a store a few
/// pages larger or smaller still loses the same kind of page. A leaf, not the
/// root, so it is one page of memories lost, not the whole table's index.
fn garble_middle_page(path: &Path) -> std::io::Result<()> {
    let root = table_root(path, "memories")?;
    let leaf = middle_leaf_page(path, root)?;
    // SQLite numbers pages from 1; `garble_pages` takes file indices from 0.
    garble_pages(path, &[leaf - 1])
}

/// The root page number SQLite's schema records for `table`, read through a
/// read-only connection that is closed before the file is damaged.
fn table_root(path: &Path, table: &str) -> std::io::Result<u64> {
    let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY;
    let conn = rusqlite::Connection::open_with_flags(path, flags).map_err(std::io::Error::other)?;
    let root: i64 = conn
        .query_row(
            "SELECT rootpage FROM sqlite_schema WHERE type = 'table' AND name = ?1",
            [table],
            |row| row.get(0),
        )
        .map_err(std::io::Error::other)?;
    assert!(
        root > 1,
        "table {table} cannot be rooted on the schema page"
    );
    Ok(root as u64)
}

/// The leaf in the middle of the table b-tree rooted at `root`: from each
/// interior page, the middle child. The root itself when the table fits on it.
fn middle_leaf_page(path: &Path, root: u64) -> std::io::Result<u64> {
    let bytes = std::fs::read(path)?;
    assert_eq!(u64::from(be_u16(&bytes, 16)), PAGE, "the store's page size");
    let pages = bytes.len() as u64 / PAGE;
    let mut page = root;
    for _ in 0..MAX_DEPTH {
        assert!((2..=pages).contains(&page), "page {page} of {pages}");
        let start = (page - 1) as usize * PAGE as usize;
        let header = &bytes[start..start + PAGE as usize];
        let cells = usize::from(be_u16(header, 3));
        match header[0] {
            TABLE_LEAF => return Ok(page),
            TABLE_INTERIOR if cells > 0 => {}
            other => {
                return Err(std::io::Error::other(format!(
                    "page {page} is no table page with cells: type {other:#x}, {cells} cells"
                )));
            }
        }
        // Children are each cell's left pointer, then the right-most pointer.
        let middle = cells.div_ceil(2);
        let child = match middle < cells {
            true => be_u32(
                header,
                usize::from(be_u16(header, INTERIOR_HEADER + 2 * middle)),
            ),
            false => be_u32(header, 8),
        };
        page = u64::from(child);
    }
    Err(std::io::Error::other(format!(
        "no leaf within {MAX_DEPTH} levels of page {root}"
    )))
}

fn be_u16(page: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes([page[offset], page[offset + 1]])
}

fn be_u32(page: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes([
        page[offset],
        page[offset + 1],
        page[offset + 2],
        page[offset + 3],
    ])
}

/// SQLite b-tree page types and the interior page header's length.
const TABLE_INTERIOR: u8 = 0x05;
const TABLE_LEAF: u8 = 0x0d;
const INTERIOR_HEADER: usize = 12;
/// Far deeper than any table of this bench grows; it bounds the walk.
const MAX_DEPTH: usize = 16;

/// Spread damage that is bound to reach the memory tables and their index.
fn garble_every_8th_page(path: &Path) -> std::io::Result<()> {
    let pages = std::fs::metadata(path)?.len() / PAGE;
    let damaged: Vec<u64> = (1..pages).step_by(8).collect();
    garble_pages(path, &damaged)
}

/// Overwrite each page by its index in the file, from 0.
fn garble_pages(path: &Path, pages: &[u64]) -> std::io::Result<()> {
    let mut file = OpenOptions::new().write(true).open(path)?;
    for page in pages {
        debug_assert!(*page > 0, "page 0 is the header case");
        file.seek(SeekFrom::Start(page * PAGE))?;
        file.write_all(&[0x5a; PAGE as usize])?;
    }
    file.sync_all()
}
