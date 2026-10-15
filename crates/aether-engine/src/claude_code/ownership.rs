use std::path::{Path, PathBuf};

use crate::EngineTurn;

pub(super) fn session_log_paths(turn: &EngineTurn) -> Vec<PathBuf> {
    if turn.session.is_some() || turn.session_recorder.is_none() {
        return Vec::new();
    }
    let env = |name: &str| {
        (!turn.hidden_env.iter().any(|hidden| hidden == name))
            .then(|| std::env::var_os(name))
            .flatten()
    };
    // Native project-name overrides and long/unicode paths have version-dependent
    // encodings. Unknown paths must not become evidence for later cleanup.
    if env("CLAUDE_CODE_PROJECT_DIR_NAME").is_some() {
        return Vec::new();
    }
    let config = env("CLAUDE_CONFIG_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| env("HOME").map(|home| PathBuf::from(home).join(".claude")));
    let Some(config) = config else {
        return Vec::new();
    };
    let Ok(cwd) = std::fs::canonicalize(&turn.cwd) else {
        return Vec::new();
    };
    session_log_path(&config, &cwd, &turn.new_session_id)
        .into_iter()
        .collect()
}

fn session_log_path(config: &Path, cwd: &Path, id: &str) -> Option<PathBuf> {
    let cwd = cwd.to_str()?;
    let config_text = config.to_str()?;
    if !config.is_absolute()
        || config
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
        || !config_text.is_ascii()
        || !cwd.is_ascii()
        || cwd.len() > 200
        || id.is_empty()
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
    {
        return None;
    }
    // Matches Anthropic's SDK _sanitize_path for its unambiguous short ASCII case:
    // github.com/anthropics/claude-agent-sdk-python/blob/main/src/claude_agent_sdk/_internal/sessions.py
    let project: String = cwd
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    Some(
        config
            .join("projects")
            .join(project)
            .join(format!("{id}.jsonl")),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_claude_transcript_path_uses_only_the_fresh_id_and_exact_project() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("claude-profile");
        let cwd = dir.path().join("project.with spaces");
        let expected_project: String = cwd
            .to_str()
            .unwrap()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        let id = "11111111-2222-4333-8444-555555555555";
        assert_eq!(
            session_log_path(&config, &cwd, id),
            Some(
                config
                    .join("projects")
                    .join(expected_project)
                    .join(format!("{id}.jsonl"))
            )
        );
        assert!(
            !config.exists(),
            "resolving ownership must not create or scan native storage"
        );
    }

    #[test]
    fn uncertain_claude_path_encodings_stay_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("profile");
        for cwd in [dir.path().join("x".repeat(201)), dir.path().join("café")] {
            assert_eq!(session_log_path(&config, &cwd, "abc-123"), None);
        }
        assert_eq!(
            session_log_path(Path::new("relative"), dir.path(), "abc-123"),
            None
        );
        assert_eq!(
            session_log_path(&config, dir.path(), "../user-session"),
            None
        );
    }
}
