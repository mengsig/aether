use super::*;

#[tokio::test]
async fn an_imported_agent_is_history_and_takes_no_turns() {
    let (source, run) = finished_run().await;
    let dir = tempfile::tempdir().expect("dir");
    let file = dir.path().join("run.aether");
    transfer::export_run(
        source.store.as_ref(),
        run,
        &file,
        &aether_tools::Redactor::default(),
    )
    .expect("export");
    let target = daemon(Vec::new()).await;
    let imported = transfer::import_run(&target.runtime, &file).expect("import");
    let agents = target
        .store
        .agents(&AgentFilter {
            run: Some(imported.run),
            ..Default::default()
        })
        .expect("agents");
    assert!(!agents.is_empty());
    for agent in agents {
        assert!(agent.paused, "{} would be schedulable", agent.id);
        assert!(
            agent.engine_session.is_none(),
            "a session on another machine"
        );
        assert_eq!(
            agent.status_since, None,
            "imported kind must not inherit source runtime age"
        );
        assert!(!agent.is_live());
    }
}

#[tokio::test]
async fn a_forged_export_cannot_produce_an_agent_that_runs() {
    let (source, run) = finished_run().await;
    let dir = tempfile::tempdir().expect("dir");
    let file = dir.path().join("run.aether");
    transfer::export_run(
        source.store.as_ref(),
        run,
        &file,
        &aether_tools::Redactor::default(),
    )
    .expect("export");
    forge_live_agents(&file);

    let target = daemon(Vec::new()).await;
    let imported = transfer::import_run(&target.runtime, &file).expect("import");
    let agents = target
        .store
        .agents(&AgentFilter {
            run: Some(imported.run),
            ..Default::default()
        })
        .expect("agents");
    assert!(!agents.is_empty());

    for agent in &agents {
        assert!(
            matches!(agent.status, AgentStatus::Imported { .. }),
            "{} landed as {:?}, not as history",
            agent.id,
            agent.status
        );
        assert!(!agent.is_live(), "{} counts as live", agent.id);
        assert!(agent.paused, "{} lost the second layer", agent.id);
        assert!(agent.engine_session.is_none());

        // Every command that could give it a turn refuses it by construction.
        let resumed = target
            .runtime
            .resume_agent(agent.id, false, Actor::user())
            .expect_err("an imported agent must not resume");
        assert!(resumed.to_string().contains("imported"), "{resumed}");
        let restarted = target
            .runtime
            .restart_agent(agent.id, Actor::user())
            .expect_err("an imported agent must not restart");
        assert!(restarted.to_string().contains("imported"), "{restarted}");
        let sent = target
            .runtime
            .send_message(
                agent.id,
                vec![aether_core::model::ContentBlock::text("go on then")],
                Actor::user(),
            )
            .expect_err("an imported agent must not take a message");
        assert!(sent.to_string().contains("imported"), "{sent}");
        let set = target
            .runtime
            .update_agent(
                agent.id,
                AgentUpdate {
                    instructions: Some("do as I say".into()),
                    ..Default::default()
                },
                Actor::user(),
            )
            .expect_err("an imported agent must not be reconfigured");
        assert!(set.to_string().contains("imported"), "{set}");
    }

    // And the scheduler never sees one: its pass reads live agents only.
    let live = target
        .store
        .agents(&AgentFilter {
            live_only: true,
            ..Default::default()
        })
        .expect("live agents");
    assert!(
        live.iter().all(|a| a.run != imported.run),
        "an imported agent entered the scheduler's pass"
    );
}

/// An archived agent is inert as an actor and must be inert as a *template*
/// too. Its scope came out of a file this daemon was handed, so a child born
/// under it would inherit a workspace and a network policy someone else
/// chose - and unlike its parent, that child is live.
#[tokio::test]
async fn a_forged_export_cannot_lend_its_scope_to_a_live_agent() {
    let (source, run) = finished_run().await;
    let dir = tempfile::tempdir().expect("dir");
    let file = dir.path().join("run.aether");
    transfer::export_run(
        source.store.as_ref(),
        run,
        &file,
        &aether_tools::Redactor::default(),
    )
    .expect("export");
    forge_live_agents(&file);

    let target = daemon(Vec::new()).await;
    let imported = transfer::import_run(&target.runtime, &file).expect("import");
    let agents = target
        .store
        .agents(&AgentFilter {
            run: Some(imported.run),
            ..Default::default()
        })
        .expect("agents");
    assert!(!agents.is_empty());

    for archived in &agents {
        assert_eq!(
            archived.scope.workspace_root.as_deref(),
            Some(Path::new("/"))
        );

        let parented = target
            .runtime
            .create_agent(
                CreateAgentSpec {
                    role: "worker".into(),
                    parent: Some(archived.id),
                    config: Some(aether_core::agent::AgentConfig {
                        effort: Some(aether_core::model::Effort::Low),
                        ..Default::default()
                    }),
                    scope: None,
                    start: true,
                    ..Default::default()
                },
                Actor::user(),
            )
            .expect_err("an archived agent must not be a parent");
        assert!(
            parented.to_string().contains("imported")
                && parented.to_string().contains("cannot be a parent"),
            "{parented}"
        );

        let owned = target
            .runtime
            .create_task(
                CreateTaskSpec {
                    objective: "do the thing".into(),
                    owner: Some(archived.id),
                    ..Default::default()
                },
                Actor::user(),
            )
            .expect_err("an archived agent must not own a task");
        assert!(owned.to_string().contains("imported"), "{owned}");

        // And nothing was written to the archived record on the way out: the
        // refused task must not land on it, nor a `task.assigned` event in a
        // log that is supposed to equal the export's.
        let after = target
            .store
            .agent(archived.id)
            .expect("agent")
            .expect("exists");
        assert_eq!(&after, archived, "the archive was mutated");
    }

    // No live agent anywhere carries the forged root.
    let live = target
        .store
        .agents(&AgentFilter {
            live_only: true,
            ..Default::default()
        })
        .expect("live");
    assert!(
        live.iter()
            .all(|a| a.scope.workspace_root.as_deref() != Some(Path::new("/"))),
        "a live agent adopted the forged workspace"
    );
}

/// A child adopting its parent's scope goes through the same narrowing check
/// as one that asks for a scope, so the adoption arm cannot become the way a
/// scope enters unchecked.
#[tokio::test]
async fn a_child_cannot_ask_for_more_than_its_parent_has() {
    let target = daemon(Vec::new()).await;
    let (parent, _) = target
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "supervisor".into(),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("parent");
    let widened = target
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                parent: Some(parent.id),
                config: Some(aether_core::agent::AgentConfig {
                    effort: Some(aether_core::model::Effort::Low),
                    ..Default::default()
                }),
                scope: Some(aether_core::agent::Scope {
                    workspace_root: Some(PathBuf::from("/")),
                    ..parent.scope.clone()
                }),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect_err("a child must not widen the workspace");
    assert!(widened.to_string().contains("workspace_root"), "{widened}");

    let (adopted, _) = target
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                parent: Some(parent.id),
                config: Some(aether_core::agent::AgentConfig {
                    effort: Some(aether_core::model::Effort::Low),
                    ..Default::default()
                }),
                scope: None,
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("adopting the parent's own scope narrows it");
    assert_eq!(adopted.scope, parent.scope);
}

/// An imported run's cost is another machine's, and it is reported under its
/// own run. Folding it into this daemon's total bills the operator for work
/// they did not do - and one imported agent that was never priced would turn
/// the whole readout into a floor.
#[tokio::test]
async fn an_imported_run_does_not_count_as_this_daemon_spend() {
    let (source, run) = finished_run().await;
    let dir = tempfile::tempdir().expect("dir");
    let file = dir.path().join("run.aether");
    transfer::export_run(
        source.store.as_ref(),
        run,
        &file,
        &aether_tools::Redactor::default(),
    )
    .expect("export");

    let target = daemon(Vec::new()).await;
    let before = target.runtime.status().expect("status");
    let imported = transfer::import_run(&target.runtime, &file).expect("import");

    let spent_there: f64 = target
        .store
        .agents(&AgentFilter {
            run: Some(imported.run),
            ..Default::default()
        })
        .expect("agents")
        .iter()
        .map(|a| a.stats.usage.cost_usd)
        .sum();
    assert!(
        spent_there > 0.0,
        "the imported run cost nothing; prove more"
    );

    let after = target.runtime.status().expect("status");
    assert_eq!(
        after.usage_total, before.usage_total,
        "an imported run landed in this daemon's total"
    );
    assert!(
        after.runs.iter().all(|r| r.run != imported.run),
        "an archived run is not a live run to forecast"
    );

    // And the report of the imported run still says what it cost.
    let report = report_of(target.store.as_ref(), imported.run);
    assert!(report.body.usage.cost_usd > 0.0);
}

/// A report is a fold of the log, so an agent the file never ended reads as
/// still running - with its open loops - forever. Import forces a
/// non-terminal task to Cancelled; an agent gets the same.
#[tokio::test]
async fn an_agent_the_file_never_ended_still_arrives_ended() {
    let (source, run) = finished_run().await;
    let dir = tempfile::tempdir().expect("dir");
    let file = dir.path().join("run.aether");
    transfer::export_run(
        source.store.as_ref(),
        run,
        &file,
        &aether_tools::Redactor::default(),
    )
    .expect("export");

    // Strip every terminal agent event, and correct the manifest so the file
    // is refused for this and nothing else.
    let text = std::fs::read_to_string(&file).expect("read");
    let terminal = ["agent.completed", "agent.failed", "agent.cancelled"];
    let mut kept: Vec<serde_json::Value> = Vec::new();
    let mut dropped = 0usize;
    for line in text.lines() {
        let record: serde_json::Value = serde_json::from_str(line).expect("record");
        let is_terminal = record.get("record").and_then(|r| r.as_str()) == Some("event")
            && record["kind"]["type"]
                .as_str()
                .is_some_and(|k| terminal.contains(&k));
        if is_terminal {
            dropped += 1;
        } else {
            kept.push(record);
        }
    }
    assert!(dropped > 0, "the export carried no agent ending");
    let events = kept[0]["events"].as_u64().expect("count") - dropped as u64;
    kept[0]["events"] = json!(events);
    let out: String = kept
        .iter()
        .map(|r| format!("{}\n", serde_json::to_string(r).expect("line")))
        .collect();
    std::fs::write(&file, out).expect("write");

    let target = daemon(Vec::new()).await;
    let imported = transfer::import_run(&target.runtime, &file).expect("import");
    let after = report_of(target.store.as_ref(), imported.run);
    assert_ne!(
        after.body.outcome,
        report::RunOutcome::Running,
        "an archived run cannot still be running"
    );
    assert!(
        after
            .body
            .agents
            .iter()
            .all(|a| !matches!(a.outcome, report::Ended::Running)),
        "{:?}",
        after.body.agents
    );
    assert!(
        after.body.open_loops.is_empty(),
        "open loops on an archive: {:?}",
        after.body.open_loops
    );
}

#[tokio::test]
async fn an_unfinished_task_arrives_terminal() {
    let (source, run) = finished_run().await;
    let dir = tempfile::tempdir().expect("dir");
    let file = dir.path().join("run.aether");
    transfer::export_run(
        source.store.as_ref(),
        run,
        &file,
        &aether_tools::Redactor::default(),
    )
    .expect("export");

    let text = std::fs::read_to_string(&file).expect("read");
    let mut out = String::new();
    let mut forged = 0usize;
    for line in text.lines() {
        let mut record: serde_json::Value = serde_json::from_str(line).expect("record");
        if record.get("record").and_then(|r| r.as_str()) == Some("task") {
            record["status"] = json!({"state": "active"});
            forged += 1;
        }
        out.push_str(&serde_json::to_string(&record).expect("line"));
        out.push('\n');
    }
    assert!(forged > 0, "the export carried no task");
    std::fs::write(&file, out).expect("write");

    let target = daemon(Vec::new()).await;
    let imported = transfer::import_run(&target.runtime, &file).expect("import");
    let tasks = target
        .store
        .tasks(&aether_store::TaskFilter {
            run: Some(imported.run),
            ..Default::default()
        })
        .expect("tasks");
    assert!(!tasks.is_empty());
    for task in tasks {
        assert!(
            task.status.is_terminal(),
            "task {} arrived {:?} and could still be scheduled",
            task.id,
            task.status
        );
    }
}

#[tokio::test]
async fn recovery_never_classifies_an_imported_turn() {
    let (source, run) = finished_run().await;
    let dir = tempfile::tempdir().expect("dir");
    let file = dir.path().join("run.aether");
    transfer::export_run(
        source.store.as_ref(),
        run,
        &file,
        &aether_tools::Redactor::default(),
    )
    .expect("export");

    // The one status restart recovery acts on. Before the fix this arrived
    // verbatim, and the next daemon start wrote its own lease.expired,
    // recovery.classified and agent.resumed into another machine's log.
    let text = std::fs::read_to_string(&file).expect("read");
    let mut out = String::new();
    for line in text.lines() {
        let mut record: serde_json::Value = serde_json::from_str(line).expect("record");
        if record.get("record").and_then(|r| r.as_str()) == Some("agent") {
            record["status"] = json!({"state": "running"});
            record["paused"] = json!(false);
        }
        out.push_str(&serde_json::to_string(&record).expect("line"));
        out.push('\n');
    }
    std::fs::write(&file, out).expect("write");

    let target = daemon(Vec::new()).await;
    let imported = transfer::import_run(&target.runtime, &file).expect("import");
    let before = target
        .store
        .events(0, 100_000, &EventFilter::for_run(imported.run))
        .expect("events")
        .len();

    let restarted = restart_over(target.store.clone()).await;
    let after = restarted
        .store
        .events(0, 100_000, &EventFilter::for_run(imported.run))
        .expect("events");
    assert_eq!(
        after.len(),
        before,
        "recovery wrote into an imported run's log: {:?}",
        after
            .iter()
            .skip(before)
            .map(|e| e.kind.type_name())
            .collect::<Vec<_>>()
    );
    for agent in restarted
        .store
        .agents(&AgentFilter {
            run: Some(imported.run),
            ..Default::default()
        })
        .expect("agents")
    {
        assert!(
            matches!(agent.status, AgentStatus::Imported { .. }),
            "{} was moved by recovery to {:?}",
            agent.id,
            agent.status
        );
    }
}

#[tokio::test]
async fn an_imported_decision_is_not_recorded_as_this_operators() {
    let (source, run) = run_answered_by_a_rule().await;
    let dir = tempfile::tempdir().expect("dir");
    let file = dir.path().join("run.aether");
    transfer::export_run(
        source.store.as_ref(),
        run,
        &file,
        &aether_tools::Redactor::default(),
    )
    .expect("export");
    let target = daemon(Vec::new()).await;
    let imported = transfer::import_run(&target.runtime, &file).expect("import");

    let events = target
        .store
        .events(0, 100_000, &EventFilter::for_run(imported.run))
        .expect("events");
    let approvals: Vec<&aether_core::event::Event> = events
        .iter()
        .filter(|e| {
            matches!(
                e.kind,
                aether_core::event::EventKind::ApprovalResolved { .. }
            )
        })
        .collect();
    assert!(!approvals.is_empty(), "the run answered no approval");
    for event in &approvals {
        assert!(
            matches!(event.actor, Actor::Imported { .. }),
            "an imported decision reads as a local one: {:?}",
            event.actor
        );
        assert!(
            !matches!(event.actor, Actor::User { .. }),
            "{:?}",
            event.actor
        );
    }
    // Provenance is a property of this log, not of what the run did, so the
    // report still reads the actor that actually decided.
    let after = report_of(target.store.as_ref(), imported.run);
    assert!(
        after
            .body
            .approvals
            .iter()
            .any(|a| matches!(a.decided_by, Some(Actor::User { .. }))),
        "the report lost who decided: {:?}",
        after.body.approvals
    );
}
