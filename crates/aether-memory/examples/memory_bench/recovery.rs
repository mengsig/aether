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

const CASES: [Case; 5] = [
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

/// A page in the middle written over with a byte pattern: damage the header
/// check cannot see.
fn garble_middle_page(path: &Path) -> std::io::Result<()> {
    const PAGE: u64 = 4_096;
    let len = std::fs::metadata(path)?.len();
    let offset = (len / 2 / PAGE) * PAGE;
    let mut file = OpenOptions::new().write(true).open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    file.write_all(&[0x5a; PAGE as usize])?;
    file.sync_all()
}
