//! What an operator sees and decides about a child's workspace: its diff,
//! an integration into the source it came from, and a discard.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::id::AgentId;
use crate::time::Timestamp;

/// Bytes of patch text a diff carries at most, before the truncation marker.
pub const PATCH_LIMIT: usize = 256 * 1024;
/// Changed files a diff lists at most; the diffstat still counts them all.
pub const FILE_LIMIT: usize = 1000;
/// Commits a diff lists at most, newest first.
pub const COMMIT_LIMIT: usize = 200;
/// Characters of a commit subject a diff keeps.
pub const SUMMARY_LIMIT: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ChangedFile {
    pub path: String,
    pub status: FileStatus,
    /// Line counts; both absent for a binary file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub insertions: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deletions: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkspaceCommit {
    pub id: String,
    pub author: String,
    pub at: Timestamp,
    pub summary: String,
    /// The agent turn whose start and end HEAD bracket this commit, when
    /// Aether recorded both.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<u32>,
}

/// An agent's work in its workspace, bounded for the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkspaceDiff {
    pub agent: AgentId,
    pub workspace: PathBuf,
    /// The repository an isolated workspace integrates into.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<PathBuf>,
    /// The workspace is a managed worktree of this agent's own. Otherwise it
    /// is shared, and files and commits come from its turns' HEAD snapshots.
    pub isolated: bool,
    /// Other agents may have committed in the same workspace during this
    /// agent's turns, so files and commits can include their work.
    pub ambiguous: bool,
    /// The work was integrated into `source` already.
    pub integrated: bool,
    pub files: Vec<ChangedFile>,
    /// More than [`FILE_LIMIT`] files changed; the rest are not listed.
    pub files_truncated: bool,
    /// Lines added and removed across every changed file, listed or not.
    pub insertions: u64,
    pub deletions: u64,
    pub commits: Vec<WorkspaceCommit>,
    /// More than [`COMMIT_LIMIT`] commits; the oldest are not listed.
    pub commits_truncated: bool,
    /// At most [`PATCH_LIMIT`] bytes of the patch, then the truncation
    /// marker when `patch_truncated`.
    pub patch: String,
    /// Size of the whole patch.
    pub patch_bytes: u64,
    pub patch_truncated: bool,
}

/// The result of an operator's integration. Refused with `conflicts` when
/// the source changed a path the child also changed; nothing was written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkspaceIntegration {
    pub agent: AgentId,
    pub source: PathBuf,
    pub integrated: bool,
    pub already_integrated: bool,
    pub changed_paths: Vec<String>,
    pub conflicts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkspaceDiscard {
    pub agent: AgentId,
    pub workspace: PathBuf,
    /// An agent using the workspace was still live.
    pub forced: bool,
}

/// A patch cut to [`PATCH_LIMIT`] bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CappedPatch {
    pub text: String,
    pub bytes: u64,
    pub truncated: bool,
}

/// The line that ends a truncated patch.
pub fn truncation_marker(shown: usize, total: u64) -> String {
    format!("\n[patch truncated: showing {shown} of {total} bytes]\n")
}

/// Cut `patch` to [`PATCH_LIMIT`] bytes of text on a character boundary and
/// mark the cut. `total` is the whole patch's size, which may exceed what was read.
pub fn cap_patch(patch: &[u8], total: u64) -> CappedPatch {
    debug_assert!(
        total >= patch.len() as u64,
        "a patch's total size covers the bytes read"
    );
    // Replacement characters can make the text longer than the raw bytes.
    let text = String::from_utf8_lossy(patch);
    if text.len() <= PATCH_LIMIT && patch.len() as u64 == total {
        return CappedPatch {
            text: text.into_owned(),
            bytes: total,
            truncated: false,
        };
    }
    let mut end = text.len().min(PATCH_LIMIT);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut shown = text[..end].to_owned();
    shown.push_str(&truncation_marker(end, total));
    debug_assert!(shown.len() <= PATCH_LIMIT + truncation_marker(end, total).len());
    CappedPatch {
        text: shown,
        bytes: total,
        truncated: true,
    }
}

/// Keep the first [`SUMMARY_LIMIT`] characters of a commit subject.
pub fn cap_summary(summary: &str) -> String {
    match summary.char_indices().nth(SUMMARY_LIMIT) {
        Some((end, _)) => format!("{}…", &summary[..end]),
        None => summary.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_small_patch_is_kept_whole() {
        let capped = cap_patch(b"diff --git a/x b/x\n", 19);
        assert!(!capped.truncated);
        assert_eq!(capped.text, "diff --git a/x b/x\n");
        assert_eq!(capped.bytes, 19);
    }

    #[test]
    fn a_large_patch_is_cut_on_a_character_boundary_and_marked() {
        let mut patch = vec![b'a'; PATCH_LIMIT - 1];
        patch.extend_from_slice("é and more".as_bytes());
        let total = patch.len() as u64;
        let capped = cap_patch(&patch, total);
        assert!(capped.truncated);
        let marker = truncation_marker(PATCH_LIMIT - 1, total);
        assert!(
            capped.text.ends_with(&marker),
            "{}",
            &capped.text[PATCH_LIMIT - 8..]
        );
        assert_eq!(capped.text.len(), PATCH_LIMIT - 1 + marker.len());
    }

    #[test]
    fn a_patch_read_short_of_its_total_is_marked_even_under_the_limit() {
        let capped = cap_patch(b"partial", 10 * PATCH_LIMIT as u64);
        assert!(capped.truncated);
        assert!(capped.text.starts_with("partial\n[patch truncated"));
    }

    #[test]
    fn a_patch_that_grows_when_decoded_is_still_cut_to_the_limit() {
        let patch = vec![0xff; PATCH_LIMIT / 2];
        let capped = cap_patch(&patch, patch.len() as u64);
        assert!(capped.truncated);
        let marker = truncation_marker(PATCH_LIMIT - PATCH_LIMIT % 3, patch.len() as u64);
        assert!(
            capped.text.ends_with(&marker),
            "{}",
            &capped.text[PATCH_LIMIT - 8..]
        );
        assert!(capped.text.len() <= PATCH_LIMIT + marker.len());
    }

    #[test]
    fn long_subjects_are_shortened() {
        let long = "x".repeat(SUMMARY_LIMIT + 5);
        assert_eq!(cap_summary(&long).chars().count(), SUMMARY_LIMIT + 1);
        assert_eq!(cap_summary("short"), "short");
    }
}
