use super::*;
use aether_core::tool::ToolOutcome;

/// How a lead and its workers build one change together. Only a lead reads
/// it; workers get the Worker rules in their objectives.
pub const SWARM_PLAYBOOK: &str = include_str!("swarm_playbook.md");

#[derive(Debug)]
pub struct SwarmPlaybook;

#[async_trait]
impl Tool for SwarmPlaybook {
    fn descriptor(&self) -> ToolDescriptor {
        harness_descriptor(
            "swarm_playbook",
            "How to split one change across parallel leaf workers.",
            json!({"type":"object", "properties":{}, "additionalProperties":false}),
        )
    }

    fn resources(&self, _input: &Value, _ctx: &ToolContext) -> Vec<ResourceKey> {
        Vec::new()
    }

    async fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
        _cancel: CancellationToken,
    ) -> Result<ToolExecution, ToolError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Input {}
        let _: Input = parse_input(input)?;
        Ok(ToolExecution::Outcome(ToolOutcome::ok(SWARM_PLAYBOOK)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::ctx_in;

    #[tokio::test]
    async fn the_playbook_names_the_tools_a_swarm_uses() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_in(dir.path());
        let ToolExecution::Outcome(outcome) = SwarmPlaybook
            .execute(json!({}), &ctx, CancellationToken::new())
            .await
            .unwrap()
        else {
            panic!("the playbook is answered in place");
        };
        assert!(!outcome.is_error);
        for tool in [
            "delegate",
            "allowed_paths",
            "integrate_workspace",
            "list_peers",
            "send_message",
            "wait_for(message_from",
        ] {
            assert!(outcome.content.contains(tool), "{tool}");
        }
        let err = SwarmPlaybook
            .execute(json!({"bogus": 1}), &ctx, CancellationToken::new())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("bogus"), "{err}");
    }
}
