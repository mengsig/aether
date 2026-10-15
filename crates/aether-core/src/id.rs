//! Typed, time-ordered identifiers.
//!
//! Every durable entity gets its own id type so an agent id can never be passed
//! where a task id is expected. Ids render as `prefix_ULID`; the prefix makes
//! them self-describing in logs, URLs and filenames, and the ULID keeps them
//! sortable by creation time.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use ulid::Ulid;

/// Error returned when a string is not a well-formed typed id.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IdParseError {
    #[error("expected id with prefix `{expected}_`, got `{input}`")]
    WrongPrefix {
        expected: &'static str,
        input: String,
    },
    #[error("invalid ulid in `{input}`: {reason}")]
    InvalidUlid { input: String, reason: String },
}

/// The prefix of a `prefix_ULID` typed id, or `None` for anything else.
/// Whole-string only: an id inside a longer text is text, not a reference.
pub fn prefix_of(text: &str) -> Option<&str> {
    let (prefix, ulid) = text.split_once('_')?;
    if prefix.is_empty() || !prefix.bytes().all(|b| b.is_ascii_lowercase()) {
        return None;
    }
    if ulid.len() != 26 || Ulid::from_string(ulid).is_err() {
        return None;
    }
    Some(prefix)
}

macro_rules! typed_id {
    ($(#[$doc:meta])* $name:ident, $prefix:literal) => {
        $(#[$doc])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(Ulid);

        impl $name {
            /// Prefix used when rendering this id.
            pub const PREFIX: &'static str = $prefix;

            /// Mint a fresh id from the current time and a random tail.
            pub fn new() -> Self {
                Self(Ulid::generate())
            }

            /// Wrap an existing ULID (used by stores and tests).
            pub const fn from_ulid(ulid: Ulid) -> Self {
                Self(ulid)
            }

            /// The underlying ULID.
            pub const fn ulid(&self) -> Ulid {
                self.0
            }

            /// Millisecond timestamp encoded in the id.
            pub fn timestamp_ms(&self) -> u64 {
                self.0.timestamp_ms()
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}_{}", Self::PREFIX, self.0)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(self, f)
            }
        }

        impl FromStr for $name {
            type Err = IdParseError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                let Some(rest) = s.strip_prefix(concat!($prefix, "_")) else {
                    return Err(IdParseError::WrongPrefix { expected: $prefix, input: s.to_owned() });
                };
                Ulid::from_string(rest).map(Self).map_err(|e| IdParseError::InvalidUlid {
                    input: s.to_owned(),
                    reason: e.to_string(),
                })
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.collect_str(self)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let s = String::deserialize(deserializer)?;
                s.parse().map_err(serde::de::Error::custom)
            }
        }

        // The id is a string on the wire, so that is what a generated client
        // sees: the pattern is the whole shape, prefix included.
        impl schemars::JsonSchema for $name {
            fn schema_name() -> std::borrow::Cow<'static, str> {
                stringify!($name).into()
            }

            fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
                schemars::json_schema!({
                    "type": "string",
                    "pattern": concat!("^", $prefix, "_[0-9A-HJKMNP-TV-Z]{26}$"),
                    "description": concat!("A ", $prefix, "_ prefixed ULID."),
                })
            }
        }
    };
}

typed_id!(
    /// Identifies a durable recurring trigger.
    ScheduleId,
    "sch"
);
typed_id!(
    /// Identifies an agent for its whole lifetime.
    AgentId,
    "agt"
);
typed_id!(
    /// Identifies a unit of work owned by an agent.
    TaskId,
    "tsk"
);
typed_id!(
    /// Identifies one entry in the event log.
    EventId,
    "evt"
);
typed_id!(
    /// Identifies one model invocation (a single request/response).
    InvocationId,
    "inv"
);
typed_id!(
    /// Identifies an immutable context snapshot fed to a model turn.
    SnapshotId,
    "snp"
);
typed_id!(
    /// Identifies a structured compaction checkpoint.
    CheckpointId,
    "chk"
);
typed_id!(
    /// Identifies a durable memory record.
    MemoryId,
    "mem"
);
typed_id!(
    /// Identifies one operator feedback record on a memory.
    MemoryFeedbackId,
    "mfb"
);
typed_id!(
    /// Identifies a stored artifact (file, output, diff, commit).
    ArtifactId,
    "art"
);
typed_id!(
    /// Correlates everything that happened for one top-level run.
    RunId,
    "run"
);
typed_id!(
    /// Identifies a tool call as tracked by the runtime.
    ToolCallId,
    "call"
);
typed_id!(
    /// Identifies an approval request.
    ApprovalId,
    "apr"
);
typed_id!(
    /// Identifies a remembered approval decision.
    ApprovalRuleId,
    "rule"
);
typed_id!(
    /// Identifies a background process started by the shell tool.
    ProcessId,
    "proc"
);
typed_id!(
    /// Identifies one item in an agent inbox.
    InboxItemId,
    "ibx"
);
typed_id!(
    /// Identifies a message sent between actors.
    MessageId,
    "msg"
);
typed_id!(
    /// Identifies a paired remote device for as long as it stays enrolled.
    DeviceId,
    "dev"
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips_through_display_and_parse() {
        let id = AgentId::new();
        let text = id.to_string();
        assert!(text.starts_with("agt_"));
        let parsed: AgentId = text.parse().expect("parse");
        assert_eq!(id, parsed);
    }

    #[test]
    fn rejects_wrong_prefix() {
        let id = TaskId::new().to_string();
        let err = id.parse::<AgentId>().unwrap_err();
        assert!(matches!(
            err,
            IdParseError::WrongPrefix {
                expected: "agt",
                ..
            }
        ));
    }

    #[test]
    fn prefix_detection_requires_a_parseable_ulid() {
        let valid = AgentId::new().to_string();
        assert_eq!(prefix_of(&valid), Some("agt"));
        assert_eq!(prefix_of(&valid.to_ascii_lowercase()), Some("agt"));
        for invalid in [
            "agt_IIIIIIIIIIIIIIIIIIIIIIIIII",
            "agt_LLLLLLLLLLLLLLLLLLLLLLLLLL",
            "agt_OOOOOOOOOOOOOOOOOOOOOOOOOO",
            "agt_UUUUUUUUUUUUUUUUUUUUUUUUUU",
        ] {
            assert!(invalid.parse::<AgentId>().is_err(), "{invalid}");
            assert_eq!(prefix_of(invalid), None, "{invalid}");
        }
    }

    #[test]
    fn serde_uses_string_form() {
        let id = EventId::new();
        let json = serde_json::to_string(&id).expect("serialize");
        assert_eq!(json, format!("\"{id}\""));
        let back: EventId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(id, back);
    }

    #[test]
    fn ids_are_time_ordered() {
        let a = RunId::new();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let b = RunId::new();
        assert!(a < b);
    }
}
