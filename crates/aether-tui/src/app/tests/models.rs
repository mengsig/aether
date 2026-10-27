//! models reducer regressions.
use super::*;

#[test]
fn catalogue_lookup_respects_provider_and_case_and_remembers_aliases_canonically() {
    let (mut app, _) = app_with_agent();
    let entry = |provider: &str| ModelInfo {
        tier: None,
        model: ModelRef::new(provider, "claude-opus-5-5"),
        aliases: vec!["opus".into(), "opus-5-5".into()],
        name: "Claude Opus 5.5".into(),
        provider_name: provider.into(),
        capabilities: aether_core::model::CapabilityProfile {
            context_window: 1_000_000,
            compaction_tokens: Some(256_000),
            ..Default::default()
        },
        configured: true,
        auth_source: None,
        is_default: false,
    };
    app.models = vec![entry("anthropic"), entry("claude-code")];
    for name in ["claude-opus-5-5", "opus", "opus-5-5"] {
        let info = app
            .model_info(&ModelRef::new("claude-code", name))
            .expect("opus");
        assert_eq!(info.model, ModelRef::new("claude-code", "claude-opus-5-5"));
        assert_eq!(
            app.context_window(&ModelRef::new("claude-code", name)),
            Some(1_000_000)
        );
        assert_eq!(
            app.compaction_threshold(&ModelRef::new("claude-code", name)),
            Some(256_000)
        );
    }
    for (provider, name) in [
        ("codex", "opus"),
        ("Claude-Code", "opus"),
        ("claude-code", "Opus"),
        ("claude-code", "CLAUDE-OPUS-5-5"),
    ] {
        assert!(app.model_info(&ModelRef::new(provider, name)).is_none());
    }
    // Choosing a model never reorders the catalogue lookups resolve against.
    app.remember_model(&ModelChain::single(ModelRef::new("claude-code", "opus")));
    assert_eq!(app.models[0].model.provider, "anthropic");
    assert_eq!(app.recent_models, ["claude-code/claude-opus-5-5"]);
}

/// `/model <ref>` sets the agent's next turn. It switches the
/// agent in front of you; `--default` moves what new agents get without
/// touching it; both are remembered, and the memory orders the picker.
#[test]
fn model_switches_the_current_agent_and_default_moves_only_the_session() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (mut app, id) = app_with_agent();
    app.options.prefs_dir = Some(dir.path().to_path_buf());

    // A plain switch patches the agent and leaves the session default be.
    let effects = app.run_command(SlashCommand::Model {
        chain: ModelChain::single(ModelRef::new("anthropic", "opus")),
        default_only: false,
    });
    assert!(matches!(
        &effects[..],
        [Effect::Request {
            command: Command::UpdateAgent { id: patched, patch, subtree: false, idempotency_key: None, },
            ..
        }] if *patched == id && patch.model == Some(ModelRef::new("anthropic", "opus"))
    ));
    assert_eq!(
        app.options.default_model, None,
        "no /new needed, and no default moved"
    );
    let mut patched = agent(id, "root");
    patched.model = ModelRef::new("anthropic", "opus");
    app.reduce(AppEvent::Reply {
        then: Then::ModelPatched(id),
        result: Ok(Reply::Agent(patched)),
    });

    // --default moves the default, sends nothing, and is saved.
    let effects = app.run_command(SlashCommand::Model {
        chain: ModelChain::single(ModelRef::new("claude-code", "haiku")),
        default_only: true,
    });
    assert!(effects.is_empty(), "{effects:?}");
    assert_eq!(
        app.options.default_model,
        Some(ModelRef::new("claude-code", "haiku"))
    );
    let saved = crate::prefs::UiPrefs::load(dir.path()).expect("load");
    assert_eq!(saved.model.as_deref(), Some("claude-code/haiku"));
    assert_eq!(
        saved.recent_models,
        vec!["claude-code/haiku".to_owned(), "anthropic/opus".to_owned()],
        "newest first"
    );

    // The catalogue keeps registry order: the picker, not the lookup,
    // lifts recent choices.
    app.reduce(AppEvent::Reply {
        then: Then::Models,
        result: Ok(Reply::Models(
            ["mock/a", "anthropic/opus", "claude-code/haiku"]
                .into_iter()
                .map(|m| ModelInfo {
                    tier: None,
                    aliases: Vec::new(),
                    model: ModelChain::parse(m).expect("model").primary,
                    name: m.to_owned(),
                    provider_name: "p".into(),
                    capabilities: Default::default(),
                    configured: true,
                    auth_source: None,
                    is_default: false,
                })
                .collect(),
        )),
    });
    let order: Vec<String> = app.models.iter().map(|m| m.model.to_string()).collect();
    assert_eq!(order, ["mock/a", "anthropic/opus", "claude-code/haiku"]);
}

#[test]
fn choosing_a_model_without_an_agent_waits_for_a_message() {
    let mut app = app_with_status(TuiOptions::default());
    let model = ModelRef::new("claude-code", "opus");
    let effects = app.run_command(SlashCommand::Model {
        chain: ModelChain::single(model.clone()),
        default_only: false,
    });
    assert!(
        effects.is_empty(),
        "model selection started an agent: {effects:?}"
    );
    assert_eq!(app.options.default_model, Some(model.clone()));

    for c in "hello".chars() {
        app.reduce(key(KeyCode::Char(c)));
    }
    let effects = app.reduce(key(KeyCode::Enter));
    assert!(matches!(
        &effects[..],
        [Effect::Request { command: Command::CreateAgent(c), .. }]
            if c.model.as_ref() == Some(&model)
    ));
}

#[test]
fn choosing_a_model_for_a_new_root_before_tree_refresh_updates_that_root() {
    let mut app = app_with_status(TuiOptions::default());
    let start = app.run_command(SlashCommand::New { objective: None });
    let id = AgentId::new();
    app.reduce(AppEvent::Reply {
        then: start_then(&start),
        result: Ok(Reply::Agent(agent(id, "assistant"))),
    });
    assert_eq!(app.selected, Some(id));
    assert!(!app.tree.nodes.contains_key(&id));

    let model = ModelRef::new("codex", "gpt-6-sol");
    let effects = app.run_command(SlashCommand::Model {
        chain: ModelChain::single(model.clone()),
        default_only: false,
    });
    assert!(effects.iter().any(|effect| matches!(
        effect,
        Effect::Request {
            command: Command::UpdateAgent { id: target, patch, subtree: false, idempotency_key: None, },
            then: Then::ModelPatched(reply_id),
        } if *target == id && *reply_id == id && patch.model.as_ref() == Some(&model)
    )));
    assert!(effects.iter().all(|effect| !matches!(
        effect,
        Effect::Request {
            command: Command::CreateAgent(_),
            ..
        }
    )));
}

#[test]
fn choosing_a_model_with_a_finished_agent_waits_for_a_message() {
    let (mut app, id) = app_with_agent();
    app.tree
        .nodes
        .get_mut(&id)
        .expect("selected agent")
        .agent
        .status = AgentStatus::Completed { summary: None };

    let model = ModelRef::new("codex", "gpt-6-sol");
    let effects = app.run_command(SlashCommand::Model {
        chain: ModelChain::single(model.clone()),
        default_only: false,
    });

    assert!(
        effects.is_empty(),
        "model selection started an agent: {effects:?}"
    );
    assert_eq!(app.options.default_model, Some(model.clone()));
    for c in "hello".chars() {
        app.reduce(key(KeyCode::Char(c)));
    }
    let effects = app.reduce(key(KeyCode::Enter));
    assert!(matches!(
        &effects[..],
        [Effect::Request { command: Command::CreateAgent(request), .. }]
            if request.model.as_ref() == Some(&model)
    ));
}

#[test]
fn choosing_a_model_with_a_failed_agent_keeps_it_for_recovery() {
    let (mut app, id) = app_with_agent();
    app.tree
        .nodes
        .get_mut(&id)
        .expect("selected agent")
        .agent
        .status = AgentStatus::Failed {
        error: "quota".into(),
    };

    let effects = app.run_command(SlashCommand::Model {
        chain: ModelChain::single(ModelRef::new("codex", "gpt-6-luna")),
        default_only: false,
    });

    assert!(matches!(
        &effects[..],
        [Effect::Request { command: Command::UpdateAgent { id: target, .. }, .. }]
            if *target == id
    ));
    let mut patched = agent(id, "root");
    patched.model = ModelRef::new("codex", "gpt-6-luna");
    patched.status = AgentStatus::Failed {
        error: "quota".into(),
    };
    app.reduce(AppEvent::Reply {
        then: Then::ModelPatched(id),
        result: Ok(Reply::Agent(patched)),
    });
    assert!(
        app.live_flash()
            .is_some_and(|notice| notice.contains("/wake to retry"))
    );
}

#[test]
fn choosing_a_model_on_a_restarted_agent_updates_its_live_successor() {
    let (mut app, old) = app_with_agent();
    let successor = AgentId::new();
    let mut successor_node = app.tree.nodes.get(&old).expect("old agent").clone();
    successor_node.agent.id = successor;
    app.tree.nodes.insert(successor, successor_node);
    let old_agent = &mut app.tree.nodes.get_mut(&old).expect("old agent").agent;
    old_agent.status = AgentStatus::Failed {
        error: "quota".into(),
    };
    old_agent.restarted_as = Some(successor);

    let model = ModelRef::new("codex", "gpt-6-sol");
    let effects = app.run_command(SlashCommand::Model {
        chain: ModelChain::single(model.clone()),
        default_only: false,
    });

    assert_eq!(app.selected, Some(successor));
    assert!(effects.iter().any(|effect| matches!(
        effect,
        Effect::Request {
            command: Command::UpdateAgent { id, patch, subtree: false, idempotency_key: None, },
            then: Then::ModelPatched(reply_id),
        } if *id == successor && *reply_id == successor && patch.model.as_ref() == Some(&model)
    )));
}

#[test]
fn choosing_a_model_on_a_restarted_agent_without_a_successor_waits_for_a_message() {
    let (mut app, old) = app_with_agent();
    let old_agent = &mut app.tree.nodes.get_mut(&old).expect("old agent").agent;
    old_agent.status = AgentStatus::Failed {
        error: "quota".into(),
    };
    old_agent.restarted_as = Some(AgentId::new());

    let model = ModelRef::new("codex", "gpt-6-luna");
    let effects = app.run_command(SlashCommand::Model {
        chain: ModelChain::single(model.clone()),
        default_only: false,
    });

    assert!(
        effects.is_empty(),
        "model selection started an agent: {effects:?}"
    );
    assert_eq!(app.options.default_model, Some(model.clone()));
    for c in "hello".chars() {
        app.reduce(key(KeyCode::Char(c)));
    }
    let effects = app.reduce(key(KeyCode::Enter));
    assert!(matches!(
        &effects[..],
        [Effect::Request { command: Command::CreateAgent(request), .. }]
            if request.model.as_ref() == Some(&model)
    ));
}

#[test]
fn rejected_model_change_does_not_enter_recent_models() {
    let (mut app, id) = app_with_agent();
    let chain = ModelChain::single(ModelRef::new("codex", "gpt-6-sol"));
    let effects = app.run_command(SlashCommand::Model {
        chain,
        default_only: false,
    });
    assert!(matches!(
        &effects[..],
        [Effect::Request {
            then: Then::ModelPatched(_),
            ..
        }]
    ));
    assert!(app.recent_models.is_empty());

    app.reduce(AppEvent::Reply {
        then: Then::ModelPatched(id),
        result: Err(ClientError::Protocol(aether_protocol::ErrorReply::new(
            aether_protocol::ErrorCode::InvalidArgument,
            "model unavailable",
        ))),
    });
    assert!(app.recent_models.is_empty());
}

#[test]
fn running_model_change_says_when_it_will_take_effect() {
    let (mut app, id) = app_with_agent();
    app.tree.nodes.get_mut(&id).expect("agent").agent.status = AgentStatus::Running;
    let model = ModelRef::new("codex", "gpt-6-sol");

    app.run_command(SlashCommand::Model {
        chain: ModelChain::single(model.clone()),
        default_only: false,
    });
    assert!(app.live_flash().is_some_and(
        |notice| notice.contains("next turn") && notice.contains("current turn continues")
    ));

    let mut patched = agent(id, "root");
    patched.model = model;
    patched.status = AgentStatus::Running;
    app.reduce(AppEvent::Reply {
        then: Then::ModelPatched(id),
        result: Ok(Reply::Agent(patched)),
    });
    assert!(app.live_flash().is_some_and(
        |notice| notice.contains("next turn") && notice.contains("current turn continues")
    ));
}

#[test]
fn rapid_model_choices_apply_the_latest_after_the_first_reply() {
    let (mut app, id) = app_with_agent();
    let first = ModelRef::new("codex", "gpt-6-sol");
    let middle = ModelRef::new("codex", "gpt-6-luna");
    let last = ModelRef::new("mock", "final");
    let choose = |app: &mut App, model: ModelRef| {
        app.run_command(SlashCommand::Model {
            chain: ModelChain::single(model),
            default_only: false,
        })
    };

    assert!(choose(&mut app, first.clone()).iter().any(|effect| matches!(
        effect,
        Effect::Request { command: Command::UpdateAgent { id: target, .. }, .. } if *target == id
    )));
    for model in [middle, last.clone()] {
        assert!(
            choose(&mut app, model).iter().all(|effect| !matches!(
                effect,
                Effect::Request {
                    command: Command::UpdateAgent { .. },
                    ..
                }
            )),
            "the previous model update must settle before the latest is sent"
        );
    }
    assert_eq!(
        app.selected_requested_model().map(|m| &m.primary),
        Some(&last)
    );

    let mut first_reply = agent(id, "root");
    first_reply.model = first;
    let effects = app.reduce(AppEvent::Reply {
        then: Then::ModelPatched(id),
        result: Ok(Reply::Agent(first_reply)),
    });
    assert!(effects.iter().any(|effect| matches!(
        effect,
        Effect::Request {
            command: Command::UpdateAgent { id: target, patch, subtree: false, idempotency_key: None, },
            then: Then::ModelPatched(reply_id),
        } if *target == id && *reply_id == id && patch.model.as_ref() == Some(&last)
    )));

    let mut final_reply = agent(id, "root");
    final_reply.model = last.clone();
    app.reduce(AppEvent::Reply {
        then: Then::ModelPatched(id),
        result: Ok(Reply::Agent(final_reply)),
    });
    assert!(app.selected_requested_model().is_none());
    assert_eq!(app.recent_models.first(), Some(&last.to_string()));
}

#[test]
fn latest_model_choice_is_tried_after_an_earlier_choice_fails() {
    let (mut app, id) = app_with_agent();
    let choose = |app: &mut App, model| {
        app.run_command(SlashCommand::Model {
            chain: ModelChain::single(model),
            default_only: false,
        })
    };
    choose(&mut app, ModelRef::new("codex", "unavailable"));
    let latest = ModelRef::new("codex", "gpt-6-sol");
    assert!(choose(&mut app, latest.clone()).is_empty());

    let effects = app.reduce(AppEvent::Reply {
        then: Then::ModelPatched(id),
        result: Err(ClientError::Protocol(aether_protocol::ErrorReply::new(
            aether_protocol::ErrorCode::InvalidArgument,
            "model unavailable",
        ))),
    });
    assert!(effects.iter().any(|effect| matches!(
        effect,
        Effect::Request {
            command: Command::UpdateAgent { id: target, patch, subtree: false, idempotency_key: None, },
            then: Then::ModelPatched(reply_id),
        } if *target == id && *reply_id == id && patch.model.as_ref() == Some(&latest)
    )));
    assert!(app.live_flash().is_some_and(|notice| {
        notice.contains("model unavailable") && notice.contains("trying the latest")
    }));
}

#[test]
fn model_chosen_during_root_creation_patches_it_before_queued_messages() {
    let mut app = app_with_status(TuiOptions::default());
    let created = app.run_command(SlashCommand::New {
        objective: Some("first task".into()),
    });
    assert!(matches!(
        &created[..],
        [Effect::Request {
            command: Command::CreateAgent(_),
            ..
        }]
    ));

    let model = ModelRef::new("codex", "gpt-6-sol");
    let effects = app.run_command(SlashCommand::Model {
        chain: ModelChain::single(model.clone()),
        default_only: false,
    });
    assert!(
        effects.is_empty(),
        "a second root was requested: {effects:?}"
    );
    assert_eq!(app.options.default_model, Some(model.clone()));
    assert_eq!(
        app.awaited_in_flight()
            .and_then(|start| start.model.as_ref())
            .map(|m| &m.primary),
        Some(&model)
    );

    let fresh = AgentId::new();
    for c in "second task".chars() {
        app.reduce(key(KeyCode::Char(c)));
    }
    assert!(app.reduce(key(KeyCode::Enter)).is_empty());
    let effects = app.reduce(AppEvent::Reply {
        then: start_then(&created),
        result: Ok(Reply::Agent(agent(fresh, "assistant"))),
    });
    assert!(effects.iter().any(|effect| matches!(
        effect,
        Effect::Request {
            command: Command::UpdateAgent { id, patch, subtree: false, idempotency_key: None, },
            then: Then::PendingRootModelPatched(reply_id),
        } if *id == fresh && *reply_id == fresh && patch.model.as_ref() == Some(&model)
    )));
    assert!(effects.iter().all(|effect| !matches!(
        effect,
        Effect::Request {
            command: Command::SendMessage { .. },
            ..
        }
    )));
    assert!(app.send_message(fresh, "third task".into()).is_empty());

    let mut patched = agent(fresh, "assistant");
    patched.model = model;
    let effects = app.reduce(AppEvent::Reply {
        then: Then::PendingRootModelPatched(fresh),
        result: Ok(Reply::Agent(patched)),
    });
    let sent: Vec<_> = effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Request {
                command: Command::SendMessage { to, content, .. },
                ..
            } if *to == fresh => Some(content.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(sent, vec!["first task", "second task", "third task"]);
    assert!(!app.holds_root(fresh));
}

#[test]
fn failed_pending_root_model_change_holds_messages_for_retry() {
    let mut app = app_with_status(TuiOptions::default());
    let start = app.run_command(SlashCommand::New {
        objective: Some("first task".into()),
    });
    let chain = ModelChain::single(ModelRef::new("codex", "gpt-6-luna"));
    assert!(
        app.run_command(SlashCommand::Model {
            chain: chain.clone(),
            default_only: false
        })
        .is_empty()
    );

    let fresh = AgentId::new();
    app.reduce(AppEvent::Reply {
        then: start_then(&start),
        result: Ok(Reply::Agent(agent(fresh, "assistant"))),
    });
    app.reduce(AppEvent::Reply {
        then: Then::PendingRootModelPatched(fresh),
        result: Err(ClientError::Protocol(aether_protocol::ErrorReply::new(
            aether_protocol::ErrorCode::InvalidArgument,
            "model unavailable",
        ))),
    });
    assert_eq!(
        app.held_messages(fresh).cloned(),
        Some(vec!["first task".into()])
    );
    assert!(
        app.live_flash()
            .is_some_and(|notice| notice.contains("queued messages were held"))
    );

    let effects = app.run_command(SlashCommand::Model {
        chain,
        default_only: false,
    });
    assert!(effects.iter().any(|effect| matches!(
        effect,
        Effect::Request { command: Command::UpdateAgent { id, .. }, then: Then::PendingRootModelPatched(reply_id) }
            if *id == fresh && *reply_id == fresh
    )));
    let mut patched = agent(fresh, "assistant");
    patched.model = ModelRef::new("codex", "gpt-6-luna");
    let effects = app.reduce(AppEvent::Reply {
        then: Then::PendingRootModelPatched(fresh),
        result: Ok(Reply::Agent(patched)),
    });
    assert!(effects.iter().any(|effect| matches!(
        effect,
        Effect::Request { command: Command::SendMessage { to, content, .. }, .. }
            if *to == fresh && content == "first task"
    )));
    assert!(!app.holds_root(fresh));
}

#[test]
fn repeated_model_choice_during_pending_patch_waits_for_the_last_model() {
    let mut app = app_with_status(TuiOptions::default());
    let start = app.run_command(SlashCommand::New {
        objective: Some("first task".into()),
    });
    let first = ModelRef::new("codex", "gpt-6-luna");
    app.run_command(SlashCommand::Model {
        chain: ModelChain::single(first.clone()),
        default_only: false,
    });
    let fresh = AgentId::new();
    app.reduce(AppEvent::Reply {
        then: start_then(&start),
        result: Ok(Reply::Agent(agent(fresh, "assistant"))),
    });

    let last = ModelRef::new("codex", "gpt-6-sol");
    let effects = app.run_command(SlashCommand::Model {
        chain: ModelChain::single(last.clone()),
        default_only: false,
    });
    assert!(
        effects.is_empty(),
        "model updates must be serialized: {effects:?}"
    );
    assert_eq!(
        app.selected_requested_model().map(|m| &m.primary),
        Some(&last)
    );

    let mut first_patch = agent(fresh, "assistant");
    first_patch.model = first;
    let effects = app.reduce(AppEvent::Reply {
        then: Then::PendingRootModelPatched(fresh),
        result: Ok(Reply::Agent(first_patch)),
    });
    assert!(effects.iter().any(|effect| matches!(
        effect,
        Effect::Request {
            command: Command::UpdateAgent { id, patch, subtree: false, idempotency_key: None, },
            then: Then::PendingRootModelPatched(reply_id),
        } if *id == fresh && *reply_id == fresh && patch.model.as_ref() == Some(&last)
    )));
    assert!(effects.iter().all(|effect| !matches!(
        effect,
        Effect::Request {
            command: Command::SendMessage { .. },
            ..
        }
    )));

    let mut final_patch = agent(fresh, "assistant");
    final_patch.model = last;
    let effects = app.reduce(AppEvent::Reply {
        then: Then::PendingRootModelPatched(fresh),
        result: Ok(Reply::Agent(final_patch)),
    });
    assert!(effects.iter().any(|effect| matches!(
        effect,
        Effect::Request { command: Command::SendMessage { to, content, .. }, .. }
            if *to == fresh && content == "first task"
    )));
}

#[test]
fn failed_root_creation_clears_its_pending_model_choice() {
    let mut app = app_with_status(TuiOptions::default());
    let start = app.run_command(SlashCommand::New { objective: None });
    app.run_command(SlashCommand::Model {
        chain: ModelChain::single(ModelRef::new("codex", "gpt-6-sol")),
        default_only: false,
    });

    app.reduce(AppEvent::Reply {
        then: start_then(&start),
        result: Err(ClientError::Protocol(aether_protocol::ErrorReply::new(
            aether_protocol::ErrorCode::InvalidArgument,
            "could not create agent",
        ))),
    });

    assert!(app.awaited_in_flight().is_none());
    assert!(app.root_starts.values().all(|start| start.model.is_none()));
    assert!(
        app.live_flash()
            .is_some_and(|notice| notice.contains("could not create agent"))
    );
}

#[test]
fn effort_chosen_before_an_agent_applies_to_the_first_one() {
    let mut app = app_with_status(TuiOptions::default());
    assert!(
        app.run_command(SlashCommand::Effort(Effort::Max))
            .is_empty()
    );
    let effects = app.run_command(SlashCommand::New { objective: None });
    assert!(matches!(
        &effects[..],
        [Effect::Request { command: Command::CreateAgent(c), .. }]
            if c.config.as_ref().and_then(|c| c.effort) == Some(Effort::Max)
    ));
}

#[test]
fn palette_runs_the_highlighted_command() {
    use crate::ui::bottom::popup::{self, PopupItem};
    let mut app = App::new(TuiOptions::default());
    for c in "/m".chars() {
        app.reduce(key(KeyCode::Char(c)));
    }
    let popup = app.bottom.popup.as_ref().expect("/ opens the palette");
    let names: Vec<&str> = popup::items(popup, &app)
        .into_iter()
        .filter_map(|item| match item {
            PopupItem::Command { name, .. } => Some(name),
            _ => None,
        })
        .collect();
    assert_eq!(names.first(), Some(&"model"), "{names:?}");
    assert!(
        names.contains(&"memory") && names.contains(&"mcp"),
        "{names:?}"
    );
    app.reduce(key(KeyCode::Down));
    assert_eq!(app.bottom.popup.as_ref().map(|p| p.cursor), Some(1));
    app.reduce(key(KeyCode::Up));
    let effects = app.reduce(key(KeyCode::Enter));
    assert!(app.bottom.popup.is_none());
    assert_eq!(app.bottom.view_stack.len(), 1, "/model opens its view");
    assert!(app.input.text.is_empty());
    assert!(effects.iter().any(|e| matches!(
        e,
        Effect::Request {
            command: Command::ListModels,
            ..
        }
    )));
}

#[test]
fn finished_login_starts_the_first_agent() {
    let mut app = app_with_status(TuiOptions::default());
    let effects = app.reduce(AppEvent::ExternalFinished {
        label: "login".into(),
        error: None,
    });
    assert!(effects.iter().any(|e| matches!(
        e,
        Effect::Request {
            command: Command::CreateAgent(_),
            ..
        }
    )));
}

#[test]
fn root_creation_waits_for_network_defaults_and_recovers_from_disconnect() {
    use aether_core::agent::NetworkPolicy;
    for network in [NetworkPolicy::Allowed, NetworkPolicy::Denied] {
        let mut app = App::new(TuiOptions {
            cwd: Some("/proj".into()),
            ..Default::default()
        });
        app.reduce(AppEvent::Connected);
        let defaults = app.start_new_root(Some("hello".into()), Some("/proj".into()));
        let Effect::Request {
            command: Command::Status,
            then: then @ Then::RootDefaults { .. },
        } = defaults
        else {
            panic!("{defaults:?}");
        };
        let mut status = crate::ui::tests_support::quiet_status();
        status.network_default = network;
        let effects = app.reduce(AppEvent::Reply {
            then: then.clone(),
            result: Ok(Reply::Status(status)),
        });
        assert!(matches!(&effects[..], [Effect::Request {
                    command: Command::CreateAgent(request), then: Then::AgentCreated(_)
                }] if request.scope.as_ref().map(|scope| scope.network) == Some(network)
                    && request.scope.as_ref().and_then(|scope| scope.workspace_root.as_deref()) == Some(std::path::Path::new("/proj"))
        ));
        app.reduce(AppEvent::Reply {
            then,
            result: Err(ClientError::Disconnected),
        });
        assert!(app.awaited_in_flight().is_none());
        assert!(!app.root_create_attempted);
    }
}

/// Every path that starts a root goes through `create_root`,
/// and a root is always parentless and the coordinator - never a worker
/// role inherited from whatever was selected.
#[test]
fn every_root_creation_path_asks_for_a_parentless_coordinator() {
    let created = |effects: &[Effect]| -> CreateAgent {
        effects
            .iter()
            .find_map(|e| match e {
                Effect::Request {
                    command: Command::CreateAgent(req),
                    ..
                } => Some(req.clone()),
                _ => None,
            })
            .expect("a create request")
    };
    let check = |req: &CreateAgent| {
        assert_eq!(req.parent, None, "a root has no parent");
        assert_eq!(req.role, Role::ASSISTANT);
        assert_eq!(req.name.as_deref(), Some(Role::ASSISTANT));
    };

    // /new
    let mut app = app_with_status(TuiOptions::default());
    check(&created(
        &app.run_command(SlashCommand::New { objective: None }),
    ));

    // The auto-create when the daemon has no live agent.
    let mut app = app_with_status(TuiOptions {
        auto_create_root: true,
        ..Default::default()
    });
    app.reduce(AppEvent::Connected);
    app.status = Some(crate::ui::tests_support::quiet_status());
    check(&created(&app.reduce(AppEvent::Reply {
        then: Then::Tree,
        result: Ok(Reply::Tree(AgentTree::default())),
    })));

    // A first message typed with nothing selected.
    let mut app = app_with_status(TuiOptions::default());
    for c in "do the thing".chars() {
        app.reduce(key(KeyCode::Char(c)));
    }
    check(&created(&app.reduce(key(KeyCode::Enter))));
}

#[test]
fn auto_create_root_when_none_live() {
    let mut app = app_with_status(TuiOptions {
        auto_create_root: true,
        ..Default::default()
    });
    app.reduce(AppEvent::Connected);
    app.status = Some(crate::ui::tests_support::quiet_status());
    let effects = app.reduce(AppEvent::Reply {
        then: Then::Tree,
        result: Ok(Reply::Tree(AgentTree::default())),
    });
    assert!(effects.iter().any(|e| matches!(
        e,
        Effect::Request {
            command: Command::CreateAgent(_),
            ..
        }
    )));
    let id = AgentId::new();
    let effects = app.reduce(AppEvent::Reply {
        then: start_then(&effects),
        result: Ok(Reply::Agent(agent(id, "assistant"))),
    });
    assert_eq!(app.selected, Some(id));
    assert!(
        effects.iter().any(|e| matches!(
            e,
            Effect::Request {
                command: Command::Status,
                then: Then::Status,
            }
        )),
        "a short session must refresh the pre-creation fleet status immediately"
    );
    assert!(effects.iter().any(|e| matches!(
        e,
        Effect::Request {
            then: Then::Tree,
            ..
        }
    )));
}

#[test]
fn a_store_of_only_terminal_agents_is_never_addressed() {
    let mut app = app_with_status(TuiOptions {
        auto_create_root: true,
        ..Default::default()
    });
    app.reduce(AppEvent::Connected);
    app.status = Some(crate::ui::tests_support::quiet_status());
    let done = AgentId::new();
    let mut completed = agent(done, "assistant");
    completed.status = AgentStatus::Completed { summary: None };
    let start = app.reduce(AppEvent::Reply {
        then: Then::Tree,
        result: Ok(Reply::Tree(tree(completed))),
    });
    assert_eq!(app.selected, None, "a completed agent must not be adopted");
    assert!(start.iter().any(|e| matches!(
        e,
        Effect::Request {
            command: Command::CreateAgent(_),
            ..
        }
    )));

    // Typed before the new agent arrives: the message waits for it and
    // starts no second agent.
    app.reduce(AppEvent::Paste("hello".into()));
    let effects = app.reduce(key(KeyCode::Enter));
    assert!(
        effects.is_empty(),
        "the message went somewhere: {effects:?}"
    );

    let fresh = AgentId::new();
    let effects = app.reduce(AppEvent::Reply {
        then: start_then(&start),
        result: Ok(Reply::Agent(agent(fresh, "assistant"))),
    });
    assert_eq!(app.selected, Some(fresh));
    assert!(
        effects.iter().any(|e| matches!(
            e,
            Effect::Request { command: Command::SendMessage { to, content, subtree: false, .. }, .. }
                if *to == fresh && content == "hello"
        )),
        "the queued message did not reach the agent created for it: {effects:?}"
    );
}

#[test]
fn a_message_after_the_agent_completed_starts_a_new_agent() {
    let mut app = app_with_status(TuiOptions::default());
    app.reduce(AppEvent::Connected);
    app.status = Some(crate::ui::tests_support::quiet_status());
    let id = AgentId::new();
    app.reduce(AppEvent::Reply {
        then: Then::Tree,
        result: Ok(Reply::Tree(tree(agent(id, "assistant")))),
    });
    assert_eq!(app.selected, Some(id));

    let mut completed = agent(id, "assistant");
    completed.status = AgentStatus::Completed { summary: None };
    app.reduce(AppEvent::Reply {
        then: Then::Tree,
        result: Ok(Reply::Tree(tree(completed))),
    });
    app.reduce(AppEvent::Paste("hello".into()));
    let effects = app.reduce(key(KeyCode::Enter));
    assert!(
        effects.iter().all(|e| !matches!(
            e,
            Effect::Request {
                command: Command::SendMessage { .. },
                ..
            }
        )),
        "a completed agent was sent a message: {effects:?}"
    );
    assert!(effects.iter().any(|e| matches!(
        e,
        Effect::Request {
            command: Command::CreateAgent(_),
            ..
        }
    )));
}

/// The notice a root creation refused with `code` and `message` leaves.
fn creation_refusal(code: aether_protocol::ErrorCode, message: &str) -> String {
    let mut app = App::new(TuiOptions {
        cwd: Some("/proj".into()),
        ..Default::default()
    });
    app.reduce(AppEvent::Connected);
    let defaults = app.start_new_root(Some("hello".into()), Some("/proj".into()));
    let Effect::Request { then, .. } = defaults else {
        panic!("{defaults:?}");
    };
    let effects = app.reduce(AppEvent::Reply {
        then,
        result: Ok(Reply::Status(crate::ui::tests_support::quiet_status())),
    });
    let [
        Effect::Request {
            then: then @ Then::AgentCreated(_),
            ..
        },
    ] = &effects[..]
    else {
        panic!("{effects:?}");
    };
    app.reduce(AppEvent::Reply {
        then: then.clone(),
        result: Err(ClientError::Protocol(aether_protocol::ErrorReply::new(
            code, message,
        ))),
    });
    app.live_flash().expect("a creation notice").to_owned()
}

/// A signed-in person who named a model that does not exist must see why,
/// not be told to log in: only the daemon's `no_model` code means that.
#[test]
fn only_the_no_model_code_sends_a_failed_creation_to_login() {
    use aether_protocol::ErrorCode;
    for (code, message) in [
        (
            ErrorCode::Unavailable,
            "model `anthropic/claude-nope` not found",
        ),
        (
            ErrorCode::InvalidArgument,
            "model `openai/gpt-6` does not take effort max",
        ),
        (
            ErrorCode::Unavailable,
            "model `anthropic/claude-3-opus` is retired",
        ),
    ] {
        let shown = creation_refusal(code, message);
        assert!(shown.contains(message), "{shown}");
        assert!(!shown.contains("/login"), "{shown}");
    }
    let shown = creation_refusal(
        ErrorCode::NoModel,
        "no model configured; sign in from your client",
    );
    assert!(shown.contains("/login"), "{shown}");
}
