//! Artifact bytes for the pager.
use super::{ARTIFACT_VIEW_BYTES, App, ArtifactId, Command, Effect, Then};

impl App {
    /// Ask for an artifact's bytes; the pager shows them once they land.
    pub(super) fn read_artifact(&mut self, id: ArtifactId) -> Vec<Effect> {
        self.dirty = true;
        self.artifact_reads += 1;
        let read = self.artifact_reads;
        self.artifacts.pending_bytes = Some((id, read));
        vec![Effect::Request {
            command: Command::ReadArtifact {
                id,
                max_bytes: ARTIFACT_VIEW_BYTES,
            },
            then: Then::ArtifactBytes { id, read },
        }]
    }
}
