//! Durable memory: cold by default, retrieved selectively, always with provenance.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::id::{AgentId, ArtifactId, EventId, MemoryId, TaskId};
use crate::time::Timestamp;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "scope", rename_all = "snake_case")]
pub enum MemoryScope {
    Global,
    Repository {
        id: String,
    },
    Role {
        role: String,
    },
    Agent {
        id: AgentId,
    },
    /// A repository's notes for one role family: agents in repository `id`
    /// whose role is `role` or extends it at a dot (see [`role_families`]).
    RepositoryRole {
        id: String,
        role: String,
    },
    /// Recorded by `root` for itself and every agent beneath it.
    Subtree {
        root: AgentId,
    },
}

impl MemoryScope {
    /// Whether a memory in `self` is visible to a query in `query`.
    /// Global is visible everywhere; other scopes must match exactly.
    pub fn visible_to(&self, query: &MemoryScope) -> bool {
        matches!(self, Self::Global) || self == query
    }
}

/// Most dot-separated segments a role family can have. A repository's role
/// directory is refused past this, so a longer prefix of a role matches
/// nothing and is never asked for.
pub const MAX_ROLE_SEGMENTS: usize = 8;

/// The role families `role` belongs to, most specific first:
/// `team.steward` is in `team.steward` and `team`. Matching is by whole
/// segment, so `team` never contains `teammate`. Prefixes longer than
/// [`MAX_ROLE_SEGMENTS`] are left out: no role directory can name them.
pub fn role_families(role: &str) -> Vec<&str> {
    let mut families: Vec<&str> = role
        .match_indices('.')
        .map(|(at, _)| &role[..at])
        .chain(std::iter::once(role))
        .filter(|family| !family.is_empty())
        .take(MAX_ROLE_SEGMENTS)
        .collect();
    families.reverse();
    families
}

/// Whether `name` is a role family a repository may file notes under:
/// 1 to [`MAX_ROLE_SEGMENTS`] dot-separated segments of ASCII letters,
/// digits, `-` and `_`. Conservative on purpose - it becomes a scope key, a
/// label in listings and a word in the system prompt.
pub fn is_role_family(name: &str) -> bool {
    let segments: Vec<&str> = name.split('.').collect();
    segments.len() <= MAX_ROLE_SEGMENTS
        && segments.iter().all(|segment| {
            !segment.is_empty()
                && segment
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    Fact,
    Decision,
    Lesson,
    Constraint,
    Preference,
}

/// A committed file a memory was read from, and the commit it was read at.
/// The commit is absent when the tree is not a git repository, or git is not
/// on the machine: the note still stands, its version is just unknown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct MemorySource {
    /// Path relative to the repository root.
    pub file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
}

impl MemorySource {
    /// `file@abcdef1`, or just the file when the commit is unknown.
    pub fn label(&self) -> String {
        match &self.commit {
            // By chars, not bytes: this came out of a subprocess, and byte
            // slicing panics on a char boundary for anything but a real sha.
            Some(c) => format!("{}@{}", self.file, c.chars().take(7).collect::<String>()),
            None => self.file.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct MemoryProvenance {
    /// Authenticated actor recorded by the runtime, never supplied with content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<crate::event::Actor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<TaskId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<EventId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<ArtifactId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentId>,
    /// The committed note this came from, for repository knowledge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<MemorySource>,
}

/// Who recorded a memory, as a query names it: read from the runtime-set
/// `provenance.actor`, never from anything an author supplied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryRecorder {
    /// An operator of this daemon, local or authenticated remotely. An imported
    /// operator acted on another daemon, so - like the prompt's label - it does not count.
    Operator,
}

impl MemoryRecorder {
    pub fn recorded(self, provenance: &MemoryProvenance) -> bool {
        match self {
            Self::Operator => matches!(provenance.actor, Some(crate::event::Actor::User { .. })),
        }
    }
}

/// Operator applicability, not authorship. Repository identity is resolved by the runtime.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "scope", rename_all = "snake_case", deny_unknown_fields)]
pub enum OperatorMemoryScope {
    Agent {
        id: AgentId,
    },
    Global,
    Repository {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        root: Option<std::path::PathBuf>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        repository: Option<String>,
    },
    Role {
        role: String,
    },
    RepositoryRole {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        root: Option<std::path::PathBuf>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        repository: Option<String>,
        role: String,
    },
}

/// A person supplies content and applicability, never fabricated provenance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OperatorMemorySpec {
    pub text: String,
    pub kind: MemoryKind,
    pub scope: OperatorMemoryScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub importance: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<MemoryId>,
    /// Store for operator review without making it available to recall.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub proposed: bool,
}

/// What one pass of reading a repository's committed notes did.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
pub struct SeedReport {
    /// Notes read from the repository's files.
    pub found: usize,
    pub admitted: usize,
    pub superseded: usize,
    pub rejected: usize,
    /// Already stored word for word; not offered again.
    pub unchanged: usize,
    /// Seeded before, and no file states it any more: curated out with the
    /// reason, because the label promises the fact can still be checked.
    #[serde(default)]
    pub retracted: usize,
}

/// A proposed memory. Admission decides whether it becomes a record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct MemoryCandidate {
    pub scope: MemoryScope,
    pub kind: MemoryKind,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured: Option<Value>,
    /// 0.0 to 1.0: how much it should influence future work.
    #[serde(default = "half")]
    pub importance: f32,
    /// 0.0 to 1.0: how sure the author is.
    #[serde(default = "half")]
    pub confidence: f32,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub provenance: MemoryProvenance,
    /// The record this one replaces, when the author knows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<MemoryId>,
}

fn half() -> f32 {
    0.5
}

/// A stored memory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct MemoryRecord {
    pub id: MemoryId,
    pub scope: MemoryScope,
    pub kind: MemoryKind,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured: Option<Value>,
    pub importance: f32,
    pub confidence: f32,
    #[serde(default)]
    pub tags: Vec<String>,
    pub provenance: MemoryProvenance,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<MemoryId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<MemoryId>,
    pub created_at: Timestamp,
    /// When review was requested; the proposer is the authenticated provenance actor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposed_at: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted_at: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_retrieved_at: Option<Timestamp>,
    #[serde(default)]
    pub retrieval_count: u32,
    /// Kept in front of the model: a pinned memory outranks the rest.
    #[serde(default)]
    pub pinned: bool,
    /// Curated out. The record stays - history is never deleted - but it is
    /// never retrieved again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forgotten_at: Option<Timestamp>,
    /// Set when a sweep curated it out rather than a person. `None` is a
    /// person's decision, which outranks the file for good.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retracted: Option<Retraction>,
}

/// Why a repository-knowledge sweep curated a record out: the committed tree
/// named here no longer states it, so the `file@sha` label it carried is no
/// longer a promise anyone can check.
///
/// Distinct from a person's `forget` on purpose. A person's curation outranks
/// the file however many times the note comes back; a retraction is undone by
/// the note reappearing in the tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Retraction {
    /// The file that stated it, relative to the repository root.
    pub file: String,
    /// The commit whose tree was checked and does not state it.
    pub commit: String,
    pub at: Timestamp,
}

impl MemoryRecord {
    pub fn from_candidate(candidate: MemoryCandidate, at: Timestamp) -> Self {
        Self {
            id: MemoryId::new(),
            scope: candidate.scope,
            kind: candidate.kind,
            text: candidate.text,
            structured: candidate.structured,
            importance: candidate.importance.clamp(0.0, 1.0),
            confidence: candidate.confidence.clamp(0.0, 1.0),
            tags: candidate.tags,
            provenance: candidate.provenance,
            supersedes: candidate.supersedes,
            superseded_by: None,
            created_at: at,
            proposed_at: None,
            accepted_at: None,
            last_retrieved_at: None,
            retrieval_count: 0,
            pinned: false,
            forgotten_at: None,
            retracted: None,
        }
    }

    pub fn is_proposed(&self) -> bool {
        self.proposed_at.is_some() && self.accepted_at.is_none()
    }

    /// Available to recall: accepted, not replaced and not forgotten.
    pub fn is_current(&self) -> bool {
        !self.is_proposed() && self.superseded_by.is_none() && self.forgotten_at.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn operator_role_scopes_round_trip_and_refuse_unknown_fields() {
        let cases = [
            (
                OperatorMemoryScope::Role {
                    role: "reviewer".into(),
                },
                json!({"scope": "role", "role": "reviewer"}),
            ),
            (
                OperatorMemoryScope::RepositoryRole {
                    repository: None,
                    root: None,
                    role: "reviewer".into(),
                },
                json!({"scope": "repository_role", "role": "reviewer"}),
            ),
            (
                OperatorMemoryScope::RepositoryRole {
                    repository: None,
                    root: Some("/workspace".into()),
                    role: "reviewer.security".into(),
                },
                json!({"scope": "repository_role", "root": "/workspace", "role": "reviewer.security"}),
            ),
        ];
        for (scope, mut wire) in cases {
            assert_eq!(serde_json::to_value(&scope).expect("serialize"), wire);
            assert_eq!(
                serde_json::from_value::<OperatorMemoryScope>(wire.clone()).expect("deserialize"),
                scope
            );
            wire["id"] = json!("fabricated-repository");
            assert!(serde_json::from_value::<OperatorMemoryScope>(wire).is_err());
        }
        assert_eq!(
            serde_json::from_value::<OperatorMemoryScope>(json!({
                "scope": "repository_role", "root": null, "role": "reviewer"
            }))
            .expect("null root"),
            OperatorMemoryScope::RepositoryRole {
                repository: None,
                root: None,
                role: "reviewer".into()
            }
        );
        assert!(serde_json::from_value::<OperatorMemoryScope>(json!({"scope": "role"})).is_err());
    }

    #[test]
    fn explicit_operator_repositories_round_trip() {
        for wire in [
            json!({"scope": "repository", "repository": "aether"}),
            json!({"scope": "repository_role", "repository": "aether", "role": "archon-stage:default/review"}),
        ] {
            let scope: OperatorMemoryScope = serde_json::from_value(wire.clone()).unwrap();
            assert_eq!(serde_json::to_value(&scope).unwrap(), wire);
            assert!(serde_json::to_value(scope).unwrap().get("root").is_none());
        }
    }

    /// The store filters on the serialized actor's tag, so its shape is
    /// pinned here beside the predicate it must agree with.
    #[test]
    fn only_a_local_operator_is_recorded_by_the_operator() {
        use crate::event::Actor;
        assert_eq!(
            serde_json::to_value(MemoryRecorder::Operator).unwrap(),
            json!("operator")
        );
        assert!(serde_json::from_value::<MemoryRecorder>(json!("agent")).is_err());
        assert_eq!(
            serde_json::to_value(Actor::user()).unwrap(),
            json!({"actor": "user"})
        );
        assert_eq!(
            serde_json::to_value(Actor::user().imported()).unwrap(),
            json!({"actor": "imported", "was": {"actor": "user"}})
        );
        let cases = [
            (Some(Actor::user()), None, true),
            (Some(Actor::user().imported()), None, false),
            (Some(Actor::agent(AgentId::new())), None, false),
            (None, Some(AgentId::new()), false),
            (Some(Actor::System), None, false),
            (None, None, false),
        ];
        for (actor, agent, expected) in cases {
            let provenance = MemoryProvenance {
                actor,
                agent,
                ..Default::default()
            };
            assert_eq!(MemoryRecorder::Operator.recorded(&provenance), expected);
        }
    }

    #[test]
    fn archon_role_is_a_reachable_family_without_being_a_directory_name() {
        let role = "archon-stage:default/review";
        assert_eq!(role_families(role), vec![role]);
        assert!(!is_role_family(role));
    }

    #[test]
    fn global_visible_everywhere_others_exact() {
        let repo = MemoryScope::Repository { id: "r".into() };
        assert!(MemoryScope::Global.visible_to(&repo));
        assert!(repo.visible_to(&repo));
        assert!(!repo.visible_to(&MemoryScope::Repository { id: "other".into() }));
        assert!(!repo.visible_to(&MemoryScope::Global));
    }

    /// Stored rows carry these exact shapes; the scopes added later must not
    /// change how any earlier one reads or writes.
    #[test]
    fn scopes_round_trip_and_earlier_shapes_still_read() {
        let agent: AgentId = "agt_01JB0000000000000000000000".parse().expect("agent id");
        let cases = [
            (MemoryScope::Global, json!({"scope": "global"})),
            (
                MemoryScope::Repository { id: "r".into() },
                json!({"scope": "repository", "id": "r"}),
            ),
            (
                MemoryScope::Role {
                    role: "worker".into(),
                },
                json!({"scope": "role", "role": "worker"}),
            ),
            (
                MemoryScope::Agent { id: agent },
                json!({"scope": "agent", "id": agent.to_string()}),
            ),
            (
                MemoryScope::RepositoryRole {
                    id: "r".into(),
                    role: "team.steward".into(),
                },
                json!({"scope": "repository_role", "id": "r", "role": "team.steward"}),
            ),
            (
                MemoryScope::Subtree { root: agent },
                json!({"scope": "subtree", "root": agent.to_string()}),
            ),
        ];
        for (scope, wire) in cases {
            assert_eq!(serde_json::to_value(&scope).expect("serialize"), wire);
            let read: MemoryScope = serde_json::from_value(wire).expect("deserialize");
            assert_eq!(read, scope);
        }
    }

    #[test]
    fn a_role_belongs_to_each_whole_segment_prefix_most_specific_first() {
        assert_eq!(role_families("team.steward"), vec!["team.steward", "team"]);
        assert_eq!(role_families("worker"), vec!["worker"]);
        assert!(role_families("").is_empty());
        // Past the segment cap no directory can match, so nothing is asked.
        let deep = (0..12)
            .map(|i| format!("s{i}"))
            .collect::<Vec<_>>()
            .join(".");
        let families = role_families(&deep);
        assert_eq!(families.len(), MAX_ROLE_SEGMENTS);
        assert!(families.iter().all(|f| is_role_family(f)), "{families:?}");
        assert!(!families.contains(&deep.as_str()));
    }

    #[test]
    fn a_role_family_name_is_dotted_plain_segments() {
        for good in ["team", "team.steward", "a-b_c.d9", "A.B"] {
            assert!(is_role_family(good), "{good}");
        }
        let too_deep = ["s"; MAX_ROLE_SEGMENTS + 1].join(".");
        for bad in [
            "",
            ".hidden",
            "trailing.",
            "a..b",
            "has space",
            "slash/y",
            "émoji",
            "a@b",
            too_deep.as_str(),
        ] {
            assert!(!is_role_family(bad), "{bad}");
        }
    }
}

#[cfg(test)]
mod proposal_tests {
    use super::*;

    #[test]
    fn proposal_state_is_preserved_in_records_and_excluded_from_current_memories() {
        let wire = serde_json::json!({"id":"mem_01ARZ3NDEKTSV4RRFFQ69G5FAV", "kind":"lesson",
            "scope":{"scope":"global"}, "text":"A durable build lesson", "importance":0.7,
            "confidence":0.9, "provenance":{}, "created_at":"2026-10-15T00:00:00Z"});
        let mut record: MemoryRecord = serde_json::from_value(wire).unwrap();
        assert!(record.is_current());
        assert!(!record.is_proposed());
        record.proposed_at = Some(record.created_at);
        assert!(record.is_proposed());
        assert!(!record.is_current());
        assert_eq!(
            serde_json::from_str::<MemoryRecord>(&serde_json::to_string(&record).unwrap()).unwrap(),
            record
        );
        record.forgotten_at = Some(record.created_at);
        assert!(record.is_proposed());
        assert!(!record.is_current());
        record.forgotten_at = None;
        record.accepted_at = Some(record.created_at);
        assert!(!record.is_proposed());
        assert!(record.is_current());
    }
}
