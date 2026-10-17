use super::*;

#[derive(Debug)]
pub struct RefreshProviderUsage;

#[async_trait]
impl Tool for RefreshProviderUsage {
    fn descriptor(&self) -> ToolDescriptor {
        crate::builtin::descriptor(
            "refresh_provider_usage",
            "Refresh stale quota headroom for one or all enabled providers. Only usage_probe=turn providers may spend a turn; fresh, in-flight and backed-off ones are skipped.",
            json!({
                "type":"object",
                "properties":{"provider":{"type":"string", "minLength":1}},
                "additionalProperties":false
            }),
            SideEffectClass::External,
            true,
        )
    }

    fn resources(&self, _input: &Value, _ctx: &ToolContext) -> Vec<ResourceKey> {
        vec![ResourceKey::network()]
    }

    async fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
        _cancel: CancellationToken,
    ) -> Result<ToolExecution, ToolError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Input {
            provider: Option<String>,
        }
        let Input { provider } = parse_input(input)?;
        if provider.as_ref().is_some_and(|id| id.trim().is_empty()) {
            return Err(ToolError::InvalidInput("provider must not be empty".into()));
        }
        Ok(ToolExecution::Action(HarnessAction::RefreshProviderUsage {
            provider,
        }))
    }
}
