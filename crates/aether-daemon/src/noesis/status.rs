//! `noesis_status`: the stored and effective settings, today's spend, the
//! disable policy and the prompt files in force. Reads only: while disabled
//! nothing under the data directory is touched.

use std::path::Path;
use std::sync::Arc;

use aether_noesis::settings::NoesisSettings;
use aether_protocol::ErrorReply;
use aether_protocol::noesis::{BackgroundDisable, DisablePolicy, NoesisStatus, SpendToday};
use aether_runtime::Runtime;
use aether_runtime::noesis_guard::NoesisLane;

use super::{NoesisService, budget, view_of};

/// More prompt files than this would not be a set of overrides anyone wrote.
const MAX_PROMPT_FILES: usize = 256;

impl NoesisService {
    pub(super) async fn status(&self, runtime: &Arc<Runtime>) -> Result<NoesisStatus, ErrorReply> {
        let stored = self.config.read_noesis().map_err(ErrorReply::internal)?;
        let settings =
            NoesisSettings::from_table(&stored).map_err(|e| ErrorReply::invalid(e.to_string()))?;
        let effective = self.effective(&settings);
        let store = runtime.store().clone();
        let day = budget::today();
        let (interactive, background) =
            tokio::task::spawn_blocking(move || budget::usage_today(store.as_ref(), day))
                .await
                .map_err(|e| ErrorReply::internal(format!("the spend read did not finish: {e}")))?
                .map_err(crate::connection::store_err)?;
        let jobs_running = runtime
            .noesis_turns_running()
            .iter()
            .filter(|(_, lane)| *lane == NoesisLane::Background)
            .count();
        let prompt_overrides = if effective.enabled && effective.prompt_overrides {
            prompt_files(&self.data_dir).map_err(ErrorReply::internal)?
        } else {
            Vec::new()
        };
        Ok(NoesisStatus {
            enabled: settings.enabled,
            settings: view_of(&settings)?,
            effective: view_of(&effective)?,
            jobs_running: u32::try_from(jobs_running).unwrap_or(u32::MAX),
            spend_today: SpendToday {
                interactive,
                background,
            },
            disable_policy: DisablePolicy {
                grace_seconds: settings.disable_grace_seconds,
                background: BackgroundDisable::CancelAtModelBoundary,
            },
            prompt_overrides,
        })
    }
}

/// `<agent>/<NN-name>` for each `<data dir>/noesis/prompts/<agent>/<NN-name>.md`,
/// sorted. A missing directory means no overrides.
fn prompt_files(data_dir: &Path) -> std::io::Result<Vec<String>> {
    let root = data_dir.join("noesis").join("prompts");
    let agents = match std::fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut found = Vec::new();
    for agent in agents {
        let agent = agent?;
        if !agent.file_type()?.is_dir() {
            continue;
        }
        for segment in std::fs::read_dir(agent.path())? {
            let path = segment?.path();
            let (Some(agent_name), Some(stem)) = (
                agent.file_name().to_str().map(str::to_owned),
                path.file_stem(),
            ) else {
                continue;
            };
            if path.extension().is_some_and(|ext| ext == "md") && found.len() < MAX_PROMPT_FILES {
                found.push(format!("{agent_name}/{}", stem.to_string_lossy()));
            }
        }
    }
    found.sort();
    debug_assert!(found.len() <= MAX_PROMPT_FILES);
    Ok(found)
}
