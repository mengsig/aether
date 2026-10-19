//! Live watcher configuration through the daemon; no client-side config file.

use aether_client::Client;
use aether_core::agent::{AgentConfig, NetworkPolicy};
use aether_core::github::{GithubDispatchMode, GithubRepositoryConfig, GithubWatcherStatus};
use aether_core::id::AgentId;
use aether_core::model::{Effort, ModelChain};
use aether_core::schedule::SpawnSpec;
use anyhow::{Context as _, anyhow, bail};

use super::{acted, connect_or_start, here, parse_backend};
use crate::GithubWatcherCmd;

/// What a thread-dispatch repository's spawned agents get, from `add
/// --spawn-role`/`--spawn-objective`.
struct Template {
    role: String,
    objective: String,
    model: Option<String>,
    effort: Option<String>,
    token_budget: Option<u64>,
    cost_budget: Option<f64>,
    max_turns: Option<u32>,
    backend: Option<String>,
    network: Option<bool>,
}

/// The dispatch change `add`'s flags request: a fixed target, a thread
/// template, or (both absent) automatic for a new repository and unchanged
/// for an existing one.
#[allow(clippy::large_enum_variant)]
enum AddDispatch {
    Fixed(AgentId),
    Thread(SpawnSpec),
    Unchanged,
}

pub async fn run(
    socket: impl Into<aether_client::Endpoint>,
    command: GithubWatcherCmd,
) -> anyhow::Result<()> {
    let socket = socket.into();
    let client = connect_or_start(&socket).await?;
    let current = client.github_watcher().await?;
    let mut config = current.config.clone();
    let json = match command {
        GithubWatcherCmd::Status { json } => return show(json, &current, &socket),
        GithubWatcherCmd::Add {
            repository,
            agent,
            spawn_role,
            spawn_objective,
            spawn_model,
            spawn_effort,
            spawn_token_budget,
            spawn_cost_budget,
            spawn_max_turns,
            spawn_backend,
            spawn_network,
            allowed_users,
            json,
        } => {
            let repository = repository.to_ascii_lowercase();
            let template = match (spawn_role, spawn_objective) {
                (Some(role), Some(objective)) => Some(Template {
                    role,
                    objective,
                    model: spawn_model,
                    effort: spawn_effort,
                    token_budget: spawn_token_budget,
                    cost_budget: spawn_cost_budget,
                    max_turns: spawn_max_turns,
                    backend: spawn_backend,
                    network: spawn_network,
                }),
                (None, None) => None,
                // clap's `requires` ties spawn_role and spawn_objective together.
                _ => unreachable!("clap requires spawn_role and spawn_objective together"),
            };
            let spawn = match template {
                Some(template) => Some(spawn_spec(&client, template).await?),
                None => None,
            };
            let agent = match agent {
                Some(agent) => Some(agent.resolve(&client, &socket).await?),
                None => None,
            };
            let dispatch = add_dispatch(agent, spawn);
            apply_add(&mut config, repository, dispatch, allowed_users)?;
            json
        }
        GithubWatcherCmd::AllowedUsers {
            repository,
            logins,
            clear,
            json,
        } => {
            let users = (!clear).then_some(logins);
            set_allowed_users(&mut config, &repository, users)?;
            json
        }
        GithubWatcherCmd::OperatorUsers {
            logins,
            clear,
            json,
        } => {
            set_operator_users(&mut config, (!clear).then_some(logins));
            json
        }
        GithubWatcherCmd::Remove { repository, json } => {
            let before = config.repositories.len();
            config
                .repositories
                .retain(|r| !r.repository.eq_ignore_ascii_case(&repository));
            if config.repositories.len() == before {
                bail!("repository {repository} is not configured");
            }
            json
        }
        GithubWatcherCmd::Enable { repository, json } => {
            set_enabled(&mut config, repository.as_deref(), true)?;
            json
        }
        GithubWatcherCmd::Disable { repository, json } => {
            set_enabled(&mut config, repository.as_deref(), false)?;
            json
        }
        GithubWatcherCmd::Configure {
            mention_regex,
            label_regex,
            poll_interval_secs,
            json,
        } => {
            if let Some(expression) = mention_regex {
                config.mention_regex = expression;
            }
            if let Some(expression) = label_regex {
                config.label_regex = expression;
            }
            if let Some(seconds) = poll_interval_secs {
                config.poll_interval_secs = seconds;
            }
            json
        }
    };
    let updated = client
        .configure_github_watcher(config, current.revision)
        .await?;
    show(json, &updated, &socket)
}

/// Resolves `add`'s flags into a dispatch decision. `--agent` and
/// `--spawn-role`/`--spawn-objective` are mutually exclusive at the clap
/// level, so exactly one of `agent`/`spawn` is `Some` unless both are absent.
fn add_dispatch(agent: Option<AgentId>, spawn: Option<SpawnSpec>) -> AddDispatch {
    match (agent, spawn) {
        (Some(agent), None) => AddDispatch::Fixed(agent),
        (None, Some(template)) => AddDispatch::Thread(template),
        (None, None) => AddDispatch::Unchanged,
        (Some(_), Some(_)) => {
            unreachable!("clap conflicts_with forbids --agent with --spawn-role")
        }
    }
}

/// Adds a new repository or updates an existing one's dispatch mode and, when
/// given, its allowlist. Adding a new repository starts the watcher so a
/// mention is enough to trigger work. Re-selecting an existing repository
/// keeps the global switch.
fn apply_add(
    config: &mut aether_core::github::GithubWatcherConfig,
    repository: String,
    dispatch: AddDispatch,
    allowed_users: Option<Vec<String>>,
) -> anyhow::Result<()> {
    debug_assert!(!repository.is_empty(), "caller lowercases a non-empty name");
    match config
        .repositories
        .iter_mut()
        .find(|r| r.repository.eq_ignore_ascii_case(&repository))
    {
        Some(entry) => {
            match dispatch {
                AddDispatch::Fixed(agent) => {
                    entry.agent = Some(agent);
                    entry.dispatch = GithubDispatchMode::Fixed;
                }
                AddDispatch::Thread(template) => {
                    entry.agent = None;
                    entry.dispatch = GithubDispatchMode::Thread(template);
                }
                AddDispatch::Unchanged => {}
            }
            if allowed_users.is_some() {
                entry.allowed_users = allowed_users;
            }
            entry.enabled = true;
        }
        None => {
            let (agent, mode) = match dispatch {
                AddDispatch::Fixed(agent) => (Some(agent), GithubDispatchMode::Fixed),
                AddDispatch::Thread(template) => (None, GithubDispatchMode::Thread(template)),
                AddDispatch::Unchanged => (None, GithubDispatchMode::Automatic),
            };
            debug_assert!(
                matches!(mode, GithubDispatchMode::Fixed) == agent.is_some(),
                "only Fixed dispatch carries an agent id"
            );
            config.repositories.push(GithubRepositoryConfig {
                repository,
                enabled: true,
                agent,
                dispatch: mode,
                allowed_users,
            });
            config.enabled = true;
        }
    }
    Ok(())
}

/// Builds a thread-dispatch template the same way `schedule create
/// --objective` builds a spawn schedule's template.
async fn spawn_spec(client: &Client, template: Template) -> anyhow::Result<SpawnSpec> {
    if template.role.trim().is_empty() {
        bail!("--spawn-role must not be empty");
    }
    if template.objective.trim().is_empty() {
        bail!("--spawn-objective must not be empty");
    }
    let chain = template
        .model
        .map(|m| ModelChain::parse(&m).ok_or_else(super::model_format_error))
        .transpose()?;
    let effort = template
        .effort
        .map(|e| e.parse::<Effort>().map_err(|e| anyhow!(e)))
        .transpose()?;
    let backend = parse_backend(template.backend.as_deref().unwrap_or("host"))?;
    let network = match template.network {
        Some(true) => NetworkPolicy::Allowed,
        Some(false) => NetworkPolicy::Denied,
        None => client.status().await?.network_default,
    };
    let config = AgentConfig {
        effort,
        token_budget: template.token_budget,
        cost_budget: template.cost_budget,
        max_turns: template.max_turns,
        ..Default::default()
    };
    let scope = here(backend, network)?;
    let spec = SpawnSpec {
        role: template.role,
        name: None,
        objective: template.objective,
        model: chain.as_ref().map(|c| c.primary.clone()),
        fallbacks: chain.map(|c| c.fallbacks).unwrap_or_default(),
        scope: Some(scope),
        config: (config != AgentConfig::default()).then_some(config),
        approval: None,
    };
    debug_assert!(!spec.role.trim().is_empty(), "checked above");
    debug_assert!(spec.scope.is_some(), "just set to Some above");
    Ok(spec)
}

/// Replaces (`Some`) or clears (`None`) one repository's allowlist. The
/// daemon validates and normalizes the logins; nothing else changes.
fn set_allowed_users(
    config: &mut aether_core::github::GithubWatcherConfig,
    repository: &str,
    users: Option<Vec<String>>,
) -> anyhow::Result<()> {
    debug_assert!(
        users.as_ref().is_none_or(|users| !users.is_empty()),
        "clap requires at least one login unless --clear"
    );
    let entry = config
        .repositories
        .iter_mut()
        .find(|r| r.repository.eq_ignore_ascii_case(repository))
        .with_context(|| format!("repository {repository} is not configured"))?;
    entry.allowed_users = users;
    Ok(())
}

/// Replaces (`Some`) or clears (`None`) the logins whose requests carry
/// operator authority. The daemon validates and normalizes them.
fn set_operator_users(
    config: &mut aether_core::github::GithubWatcherConfig,
    users: Option<Vec<String>>,
) {
    debug_assert!(
        users.as_ref().is_none_or(|users| !users.is_empty()),
        "clap requires at least one login unless --clear"
    );
    config.operator_users = users;
}

fn set_enabled(
    config: &mut aether_core::github::GithubWatcherConfig,
    repository: Option<&str>,
    enabled: bool,
) -> anyhow::Result<()> {
    if let Some(repository) = repository {
        let entry = config
            .repositories
            .iter_mut()
            .find(|r| r.repository.eq_ignore_ascii_case(repository))
            .with_context(|| format!("repository {repository} is not configured"))?;
        entry.enabled = enabled;
    } else {
        config.enabled = enabled;
    }
    Ok(())
}

fn show(
    json: bool,
    status: &GithubWatcherStatus,
    socket: &aether_client::Endpoint,
) -> anyhow::Result<()> {
    acted(
        json,
        &describe(status, &super::command_hint(socket, false)),
        serde_json::to_value(status)?,
    )
}

/// Renders a repository's dispatch mode without assuming `Fixed` names a
/// live (or even present) agent: display must never panic on stale config.
fn dispatch_summary(repository: &GithubRepositoryConfig) -> String {
    match &repository.dispatch {
        GithubDispatchMode::Fixed => match repository.agent {
            Some(agent) => agent.to_string(),
            None => "(no agent configured)".into(),
        },
        GithubDispatchMode::Automatic => "Aether agent per issue/PR".into(),
        GithubDispatchMode::Thread(template) => {
            format!("spawn per thread (role={})", template.role)
        }
    }
}

/// Who may dispatch. The allowlist only narrows, so both forms name the
/// write-access requirement that always applies.
fn requesters_summary(repository: &GithubRepositoryConfig) -> String {
    match &repository.allowed_users {
        None => "any collaborator with write access".into(),
        Some(users) => format!("{} (write access still required)", users.join(", ")),
    }
}

/// Whose requests speak as the operator. Both forms say the list grants
/// nothing by itself: the other checks always apply.
fn operators_summary(config: &aether_core::github::GithubWatcherConfig) -> String {
    match &config.operator_users {
        None => "none (every GitHub request is external content)".into(),
        Some(users) => format!(
            "{} (operator authority for their own text; write access still required)",
            users.join(", ")
        ),
    }
}

fn describe(status: &GithubWatcherStatus, command: &str) -> String {
    let config = &status.config;
    let mut lines = vec![
        format!(
            "GitHub watcher: {} · every {}s · revision {}",
            if config.enabled {
                "enabled"
            } else {
                "disabled"
            },
            config.poll_interval_secs,
            status.revision
        ),
        format!("Mention regex: {}", config.mention_regex),
        format!("Label regex: {}", config.label_regex),
        format!("Operator logins: {}", operators_summary(config)),
    ];
    if config.repositories.is_empty() {
        lines.push(format!(
            "No repositories selected. Use `{command} github-watcher add OWNER/REPO`."
        ));
    }
    for repository in &config.repositories {
        lines.push(format!(
            "{} [{}] → {}",
            repository.repository,
            if repository.enabled {
                "selected"
            } else {
                "disabled"
            },
            dispatch_summary(repository)
        ));
        lines.push(format!("  requesters: {}", requesters_summary(repository)));
        if let Some(health) = status
            .repositories
            .iter()
            .find(|r| r.repository == repository.repository)
        {
            lines.push(format!(
                "  dispatched: {} · ignored: {} · last success: {:?}",
                health.dispatched, health.ignored, health.last_success_at
            ));
            if let Some(error) = &health.last_error {
                lines.push(format!("  ERROR: {error}"));
            }
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_core::github::GithubWatcherConfig;
    use aether_core::schedule::SpawnSpec;

    fn fixed_repo(enabled: bool) -> GithubRepositoryConfig {
        GithubRepositoryConfig {
            repository: "owner/repo".into(),
            agent: Some(AgentId::new()),
            dispatch: GithubDispatchMode::Fixed,
            allowed_users: None,
            enabled,
        }
    }

    fn template() -> SpawnSpec {
        SpawnSpec {
            role: "worker".into(),
            name: None,
            objective: "Handle this thread.".into(),
            model: None,
            fallbacks: Vec::new(),
            scope: None,
            config: None,
            approval: None,
        }
    }

    #[test]
    fn selecting_a_repository_does_not_enable_global_polling() {
        let mut config = GithubWatcherConfig::default();
        config.repositories.push(fixed_repo(false));
        set_enabled(&mut config, Some("OWNER/REPO"), true).expect("configured repository");
        assert!(config.repositories[0].enabled);
        assert!(!config.enabled);
        assert!(set_enabled(&mut config, Some("missing/repo"), true).is_err());
        set_enabled(&mut config, None, true).expect("global switch");
        assert!(config.enabled);
    }

    #[test]
    fn add_dispatch_takes_an_agent_a_template_or_neither() {
        let template = template();
        assert!(matches!(add_dispatch(None, None), AddDispatch::Unchanged));
        assert!(matches!(
            add_dispatch(Some(AgentId::new()), None),
            AddDispatch::Fixed(_)
        ));
        assert!(matches!(
            add_dispatch(None, Some(template)),
            AddDispatch::Thread(_)
        ));
    }

    #[test]
    fn adding_a_new_repository_starts_automatic_dispatch() {
        let mut config = GithubWatcherConfig::default();
        apply_add(
            &mut config,
            "owner/repo".into(),
            AddDispatch::Unchanged,
            None,
        )
        .unwrap();
        assert!(config.enabled);
        assert_eq!(
            config.repositories[0],
            GithubRepositoryConfig::automatic("owner/repo".into())
        );
    }

    #[test]
    fn adding_a_new_repository_with_thread_dispatch_stores_the_template_and_no_agent() {
        let mut config = GithubWatcherConfig::default();
        apply_add(
            &mut config,
            "owner/repo".into(),
            AddDispatch::Thread(template()),
            None,
        )
        .expect("adds repository");
        let entry = &config.repositories[0];
        assert!(entry.agent.is_none());
        assert!(matches!(entry.dispatch, GithubDispatchMode::Thread(_)));
    }

    #[test]
    fn updating_an_existing_repository_without_new_flags_keeps_its_dispatch_mode() {
        let mut config = GithubWatcherConfig::default();
        config.repositories.push(fixed_repo(false));
        let agent = config.repositories[0].agent;
        apply_add(
            &mut config,
            "OWNER/REPO".into(),
            AddDispatch::Unchanged,
            None,
        )
        .expect("updates");
        assert!(config.repositories[0].enabled);
        assert_eq!(config.repositories[0].agent, agent);
        assert!(!config.enabled);
    }

    #[test]
    fn dispatch_summary_never_panics_on_a_fixed_repository_missing_its_agent() {
        let mut repo = fixed_repo(true);
        repo.agent = None;
        assert!(dispatch_summary(&repo).contains("no agent"));
        assert!(
            dispatch_summary(&GithubRepositoryConfig {
                dispatch: GithubDispatchMode::Thread(template()),
                ..fixed_repo(true)
            })
            .contains("spawn per thread")
        );
    }

    #[test]
    fn add_sets_an_allowlist_and_an_update_without_one_keeps_it() {
        let mut config = GithubWatcherConfig::default();
        let users = vec!["octoCat".to_owned(), "monaLisa".to_owned()];
        apply_add(
            &mut config,
            "owner/repo".into(),
            AddDispatch::Unchanged,
            Some(users.clone()),
        )
        .unwrap();
        assert_eq!(config.repositories[0].allowed_users, Some(users.clone()));
        apply_add(
            &mut config,
            "owner/repo".into(),
            AddDispatch::Unchanged,
            None,
        )
        .unwrap();
        assert_eq!(config.repositories[0].allowed_users, Some(users));
        let replaced = vec!["octocat".to_owned()];
        apply_add(
            &mut config,
            "OWNER/REPO".into(),
            AddDispatch::Unchanged,
            Some(replaced.clone()),
        )
        .unwrap();
        assert_eq!(config.repositories[0].allowed_users, Some(replaced));
    }

    #[test]
    fn allowed_users_sets_and_clears_without_touching_enablement() {
        let mut config = GithubWatcherConfig::default();
        config.repositories.push(fixed_repo(false));
        set_allowed_users(&mut config, "OWNER/REPO", Some(vec!["octoCat".into()])).unwrap();
        assert_eq!(
            config.repositories[0].allowed_users,
            Some(vec!["octoCat".into()])
        );
        assert!(!config.repositories[0].enabled);
        assert!(requesters_summary(&config.repositories[0]).contains("octoCat"));
        set_allowed_users(&mut config, "owner/repo", None).unwrap();
        assert_eq!(config.repositories[0].allowed_users, None);
        assert!(requesters_summary(&config.repositories[0]).contains("any collaborator"));
        assert!(set_allowed_users(&mut config, "missing/repo", None).is_err());
    }

    #[test]
    fn operator_users_set_and_clear_and_status_names_them() {
        let mut config = GithubWatcherConfig::default();
        config.repositories.push(fixed_repo(false));
        let untouched = config.repositories.clone();
        assert!(
            describe(&GithubWatcherStatus::default(), "aether")
                .contains("Operator logins: none (every GitHub request is external content)")
        );
        set_operator_users(&mut config, Some(vec!["mengsig".into(), "octocat".into()]));
        assert_eq!(
            config.operator_users,
            Some(vec!["mengsig".into(), "octocat".into()])
        );
        assert_eq!(config.repositories, untouched);
        assert!(!config.enabled, "naming operators starts no polling");
        let status = GithubWatcherStatus {
            config: config.clone(),
            ..Default::default()
        };
        let text = describe(&status, "aether");
        assert!(
            text.contains(
                "Operator logins: mengsig, octocat (operator authority for their own text; \
                 write access still required)"
            ),
            "{text}"
        );
        set_operator_users(&mut config, None);
        assert_eq!(config.operator_users, None);
    }

    #[test]
    fn status_names_the_allowlist_of_each_repository() {
        let mut narrowed = fixed_repo(true);
        narrowed.allowed_users = Some(vec!["octocat".into(), "monalisa".into()]);
        let status = GithubWatcherStatus {
            config: GithubWatcherConfig {
                repositories: vec![narrowed],
                ..Default::default()
            },
            ..Default::default()
        };
        let text = describe(&status, "aether");
        assert!(
            text.contains("requesters: octocat, monalisa (write access still required)"),
            "{text}"
        );
    }
}
