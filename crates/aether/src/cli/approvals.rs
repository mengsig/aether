use super::{
    ApprovalDecision, NewApprovalRule, RememberSpec, RuleMatch, RuleScope, RuleScopeArgs, acted,
    connect, short,
};
use anyhow::{Context as _, anyhow, bail};

pub async fn approve(
    socket: impl Into<aether_client::Endpoint>,
    approval: super::ApprovalRef,
    approve: bool,
    remember: bool,
    scope: RuleScopeArgs,
    tool_wide: bool,
    json: bool,
) -> anyhow::Result<()> {
    let socket = socket.into();
    let client = connect(&socket).await?;
    let id = approval.resolve(&client).await?;
    let scope = scope.resolve(&client, &socket).await?;
    let decision = decision_of(approve);
    client
        .resolve_approval_remembering(
            id,
            decision,
            None,
            // No scope means the answered agent: the daemon fills it in, so
            // the CLI and the TUI mean the same thing by a bare --remember.
            remember.then_some(RememberSpec {
                scope,
                tool_wide,
                note: None,
            }),
        )
        .await?;
    let decided = if approve { "approved" } else { "denied" };
    acted(
        json,
        &format!("{decided} {id}"),
        serde_json::json!({"approval": id, "decision": decision, "remembered": remember}),
    )
}

fn decision_of(approve: bool) -> ApprovalDecision {
    if approve {
        ApprovalDecision::Approved
    } else {
        ApprovalDecision::Denied
    }
}

pub async fn rules_list(
    socket: impl Into<aether_client::Endpoint>,
    all: bool,
    json: bool,
) -> anyhow::Result<()> {
    let socket = socket.into();
    let client = connect(&socket).await?;
    let rules = client.list_approval_rules(all).await?;
    if json {
        println!("{}", serde_json::to_string(&rules)?);
        return Ok(());
    }
    println!(
        "{:<32} {:<8} {:<16} {:<30} {:<34} note",
        "id", "answer", "tool", "matches", "where"
    );
    for r in rules {
        println!(
            "{:<32} {:<8} {:<16} {:<30} {:<34} {}{}",
            r.id.to_string(),
            if r.decision == ApprovalDecision::Approved {
                "allow"
            } else {
                "refuse"
            },
            super::sane(r.matcher.tool.as_deref().unwrap_or("any")),
            short(&describe_match(&r.matcher), 30),
            short(&describe_scope(&r.scope), 34),
            super::sane(r.note.as_deref().unwrap_or("")),
            if r.revoked.is_some() {
                " (revoked)"
            } else {
                ""
            },
        );
    }
    Ok(())
}

fn describe_match(matcher: &RuleMatch) -> String {
    match (&matcher.argument, &matcher.resource_prefix) {
        (Some(a), Some(r)) => format!("{a} · {r}"),
        (Some(a), None) => a.clone(),
        (None, Some(r)) => r.clone(),
        (None, None) => "any call".into(),
    }
}

fn describe_scope(scope: &RuleScope) -> String {
    match scope {
        RuleScope::Everywhere => "everywhere".into(),
        RuleScope::Agent { id } => format!("agent {id}"),
        RuleScope::Subtree { root } => format!("{root} and its workers"),
        RuleScope::Run { id } => format!("run {id}"),
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn rules_add(
    socket: impl Into<aether_client::Endpoint>,
    allow: bool,
    tool: Option<String>,
    argument: Option<String>,
    resource_prefix: Option<String>,
    scope: RuleScopeArgs,
    note: Option<String>,
    json: bool,
) -> anyhow::Result<()> {
    let socket = socket.into();
    let client = connect(&socket).await?;
    let matcher = RuleMatch {
        tool,
        argument,
        resource_prefix,
    };
    matcher.validate().map_err(|e| anyhow!(e))?;
    // A hand-written rule outlives the session that wrote it, so it never
    // widens to everywhere by omission.
    let Some(scope) = scope.resolve(&client, &socket).await? else {
        bail!(
            "say where this rule applies: --agent, --subtree, --run, or --everywhere for every \
             agent this daemon will ever run"
        );
    };
    if matcher == RuleMatch::default() && scope == RuleScope::Everywhere {
        bail!("a rule matching every call everywhere is refused; narrow it with --tool or a scope");
    }
    let rule = client
        .add_approval_rule(NewApprovalRule {
            decision: decision_of(allow),
            matcher,
            scope,
            note,
            idempotency_key: None,
        })
        .await?;
    if json {
        println!("{}", serde_json::to_string(&rule)?);
    } else {
        println!("{}", rule.id);
    }
    Ok(())
}

pub async fn rules_revoke(
    socket: impl Into<aether_client::Endpoint>,
    id: String,
    reason: Option<String>,
    json: bool,
) -> anyhow::Result<()> {
    let socket = socket.into();
    let client = connect(&socket).await?;
    let id = id.parse().context("invalid rule id")?;
    let rule = client.revoke_approval_rule(id, reason).await?;
    if json {
        println!("{}", serde_json::to_string(&rule)?);
    } else {
        println!("revoked {id}");
    }
    Ok(())
}
