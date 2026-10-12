//! The protocol as a JSON Schema.
//!
//! The Rust types are the single source the client libraries are generated
//! from, and this is how they leave the crate: one document holding every
//! type reachable from the two frames, with the frames named. `aether schema`
//! prints it and `scripts/generate_clients.py` turns it into the Python and
//! TypeScript libraries, so a type that changes here changes them.

use serde_json::{Value, json};

use crate::{ClientFrame, PROTOCOL_VERSION, ServerFrame, compat::MIN_PROTOCOL_VERSION};

/// The whole protocol: `$defs` holds every type, `client_frame` and
/// `server_frame` name the two roots.
pub fn schema() -> Value {
    let mut generator = schemars::generate::SchemaSettings::draft2020_12().into_generator();
    let client = generator.subschema_for::<ClientFrame>();
    let server = generator.subschema_for::<ServerFrame>();
    // Bodies that never ride a frame: the HTTP endpoints and the push payload.
    generator.subschema_for::<crate::remote::PairClaimRequest>();
    generator.subschema_for::<crate::remote::PairClaimResponse>();
    generator.subschema_for::<crate::remote::PairStatusRequest>();
    generator.subschema_for::<crate::remote::PairStatusResponse>();
    generator.subschema_for::<crate::remote::Health>();
    generator.subschema_for::<crate::remote::HttpError>();
    generator.subschema_for::<crate::remote::PushPayload>();
    let defs = generator.take_definitions(true);
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "Aether daemon protocol",
        "protocol_version": PROTOCOL_VERSION,
        "min_protocol_version": MIN_PROTOCOL_VERSION,
        "client_frame": client,
        "server_frame": server,
        "$defs": defs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The document a generator reads: both roots resolve, and the types a
    /// client cannot work without are all in it.
    #[test]
    fn the_schema_carries_both_frames_and_the_types_under_them() {
        let schema = schema();
        assert_eq!(schema["protocol_version"], PROTOCOL_VERSION);
        for root in ["client_frame", "server_frame"] {
            let reference = schema[root]["$ref"]
                .as_str()
                .unwrap_or_else(|| panic!("{root} refers to a definition: {}", schema[root]));
            let name = reference
                .strip_prefix("#/$defs/")
                .unwrap_or_else(|| panic!("a local definition: {reference}"));
            assert!(
                schema["$defs"].get(name).is_some(),
                "{root} refers to {name}, which is not defined"
            );
        }
        for named in ["Reply", "Hello", "Welcome", "ErrorReply", "Agent", "Event"] {
            assert!(
                schema["$defs"].get(named).is_some(),
                "{named} is missing from the schema"
            );
        }
    }

    /// Commands are flattened into the frame, so what a generator sees is one
    /// `oneOf` discriminated by `cmd` - the shape that actually goes on the
    /// wire, not a nested `command` object.
    #[test]
    fn a_client_frame_is_one_variant_per_command_tagged_by_cmd() {
        let schema = schema();
        let variants = schema["$defs"]["ClientFrame"]["oneOf"]
            .as_array()
            .expect("a discriminated union")
            .clone();
        let tags: Vec<&str> = variants
            .iter()
            .filter_map(|variant| variant["properties"]["cmd"]["const"].as_str())
            .collect();
        assert_eq!(tags.len(), variants.len(), "every variant names its `cmd`");
        for expected in [
            "hello",
            "ping",
            "create_agent",
            "subscribe",
            "reply_to_question",
        ] {
            assert!(
                tags.contains(&expected),
                "`{expected}` is missing: {tags:?}"
            );
        }
    }

    /// A typed id is a string with its prefix, not an opaque object: a
    /// generated client has to be able to hold one.
    #[test]
    fn a_typed_id_is_a_prefixed_string() {
        let schema = schema();
        let agent = &schema["$defs"]["AgentId"];
        assert_eq!(agent["type"], "string");
        assert_eq!(
            agent["pattern"].as_str().unwrap_or_default(),
            "^agt_[0-9A-HJKMNP-TV-Z]{26}$"
        );
    }
}
