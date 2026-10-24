use serde::{Deserialize, Serialize};

/// `[github_watcher]`: daemon-side bounds on the watcher's GitHub work. What
/// it watches is configured through the daemon API, not here.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct GithubWatcherSection {
    /// Most pull requests one pass of the review sweep reads; unset means 30.
    pub review_sweep_prs: Option<usize>,
}

pub const DEFAULT_REVIEW_SWEEP_PRS: usize = 30;

/// Each pull request costs at least one of a pass's 60 GitHub requests, so
/// a larger value could never be reached.
pub const MAX_REVIEW_SWEEP_PRS: usize = 50;

impl GithubWatcherSection {
    /// The per-pass pull request limit; a value that could not mean what it
    /// says is reported rather than clamped.
    pub fn review_sweep_prs(&self) -> Result<usize, String> {
        match self.review_sweep_prs {
            None => Ok(DEFAULT_REVIEW_SWEEP_PRS),
            Some(prs) if (1..=MAX_REVIEW_SWEEP_PRS).contains(&prs) => Ok(prs),
            Some(prs) => Err(format!(
                "github_watcher.review_sweep_prs must be between 1 and {MAX_REVIEW_SWEEP_PRS} \
                 (got {prs}); remove it for the default"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::config::{AetherConfig, load_config};

    #[test]
    fn review_sweep_prs_defaults_to_30_and_takes_a_bounded_value() {
        let config: AetherConfig = toml::from_str("").unwrap();
        assert_eq!(config.github_watcher.review_sweep_prs(), Ok(30));
        let config: AetherConfig =
            toml::from_str("[github_watcher]\nreview_sweep_prs = 12\n").unwrap();
        assert_eq!(config.github_watcher.review_sweep_prs(), Ok(12));
    }

    #[test]
    fn out_of_range_review_sweep_prs_is_rejected_at_load() {
        let dir = tempfile::tempdir().unwrap();
        for prs in [0, 51] {
            std::fs::write(
                dir.path().join("config.toml"),
                format!("[github_watcher]\nreview_sweep_prs = {prs}\n"),
            )
            .unwrap();
            let error = load_config(dir.path()).unwrap_err();
            assert!(error.reason.contains("review_sweep_prs"), "{prs}: {error}");
        }
    }

    #[test]
    fn generated_template_documents_a_working_review_sweep_override() {
        let template = super::super::CONFIG_TEMPLATE;
        let commented = "# [github_watcher]\n# review_sweep_prs = 30";
        assert!(template.contains(commented));
        let uncommented = template.replace(commented, "[github_watcher]\nreview_sweep_prs = 30");
        let config: AetherConfig = toml::from_str(&uncommented).unwrap();
        assert_eq!(config.github_watcher.review_sweep_prs, Some(30));
    }
}
