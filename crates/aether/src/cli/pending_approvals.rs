use std::collections::{HashMap, HashSet};
use std::io::{self, Write};

use aether_client::{Client, ClientError};
use aether_core::agent::Agent;
use aether_core::approval::RuleScope;
use aether_core::id::{AgentId, ApprovalId};
use aether_core::tool::{ApprovalDecision, ApprovalRequest, ApprovalStatus};
use aether_protocol::{AgentQuery, ErrorCode};
use anyhow::{Context as _, bail, ensure};

pub async fn pending_approvals(
    endpoint: impl Into<aether_client::Endpoint>,
    scope: super::RuleScopeArgs,
    json: bool,
) -> anyhow::Result<()> {
    let endpoint = endpoint.into();
    let client = super::connect(&endpoint).await?;
    let scope = scope.resolve(&client, &endpoint).await?;
    let pending = snapshot(&client, scope.as_ref()).await?;
    if json {
        println!("{}", serde_json::to_string(&pending)?);
        return Ok(());
    }
    println!(
        "{:<30} {:<30} {:<20} {:<40} {:<20} age",
        "id", "agent", "tool", "arguments", "side effect"
    );
    let now = aether_core::time::now();
    for request in pending {
        println!(
            "{:<30} {:<30} {:<20} {:<40} {:<20} {}s",
            request.id,
            request.agent_id,
            super::short(&request.tool, 20),
            super::short(
                request.argument.as_deref().unwrap_or(&request.description),
                40
            ),
            serde_json::to_value(request.side_effect)?
                .as_str()
                .context("invalid side effect")?,
            (now - request.requested_at).num_seconds().max(0),
        );
    }
    Ok(())
}

async fn snapshot(
    client: &Client,
    scope: Option<&RuleScope>,
) -> anyhow::Result<Vec<ApprovalRequest>> {
    let agents = match scope {
        None => None,
        Some(RuleScope::Agent { id }) => Some(HashSet::from([*id])),
        Some(RuleScope::Everywhere) => {
            bail!("pending approvals require --agent, --subtree, or --run")
        }
        Some(scope) => {
            let agents = client
                .list_agents(AgentQuery {
                    include_archived: true,
                    ..Default::default()
                })
                .await?;
            Some(scoped_agents(&agents, scope)?)
        }
    };
    let mut requests = client.list_approvals().await?;
    ensure!(
        requests.iter().all(|r| r.status == ApprovalStatus::Pending),
        "daemon returned a non-pending approval"
    );
    let unique: HashSet<_> = requests.iter().map(|r| r.id).collect();
    ensure!(
        unique.len() == requests.len(),
        "daemon returned duplicate approvals"
    );
    requests.retain(|r| agents.as_ref().is_none_or(|ids| ids.contains(&r.agent_id)));
    requests.sort_by_key(|r| (r.requested_at, r.id));
    Ok(requests)
}

fn scoped_agents(agents: &[Agent], scope: &RuleScope) -> anyhow::Result<HashSet<AgentId>> {
    if let RuleScope::Run { id } = scope {
        return Ok(agents
            .iter()
            .filter(|a| a.run == *id)
            .map(|a| a.id)
            .collect());
    }
    let RuleScope::Subtree { root } = scope else {
        bail!("expected a run or subtree scope");
    };
    ensure!(
        agents.iter().any(|a| a.id == *root),
        "subtree root {root} was not found"
    );
    let mut children: HashMap<AgentId, Vec<AgentId>> = HashMap::new();
    for agent in agents {
        if let Some(parent) = agent.parent {
            children.entry(parent).or_default().push(agent.id);
        }
    }
    let mut selected = HashSet::new();
    let mut queue = vec![*root];
    while let Some(id) = queue.pop() {
        ensure!(
            selected.insert(id),
            "daemon returned a cycle or duplicate agent in subtree {root}"
        );
        queue.extend(children.get(&id).into_iter().flatten().copied());
    }
    Ok(selected)
}

pub async fn answer_pending(
    endpoint: impl Into<aether_client::Endpoint>,
    scope: super::RuleScopeArgs,
    approve: bool,
    yes: bool,
    json: bool,
) -> anyhow::Result<()> {
    let endpoint = endpoint.into();
    let client = super::connect(&endpoint).await?;
    let scope = scope.resolve(&client, &endpoint).await?;
    let scope = scope.context("--all-pending requires --agent, --subtree, or --run")?;
    ensure!(
        scope != RuleScope::Everywhere,
        "--all-pending requires --agent, --subtree, or --run"
    );
    let pending = snapshot(&client, Some(&scope)).await?;
    let decision = if approve {
        ApprovalDecision::Approved
    } else {
        ApprovalDecision::Denied
    };
    if !pending.is_empty() && !yes && !confirm(pending.len(), approve)? {
        bail!("cancelled; no approvals answered");
    }
    let (answered, already_resolved) = settle(&client, &pending, decision, json).await?;
    super::acted(
        json,
        &format!(
            "{} {answered} pending approvals; {} already resolved",
            decision.as_str(),
            already_resolved.len()
        ),
        serde_json::json!({"decision": decision, "selected": pending.len(), "answered": answered, "already_resolved": already_resolved}),
    )
}

async fn settle(
    client: &Client,
    pending: &[ApprovalRequest],
    decision: ApprovalDecision,
    json: bool,
) -> anyhow::Result<(usize, Vec<ApprovalId>)> {
    let mut answered = 0;
    let mut already_resolved = Vec::new();
    for request in pending {
        match client.resolve_approval(request.id, decision, None).await {
            Ok(()) => answered += 1,
            Err(error) if is_already_resolved(&error, request.id) => {
                if !json {
                    println!("{} already resolved", request.id);
                }
                already_resolved.push(request.id);
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "answering {}; {answered} answered before failure",
                        request.id
                    )
                });
            }
        }
    }
    assert_eq!(answered + already_resolved.len(), pending.len());
    Ok((answered, already_resolved))
}

fn confirm(count: usize, approve: bool) -> anyhow::Result<bool> {
    assert!(count > 0);
    eprint!(
        "{} {count} pending approvals? [y/N] ",
        if approve { "Approve" } else { "Deny" }
    );
    io::stderr().flush().context("write confirmation prompt")?;
    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .context("read confirmation; use --yes for unattended batches")?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

pub(super) fn is_already_resolved(error: &ClientError, id: ApprovalId) -> bool {
    matches!(error, ClientError::Protocol(reply)
        if reply.code == ErrorCode::InvalidArgument
        && reply.message == format!("approval {id} already resolved"))
}
