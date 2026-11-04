//! Built-in prompt segments of the Noesis teaching agents, and the composer
//! that joins them, optionally with operator overrides.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

/// Largest override file accepted, in bytes.
const MAX_OVERRIDE_BYTES: u64 = 64 * 1024;

/// The three teaching agents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Agent {
    Prober,
    Teacher,
    Practitioner,
}

impl Agent {
    pub const ALL: [Agent; 3] = [Agent::Prober, Agent::Teacher, Agent::Practitioner];

    pub fn name(self) -> &'static str {
        match self {
            Agent::Prober => "prober",
            Agent::Teacher => "teacher",
            Agent::Practitioner => "practitioner",
        }
    }
}

/// One built-in piece of an agent's prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    /// `NN-kebab-name`, also the override file stem.
    pub name: &'static str,
    pub text: &'static str,
}

const PROBER: [Segment; 4] = [
    Segment {
        name: "01-role",
        text: include_str!("prober/01-role.md"),
    },
    Segment {
        name: "02-method",
        text: include_str!("prober/02-method.md"),
    },
    Segment {
        name: "03-tools",
        text: include_str!("prober/03-tools.md"),
    },
    Segment {
        name: "04-rules",
        text: include_str!("prober/04-rules.md"),
    },
];

const TEACHER: [Segment; 4] = [
    Segment {
        name: "01-role",
        text: include_str!("teacher/01-role.md"),
    },
    Segment {
        name: "02-method",
        text: include_str!("teacher/02-method.md"),
    },
    Segment {
        name: "03-tools",
        text: include_str!("teacher/03-tools.md"),
    },
    Segment {
        name: "04-rules",
        text: include_str!("teacher/04-rules.md"),
    },
];

const PRACTITIONER: [Segment; 4] = [
    Segment {
        name: "01-role",
        text: include_str!("practitioner/01-role.md"),
    },
    Segment {
        name: "02-method",
        text: include_str!("practitioner/02-method.md"),
    },
    Segment {
        name: "03-tools",
        text: include_str!("practitioner/03-tools.md"),
    },
    Segment {
        name: "04-rules",
        text: include_str!("practitioner/04-rules.md"),
    },
];

/// The built-in segments of `agent`, in numeric order.
pub fn segments(agent: Agent) -> &'static [Segment] {
    let list: &'static [Segment] = match agent {
        Agent::Prober => &PROBER,
        Agent::Teacher => &TEACHER,
        Agent::Practitioner => &PRACTITIONER,
    };
    debug_assert!(!list.is_empty(), "{} has no segments", agent.name());
    debug_assert!(
        list.windows(2)
            .all(|w| segment_number(w[0].name) < segment_number(w[1].name)),
        "{} segments are not strictly increasing",
        agent.name()
    );
    list
}

fn segment_number(name: &str) -> Option<u32> {
    name.split('-').next()?.parse().ok()
}

/// An agent prompt ready to use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposedPrompt {
    pub text: String,
    /// Names of the segments replaced by an override file.
    pub overridden: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum PromptError {
    #[error("prompt override {path} is a symlink, which is refused")]
    Symlink { path: PathBuf },
    #[error("prompt override {path} is not a regular file")]
    NotRegularFile { path: PathBuf },
    #[error("prompt override {path} is {size} bytes, over the {limit} byte limit")]
    TooLarge {
        path: PathBuf,
        size: u64,
        limit: u64,
    },
    #[error("prompt override {path} is not valid UTF-8")]
    NotUtf8 {
        path: PathBuf,
        #[source]
        source: std::string::FromUtf8Error,
    },
    #[error("cannot read prompt override {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// Builds the prompt of `agent`. `overrides` is the override directory, or
/// `None` when the operator switch is off.
pub fn compose(agent: Agent, overrides: Option<&Path>) -> Result<ComposedPrompt, PromptError> {
    let builtin = segments(agent);
    let mut parts: Vec<String> = Vec::with_capacity(builtin.len());
    let mut overridden = Vec::new();
    for segment in builtin {
        let replacement = match overrides {
            Some(dir) => read_override(dir, agent, segment.name)?,
            None => None,
        };
        match replacement {
            Some(text) => {
                overridden.push(segment.name.to_string());
                parts.push(text);
            }
            None => parts.push(segment.text.to_string()),
        }
    }
    debug_assert_eq!(parts.len(), builtin.len());
    debug_assert!(overridden.len() <= builtin.len());
    Ok(ComposedPrompt {
        text: parts.join("\n\n"),
        overridden,
    })
}

/// Sorted `"<agent>/<segment>"` names of the override files that exist.
pub fn override_names(overrides: &Path) -> Result<Vec<String>, PromptError> {
    let mut names = Vec::new();
    for agent in Agent::ALL {
        for segment in segments(agent) {
            if read_override(overrides, agent, segment.name)?.is_some() {
                names.push(format!("{}/{}", agent.name(), segment.name));
            }
        }
    }
    names.sort();
    debug_assert!(
        names.windows(2).all(|w| w[0] < w[1]),
        "duplicate override names"
    );
    Ok(names)
}

/// Reads one override file; `Ok(None)` only when the file or directory is absent.
fn read_override(dir: &Path, agent: Agent, segment: &str) -> Result<Option<String>, PromptError> {
    let path = dir.join(agent.name()).join(format!("{segment}.md"));
    let io_err = |source| PromptError::Io {
        path: path.clone(),
        source,
    };
    let meta = match fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io_err(e)),
    };
    if meta.file_type().is_symlink() {
        return Err(PromptError::Symlink { path });
    }
    if !meta.is_file() {
        return Err(PromptError::NotRegularFile { path });
    }
    if meta.len() > MAX_OVERRIDE_BYTES {
        return Err(PromptError::TooLarge {
            path,
            size: meta.len(),
            limit: MAX_OVERRIDE_BYTES,
        });
    }
    // Bounded read guards against the file growing after the metadata check.
    let mut bytes = Vec::new();
    let file = fs::File::open(&path).map_err(io_err)?;
    file.take(MAX_OVERRIDE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(io_err)?;
    if bytes.len() as u64 > MAX_OVERRIDE_BYTES {
        return Err(PromptError::TooLarge {
            path,
            size: bytes.len() as u64,
            limit: MAX_OVERRIDE_BYTES,
        });
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|source| PromptError::NotUtf8 { path, source })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, agent: &str, segment: &str, bytes: &[u8]) -> PathBuf {
        let folder = dir.join(agent);
        fs::create_dir_all(&folder).unwrap();
        let path = folder.join(format!("{segment}.md"));
        fs::write(&path, bytes).unwrap();
        path
    }

    fn builtin_join(agent: Agent) -> String {
        let texts: Vec<&str> = segments(agent).iter().map(|s| s.text).collect();
        texts.join("\n\n")
    }

    #[test]
    fn segments_are_ordered_and_named() {
        for agent in Agent::ALL {
            let list = segments(agent);
            assert!(!list.is_empty());
            let numbers: Vec<u32> = list
                .iter()
                .map(|s| segment_number(s.name).expect("NN- prefix"))
                .collect();
            assert!(numbers.windows(2).all(|w| w[0] < w[1]), "{numbers:?}");
            for segment in list {
                assert!(
                    segment
                        .name
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
                );
                assert!(!segment.text.trim().is_empty());
            }
        }
    }

    #[test]
    fn compose_without_overrides_joins_builtins() {
        for agent in Agent::ALL {
            let composed = compose(agent, None).unwrap();
            assert_eq!(composed.text, builtin_join(agent));
            assert!(composed.overridden.is_empty());
        }
    }

    #[test]
    fn override_replaces_only_its_segment() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "teacher", "02-method", b"custom method");
        write(dir.path(), "teacher", "99-unknown", b"ignored");
        let composed = compose(Agent::Teacher, Some(dir.path())).unwrap();
        assert_eq!(composed.overridden, vec!["02-method".to_string()]);
        assert!(composed.text.contains("custom method"));
        assert!(!composed.text.contains("ignored"));
        let builtin = segments(Agent::Teacher);
        assert!(composed.text.starts_with(builtin[0].text));
        assert!(composed.text.ends_with(builtin[3].text));
        assert!(!composed.text.contains(builtin[1].text));
        let other = compose(Agent::Prober, Some(dir.path())).unwrap();
        assert_eq!(other.text, builtin_join(Agent::Prober));
    }

    #[test]
    fn none_ignores_existing_override_files() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "prober", "01-role", b"custom");
        let composed = compose(Agent::Prober, None).unwrap();
        assert_eq!(composed.text, builtin_join(Agent::Prober));
    }

    #[test]
    fn missing_file_or_directory_keeps_builtin() {
        let dir = tempfile::tempdir().unwrap();
        let empty = compose(Agent::Prober, Some(dir.path())).unwrap();
        assert_eq!(empty.text, builtin_join(Agent::Prober));
        let absent = dir.path().join("does-not-exist");
        assert!(
            compose(Agent::Prober, Some(&absent))
                .unwrap()
                .overridden
                .is_empty()
        );
        write(dir.path(), "prober", "02-method", b"x");
        let partial = compose(Agent::Prober, Some(dir.path())).unwrap();
        assert_eq!(partial.overridden, vec!["02-method".to_string()]);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let target = write(dir.path(), "teacher", "target-file", b"secret");
        let link = dir.path().join("teacher/01-role.md");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let err = compose(Agent::Teacher, Some(dir.path())).unwrap_err();
        assert!(matches!(err, PromptError::Symlink { .. }), "{err}");
        assert!(override_names(dir.path()).is_err());
    }

    #[test]
    fn directory_in_place_of_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("prober/01-role.md")).unwrap();
        let err = compose(Agent::Prober, Some(dir.path())).unwrap_err();
        assert!(matches!(err, PromptError::NotRegularFile { .. }), "{err}");
    }

    #[test]
    fn oversized_file_is_an_error_and_the_limit_is_inclusive() {
        let dir = tempfile::tempdir().unwrap();
        let limit = MAX_OVERRIDE_BYTES as usize;
        write(dir.path(), "prober", "01-role", &vec![b'a'; limit]);
        assert!(compose(Agent::Prober, Some(dir.path())).is_ok());
        write(dir.path(), "prober", "01-role", &vec![b'a'; limit + 1]);
        let err = compose(Agent::Prober, Some(dir.path())).unwrap_err();
        assert!(matches!(err, PromptError::TooLarge { .. }), "{err}");
    }

    #[test]
    fn non_utf8_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "practitioner", "04-rules", &[0xff, 0xfe, 0x00]);
        let err = compose(Agent::Practitioner, Some(dir.path())).unwrap_err();
        assert!(matches!(err, PromptError::NotUtf8 { .. }), "{err}");
        assert!(err.to_string().contains("04-rules.md"));
    }

    #[test]
    fn override_names_lists_only_matching_files_sorted() {
        let dir = tempfile::tempdir().unwrap();
        assert!(override_names(dir.path()).unwrap().is_empty());
        write(dir.path(), "teacher", "03-tools", b"a");
        write(dir.path(), "prober", "01-role", b"b");
        write(dir.path(), "prober", "09-extra", b"c");
        write(dir.path(), "stranger", "01-role", b"d");
        let names = override_names(dir.path()).unwrap();
        assert_eq!(
            names,
            vec!["prober/01-role".to_string(), "teacher/03-tools".to_string()]
        );
    }

    #[test]
    fn prompts_mention_their_tools() {
        let expected: [(Agent, &[&str]); 3] = [
            (Agent::Prober, &["noesis.ask", "noesis.probe_done"]),
            (Agent::Teacher, &["noesis.plan", "noesis.ask"]),
            (Agent::Practitioner, &["noesis.problem", "noesis.conclude"]),
        ];
        for (agent, tools) in expected {
            let text = compose(agent, None).unwrap().text;
            for tool in tools {
                assert!(text.contains(tool), "{} lacks {tool}", agent.name());
            }
        }
    }
}
