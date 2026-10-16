use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

/// `[build_cache]`: where builds of agents in a repository workspace keep
/// their output, outside the workspace and shared by the agents of one run.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct BuildCacheSection {
    /// Root of the per-repository caches. Unset means `aether.build-cache`
    /// beside `aether.db`. Taken as written: no `~` or environment expansion.
    pub root: Option<PathBuf>,
    /// Variables pointed at a directory of the agent's cache partition. An
    /// empty list exports nothing.
    pub env: Vec<BuildCacheVar>,
}

/// One exported variable and the directory, inside the partition, it names.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BuildCacheVar {
    pub name: String,
    pub dir: String,
}

/// Variables an agent's temp dir already owns; a cache must not replace them.
const RESERVED: [&str; 3] = ["TMPDIR", "TMP", "TEMP"];

impl Default for BuildCacheSection {
    fn default() -> Self {
        Self {
            root: None,
            env: vec![BuildCacheVar {
                name: "CARGO_TARGET_DIR".into(),
                dir: "cargo-target".into(),
            }],
        }
    }
}

impl BuildCacheSection {
    /// The root becomes an environment value and the variables become
    /// process environment, so each must be plain, distinct and absolute.
    pub fn validate(&self) -> Result<(), String> {
        if let Some(root) = &self.root {
            check_root(root)?;
        }
        let mut names = BTreeSet::new();
        let mut dirs = BTreeSet::new();
        for var in &self.env {
            if !is_variable_name(&var.name) || RESERVED.contains(&var.name.as_str()) {
                return Err(format!(
                    "build_cache.env: `{}` must be a variable name ([A-Za-z_][A-Za-z0-9_]*) \
                     other than TMPDIR, TMP or TEMP",
                    var.name
                ));
            }
            let plain = Path::new(&var.dir).components().collect::<Vec<_>>();
            if !matches!(plain.as_slice(), [Component::Normal(part)] if *part == var.dir.as_str()) {
                return Err(format!(
                    "build_cache.env: dir `{}` of {} must be one plain directory name",
                    var.dir, var.name
                ));
            }
            if !names.insert(var.name.as_str()) || !dirs.insert(var.dir.as_str()) {
                return Err(format!(
                    "build_cache.env: {} or its dir `{}` is listed twice",
                    var.name, var.dir
                ));
            }
        }
        Ok(())
    }
}

fn check_root(root: &Path) -> Result<(), String> {
    let shown = root.display();
    if root.to_str().is_none() {
        return Err(format!("build_cache.root `{shown}` is not valid UTF-8"));
    }
    if !root.is_absolute() {
        return Err(format!(
            "build_cache.root `{shown}` must be an absolute path; `~` and variables are not \
             expanded"
        ));
    }
    if root.components().any(|c| c == Component::ParentDir) {
        return Err(format!("build_cache.root `{shown}` must not contain `..`"));
    }
    if root.parent().is_none() {
        return Err("build_cache.root cannot be the filesystem root".into());
    }
    Ok(())
}

fn is_variable_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use crate::config::{AetherConfig, load_config};

    #[test]
    fn defaults_export_the_cargo_target_and_a_table_replaces_them() {
        let config: AetherConfig = toml::from_str("").unwrap();
        assert_eq!(config.build_cache.root, None);
        let names: Vec<_> = config.build_cache.env.iter().map(|v| &v.name).collect();
        assert_eq!(names, ["CARGO_TARGET_DIR"]);
        assert_eq!(config.build_cache.env[0].dir, "cargo-target");
        assert_eq!(config.build_cache.validate(), Ok(()));

        let config: AetherConfig = toml::from_str(
            "[build_cache]\nroot = \"/srv/cache\"\n\
             [[build_cache.env]]\nname = \"npm_config_cache\"\ndir = \"npm\"\n",
        )
        .unwrap();
        assert_eq!(config.build_cache.validate(), Ok(()));
        assert_eq!(
            config.build_cache.root.as_deref(),
            Some("/srv/cache".as_ref())
        );
        let names: Vec<_> = config.build_cache.env.iter().map(|v| &v.name).collect();
        assert_eq!(names, ["npm_config_cache"]);
        let config: AetherConfig = toml::from_str("[build_cache]\nenv = []\n").unwrap();
        assert!(config.build_cache.env.is_empty());
    }

    #[test]
    fn invalid_build_caches_are_rejected_at_load() {
        let dir = tempfile::tempdir().unwrap();
        let var = |name: &str, dir: &str| {
            format!("[[build_cache.env]]\nname = \"{name}\"\ndir = \"{dir}\"\n")
        };
        for setting in [
            "[build_cache]\nroot = \"relative/cache\"\n".to_owned(),
            "[build_cache]\nroot = \"~/cache\"\n".into(),
            "[build_cache]\nroot = \"/srv/../cache\"\n".into(),
            "[build_cache]\nroot = \"/\"\n".into(),
            "[build_cache]\nrooot = \"/srv/cache\"\n".into(),
            var("TMPDIR", "tmp"),
            var("1CARGO", "cargo"),
            var("CARGO TARGET", "cargo"),
            var("CARGO_TARGET_DIR", "a/b"),
            var("CARGO_TARGET_DIR", ".."),
            var("CARGO_TARGET_DIR", ""),
            format!("{}{}", var("A", "same"), var("B", "same")),
            format!("{}{}", var("A", "one"), var("A", "two")),
        ] {
            std::fs::write(dir.path().join("config.toml"), &setting).unwrap();
            assert!(load_config(dir.path()).is_err(), "{setting}");
        }
    }
}
