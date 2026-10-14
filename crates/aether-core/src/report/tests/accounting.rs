use super::*;

#[test]
fn billing_corrections_replace_provisional_cost_in_subtree_reports() {
    let mut log = Log::new();
    let (root, child) = (AgentId::new(), AgentId::new());
    log.push(created(root, None, "root"));
    log.push(created(child, Some(root), "child"));
    log.push(spent(root, 1.0));
    for cost in [2.0, 1.5] {
        log.push(
            EventDraft::new(
                Actor::agent(child),
                EventKind::AgentUsageUpdated {
                    total: Usage {
                        cost_usd: cost,
                        output_tokens: 105,
                        ..Default::default()
                    },
                    turn_usage: Usage {
                        cost_usd: cost,
                        output_tokens: 105,
                        ..Default::default()
                    },
                },
            )
            .agent(child),
        );
    }
    let report = log.report();
    assert_eq!(report.body.usage.cost_usd, 2.5);
    assert_eq!(
        report
            .body
            .agents
            .iter()
            .find(|a| a.id == child)
            .unwrap()
            .usage
            .output_tokens,
        105
    );
    assert_eq!(
        report
            .body
            .agents
            .iter()
            .find(|a| a.id == root)
            .unwrap()
            .subtree_usage
            .cost_usd,
        2.5
    );
}

#[test]
fn cost_rolls_up_the_tree_the_log_describes() {
    let mut log = Log::new();
    let (root, child) = (AgentId::new(), AgentId::new());
    log.push(created(root, None, "root"));
    log.push(created(child, Some(root), "child"));
    log.push(spent(root, 1.0));
    log.push(spent(child, 2.0));
    let report = log.report();
    let root_line = report
        .body
        .agents
        .iter()
        .find(|a| a.id == root)
        .expect("root");
    let child_line = report
        .body
        .agents
        .iter()
        .find(|a| a.id == child)
        .expect("child");
    assert_eq!(root_line.usage.cost_usd, 1.0);
    assert_eq!(root_line.subtree_usage.cost_usd, 3.0);
    assert!(root_line.subtree_usage.cost_complete);
    assert_eq!(child_line.subtree_usage.cost_usd, 2.0);
    assert_eq!(child_line.depth, 1);
    assert_eq!(report.body.usage.cost_usd, 3.0);
}

#[test]
fn one_unpriced_invocation_makes_every_roll_up_above_it_a_floor() {
    let mut log = Log::new();
    let (root, child) = (AgentId::new(), AgentId::new());
    log.push(created(root, None, "root"));
    log.push(created(child, Some(root), "child"));
    log.push(spent(root, 1.0));
    log.push(unpriced(child));
    let report = log.report();
    let line = |id: AgentId| {
        report
            .body
            .agents
            .iter()
            .find(|a| a.id == id)
            .expect("agent")
            .clone()
    };
    assert_eq!(line(root).usage.cost_usd, 1.0, "the root did report");
    assert!(
        !line(child).usage.cost_complete,
        "the child reported nothing, and zero is not nothing"
    );
    assert_eq!(line(child).usage.cost_line(), Usage::COST_UNKNOWN);
    let subtree = line(root).subtree_usage;
    assert_eq!(subtree.cost_usd, 1.0, "the priced part still counts");
    assert!(
        !subtree.cost_complete,
        "part of the root's tree is unmeasured"
    );
    assert_eq!(subtree.cost_line(), "≥ $1.0000 (some unpriced)");
    assert_eq!(report.body.usage.cost_usd, 1.0);
    assert!(
        !report.body.usage.cost_complete,
        "and so is part of the run"
    );
}

/// The roll-up is folded child-into-parent in one pass, so a chain is
/// linear rather than an ancestor walk per agent. Correctness of the
/// fold at depth is here; what the old shape cost at scale is measured
/// in `aether-runtime`'s `rollup_bench`.
#[test]
fn a_deep_chain_rolls_up_through_every_link() {
    const DEPTH: usize = 300;
    let mut log = Log::new();
    let ids: Vec<AgentId> = (0..DEPTH).map(|_| AgentId::new()).collect();
    for (i, id) in ids.iter().enumerate() {
        log.push(created(*id, i.checked_sub(1).map(|p| ids[p]), "link"));
        log.push(spent(*id, 0.5));
    }
    let report = log.report();
    let line = |id: AgentId| {
        report
            .body
            .agents
            .iter()
            .find(|a| a.id == id)
            .expect("agent")
            .clone()
    };
    assert_eq!(line(ids[DEPTH - 1]).depth, (DEPTH - 1) as u32);
    assert_eq!(line(ids[0]).depth, 0);
    assert_eq!(line(ids[DEPTH - 1]).subtree_usage.cost_usd, 0.5, "the leaf");
    assert_eq!(
        line(ids[0]).subtree_usage.cost_usd,
        0.5 * DEPTH as f64,
        "the root carries the whole chain"
    );
    assert_eq!(
        line(ids[DEPTH / 2]).subtree_usage.cost_usd,
        0.5 * (DEPTH - DEPTH / 2) as f64
    );
}

/// A parent link that points back into its own path is data from a file,
/// not a tree. It must not spin, and it must not lose the honest totals
/// around it.
#[test]
fn a_cycle_in_the_stored_links_still_returns() {
    let mut log = Log::new();
    let (a, b) = (AgentId::new(), AgentId::new());
    log.push(created(a, Some(b), "a"));
    log.push(created(b, Some(a), "b"));
    log.push(spent(a, 1.0));
    log.push(spent(b, 2.0));
    let report = log.report();
    assert_eq!(report.body.agents.len(), 2);
    for agent in &report.body.agents {
        assert_eq!(
            agent.subtree_usage.cost_usd, agent.usage.cost_usd,
            "a cycle folds nothing into itself"
        );
    }
}

#[test]
fn depths_preserve_missing_parent_and_cycle_boundary_behavior() {
    let ids: Vec<_> = (0..6).map(|_| AgentId::new()).collect();
    let mut log = Log::new();
    log.push(created(ids[0], Some(AgentId::new()), "missing parent"));
    log.push(created(ids[1], Some(ids[0]), "child"));
    log.push(created(ids[2], Some(ids[3]), "cycle a"));
    log.push(created(ids[3], Some(ids[2]), "cycle b"));
    log.push(created(ids[4], Some(ids[3]), "below cycle"));
    log.push(created(ids[5], Some(ids[5]), "self cycle"));
    let report = log.report();
    let depths: BTreeMap<_, _> = report.body.agents.iter().map(|a| (a.id, a.depth)).collect();
    assert_eq!(depths[&ids[0]], 1);
    assert_eq!(depths[&ids[1]], 2);
    for id in &ids[2..] {
        assert_eq!(depths[id], ids.len() as u32);
    }
}

#[test]
fn cost_gap_reasons_survive_report_rollups_and_rendering() {
    use crate::model::CostGap;
    let mut log = Log::new();
    let (root, child) = (AgentId::new(), AgentId::new());
    log.push(created(root, None, "root"));
    log.push(created(child, Some(root), "child"));
    for (agent, gap, dollars) in [
        (root, CostGap::Unpriced, 1.0),
        (child, CostGap::Interrupted, 2.0),
    ] {
        let mut usage = Usage {
            cost_usd: dollars,
            input_tokens: 10,
            ..Default::default()
        };
        usage.mark_cost_gap(gap);
        log.push(
            EventDraft::new(
                Actor::agent(agent),
                EventKind::AgentUsageUpdated {
                    total: usage,
                    turn_usage: usage,
                },
            )
            .agent(agent),
        );
    }
    let report = log.report();
    assert_eq!(report.body.usage.cost_gap, Some(CostGap::Mixed));
    assert_eq!(report.body.usage.cost_usd, 3.0);
    assert_eq!(report.body.usage.total_tokens(), 20);
    let root_line = report
        .body
        .agents
        .iter()
        .find(|agent| agent.id == root)
        .unwrap();
    assert_eq!(root_line.subtree_usage.cost_gap, Some(CostGap::Mixed));
    assert!(report.digest().contains("multiple cost gaps"));
}
