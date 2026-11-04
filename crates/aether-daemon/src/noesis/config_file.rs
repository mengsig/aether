//! Reading and rewriting the `[noesis]` table of `config.toml`. Only that
//! table changes: every other line of the operator's file is kept as written.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use toml::{Table, Value};

const SECTION: &str = "noesis";

#[derive(Debug, thiserror::Error)]
pub(crate) enum ConfigFileError {
    #[error("reading {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("writing {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path} is not valid TOML: {reason}")]
    Parse { path: PathBuf, reason: String },
    #[error(
        "{path} sets noesis in a form this command cannot rewrite safely \
         (an inline table or dotted keys); move it into a [noesis] table and retry"
    )]
    Unrewritable { path: PathBuf },
}

/// The `config.toml` that holds the `[noesis]` table.
#[derive(Debug, Clone)]
pub(crate) struct ConfigFile {
    path: PathBuf,
}

impl ConfigFile {
    pub(crate) fn in_dir(config_dir: &Path) -> Self {
        Self {
            path: config_dir.join("config.toml"),
        }
    }

    /// The stored `[noesis]` table; empty when the file or the table is absent.
    pub(crate) fn read_noesis(&self) -> Result<Table, ConfigFileError> {
        let text = self.read_text()?;
        let mut whole = self.parse(&text)?;
        match whole.remove(SECTION) {
            None => Ok(Table::new()),
            Some(Value::Table(table)) => Ok(table),
            Some(_) => Err(ConfigFileError::Unrewritable {
                path: self.path.clone(),
            }),
        }
    }

    /// Replace the `[noesis]` table with `noesis`, atomically, and check that
    /// nothing else in the file changed meaning.
    pub(crate) fn write_noesis(&self, noesis: &Table) -> Result<(), ConfigFileError> {
        let old_text = self.read_text()?;
        let old = self.parse(&old_text)?;
        let new_text = replace_section(&old_text, noesis).map_err(|e| ConfigFileError::Parse {
            path: self.path.clone(),
            reason: format!("the new [noesis] table: {e}"),
        })?;
        // A `noesis` key outside a `[noesis]` header survives the cut and
        // collides with the new table: that file is not ours to rewrite.
        let new = self
            .parse(&new_text)
            .map_err(|_| ConfigFileError::Unrewritable {
                path: self.path.clone(),
            })?;
        if !same_outside_section(&old, &new)
            || new.get(SECTION) != Some(&Value::Table(noesis.clone()))
        {
            return Err(ConfigFileError::Unrewritable {
                path: self.path.clone(),
            });
        }
        self.write_atomically(&new_text)
    }

    fn read_text(&self) -> Result<String, ConfigFileError> {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => Ok(text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
            Err(source) => Err(ConfigFileError::Read {
                path: self.path.clone(),
                source,
            }),
        }
    }

    fn parse(&self, text: &str) -> Result<Table, ConfigFileError> {
        text.parse::<Table>().map_err(|e| ConfigFileError::Parse {
            path: self.path.clone(),
            reason: e.to_string(),
        })
    }

    /// A temporary file beside the target, renamed over it, so a crash leaves
    /// the old file or the new one and never half of either.
    fn write_atomically(&self, text: &str) -> Result<(), ConfigFileError> {
        let error = |source| ConfigFileError::Write {
            path: self.path.clone(),
            source,
        };
        let dir = self.path.parent().unwrap_or(Path::new("."));
        aether_model::config::create_config_dir(dir).map_err(error)?;
        let mut file = tempfile::NamedTempFile::new_in(dir).map_err(error)?;
        if let Ok(metadata) = std::fs::metadata(&self.path) {
            file.as_file()
                .set_permissions(metadata.permissions())
                .map_err(error)?;
        }
        file.write_all(text.as_bytes()).map_err(error)?;
        file.as_file().sync_all().map_err(error)?;
        file.persist(&self.path).map_err(|e| error(e.error))?;
        Ok(())
    }
}

/// `text` without its `[noesis]` and `[noesis.*]` tables, with `noesis`
/// appended as one table. The caller re-parses the result to prove it.
fn replace_section(text: &str, noesis: &Table) -> Result<String, toml::ser::Error> {
    let mut kept = String::with_capacity(text.len());
    let mut inside = false;
    for line in text.lines() {
        if let Some(name) = header_name(line) {
            inside = name == SECTION || name.starts_with("noesis.");
        }
        if !inside {
            kept.push_str(line);
            kept.push('\n');
        }
    }
    let trimmed = kept.trim_end();
    let mut wrapper = Table::new();
    wrapper.insert(SECTION.into(), Value::Table(noesis.clone()));
    let section = toml::to_string(&wrapper)?;
    debug_assert!(section.starts_with("[noesis]"), "{section}");
    if trimmed.is_empty() {
        return Ok(section);
    }
    Ok(format!("{trimmed}\n\n{section}"))
}

/// The table name of a `[name]` or `[[name]]` header line.
fn header_name(line: &str) -> Option<&str> {
    let line = line.trim_start();
    let inner = line.strip_prefix("[[").or_else(|| line.strip_prefix('['))?;
    let end = inner.find(']')?;
    Some(inner[..end].trim())
}

fn same_outside_section(old: &Table, new: &Table) -> bool {
    let without = |table: &Table| {
        let mut table = table.clone();
        table.remove(SECTION);
        table
    };
    without(old) == without(new)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(text: &str) -> Table {
        text.parse().unwrap()
    }

    #[test]
    fn only_the_noesis_tables_change_and_comments_elsewhere_stay() {
        let dir = tempfile::tempdir().unwrap();
        let text = "# my settings\n[model]\ndefault = \"a/b\" # keep\n\n[noesis]\nenabled = false\n\n[noesis.budget]\ninteractive_usd_daily = 1.0\n\n[listen]\nport = 1\n";
        std::fs::write(dir.path().join("config.toml"), text).unwrap();
        let file = ConfigFile::in_dir(dir.path());
        let noesis = table("enabled = true\n[budget]\ninteractive_usd_daily = 2.0\n");
        file.write_noesis(&noesis).unwrap();
        let written = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
        assert!(written.contains("# my settings"), "{written}");
        assert!(written.contains("default = \"a/b\" # keep"), "{written}");
        assert!(written.contains("[listen]\nport = 1"), "{written}");
        assert_eq!(file.read_noesis().unwrap(), noesis);
        assert_eq!(written.matches("[noesis]").count(), 1, "{written}");
    }

    #[test]
    fn a_missing_file_is_created_with_only_the_noesis_table() {
        let dir = tempfile::tempdir().unwrap();
        let file = ConfigFile::in_dir(&dir.path().join("fresh"));
        assert!(file.read_noesis().unwrap().is_empty());
        file.write_noesis(&table("enabled = true\n")).unwrap();
        assert_eq!(file.read_noesis().unwrap(), table("enabled = true\n"));
    }

    #[test]
    fn dotted_or_inline_noesis_keys_are_refused_without_a_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        for text in ["noesis.enabled = false\n", "noesis = { enabled = false }\n"] {
            std::fs::write(&path, text).unwrap();
            let error = ConfigFile::in_dir(dir.path())
                .write_noesis(&table("enabled = true\n"))
                .unwrap_err();
            assert!(
                matches!(error, ConfigFileError::Unrewritable { .. }),
                "{error}"
            );
            assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        }
    }

    #[test]
    fn invalid_toml_is_reported_and_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[model\n").unwrap();
        let file = ConfigFile::in_dir(dir.path());
        assert!(matches!(
            file.read_noesis(),
            Err(ConfigFileError::Parse { .. })
        ));
        assert!(file.write_noesis(&Table::new()).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[model\n");
    }
}
