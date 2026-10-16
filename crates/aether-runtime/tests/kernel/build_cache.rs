//! Builds of agents in a repository workspace go to Aether's cache: one
//! partition per repository and run, shared by the run's isolated children.
use super::*;

/// What a child's build sees: whether the cache held an artifact already,
/// then where `CARGO_TARGET_DIR` points. Leaves an artifact behind.
const BUILD: &str = "if [ -e \"$CARGO_TARGET_DIR/artifact\" ]; then printf 'warm:%s|' \"$(cat \"$CARGO_TARGET_DIR/artifact\")\"; else printf 'cold|'; fi; \
     printf 'built by %s' \"$PWD\" > \"$CARGO_TARGET_DIR/artifact\"; printf '%s' \"$CARGO_TARGET_DIR\"";

fn delegate_isolated() -> aether_model::testing::ScriptedResponse {
    ScriptedClient::tool_call(
        "delegate",
        json!({
            "role": "worker", "objective": "build",
            "model": "anthropic/claude-sonnet-5-5", "effort": "low",
            "workspace_mode": "isolated", "wait": true
        }),
    )
}

fn build_then_finish() -> [aether_model::testing::ScriptedResponse; 2] {
    [
        ScriptedClient::tool_call("bash", json!({"command": BUILD})),
        ScriptedClient::tool_call("complete_task", json!({"summary": "built"})),
    ]
}

/// The children of `parent` in creation order.
fn children(h: &Harness, parent: AgentId) -> Vec<aether_core::agent::Agent> {
    let mut children = h
        .store
        .agents(&AgentFilter {
            parent: Some(parent),
            ..Default::default()
        })
        .expect("agents");
    children.sort_by_key(|child| child.created_at);
    children
}

/// What `agent`'s build printed: `cold|` or `warm:<who built it>|`, then the
/// cache directory it was handed.
fn build_output(h: &Harness, agent: AgentId) -> (String, std::path::PathBuf) {
    let events = h
        .store
        .events(0, 10_000, &EventFilter::for_agent(agent))
        .expect("events");
    let outcome = settled_outcome(&events, "bash");
    assert!(!outcome.is_error, "{outcome:?}");
    let (seen, dir) = outcome
        .content
        .lines()
        .next()
        .and_then(|line| line.rsplit_once('|'))
        .unwrap_or_else(|| panic!("unexpected build output: {outcome:?}"));
    (seen.to_owned(), dir.into())
}

async fn cache_harness(
    responses: Vec<aether_model::testing::ScriptedResponse>,
) -> (Harness, tempfile::TempDir, tempfile::TempDir) {
    let managed = tempfile::tempdir().expect("managed worktree directory");
    let cache = tempfile::tempdir().expect("build cache root");
    let (worktrees, root) = (managed.path().to_path_buf(), cache.path().to_path_buf());
    let h = harness_configured(
        responses,
        |_| {},
        |config| {
            config.delegated_workspace_root = Some(worktrees);
            config.build_cache_root = Some(root);
        },
    )
    .await;
    super::workspace_isolation::init_repository(&h.workspace_root);
    (h, managed, cache)
}

#[tokio::test]
async fn isolated_children_of_one_run_build_warm_in_one_shared_partition() {
    let mut script = vec![delegate_isolated()];
    script.extend(build_then_finish());
    script.push(delegate_isolated());
    script.extend(build_then_finish());
    script.push(ScriptedClient::tool_call(
        "complete_task",
        json!({"summary": "parent done"}),
    ));
    let (h, _managed, cache) = cache_harness(script).await;
    let (parent, _) = h
        .runtime
        .create_agent(spec("delegate two builds"), Actor::user())
        .expect("parent");
    h.wait_until(parent.id, |status| status.is_terminal()).await;

    let [first, second] = <[_; 2]>::try_from(children(&h, parent.id)).expect("two children");
    let first_root = first.scope.workspace_root.clone().expect("first worktree");
    let second_root = second
        .scope
        .workspace_root
        .clone()
        .expect("second worktree");
    assert_ne!(first_root, second_root, "each child has its own worktree");
    let (first_seen, first_dir) = build_output(&h, first.id);
    let (second_seen, second_dir) = build_output(&h, second.id);
    assert_eq!(first_seen, "cold", "the run's first build starts cold");
    assert_eq!(first_dir, second_dir, "one partition for the run");
    assert_eq!(
        second_seen,
        format!("warm:built by {}", first_root.display()),
        "the second build reuses the first one's artifact"
    );
    assert!(
        first_dir.starts_with(cache.path()),
        "{}",
        first_dir.display()
    );
    assert!(first_dir.ends_with(format!("{}/cargo-target", parent.run)));
    assert!(!first_dir.starts_with(&first_root) && !first_dir.starts_with(&second_root));
    assert_eq!(h.client.remaining(), 0);
}

#[tokio::test]
async fn children_of_another_run_get_their_own_partition() {
    let mut script = Vec::new();
    for _ in 0..2 {
        script.push(delegate_isolated());
        script.extend(build_then_finish());
        script.push(ScriptedClient::tool_call(
            "complete_task",
            json!({"summary": "root done"}),
        ));
    }
    let (h, _managed, cache) = cache_harness(script).await;
    let mut roots = Vec::new();
    for objective in ["first run", "second run"] {
        let (root, _) = h
            .runtime
            .create_agent(spec(objective), Actor::user())
            .expect("root");
        h.wait_until(root.id, |status| status.is_terminal()).await;
        roots.push(root);
    }
    assert_ne!(roots[0].run, roots[1].run, "two runs");

    let builds: Vec<_> = roots
        .iter()
        .map(|root| {
            let [child] = <[_; 1]>::try_from(children(&h, root.id)).expect("one child");
            assert_eq!(child.run, root.run);
            build_output(&h, child.id)
        })
        .collect();
    assert_eq!(builds[0].0, "cold");
    assert_eq!(
        builds[1].0, "cold",
        "another run never sees this run's output"
    );
    assert_ne!(builds[0].1, builds[1].1);
    assert_eq!(
        builds[0].1.parent().and_then(|p| p.parent()),
        builds[1].1.parent().and_then(|p| p.parent()),
        "both partitions belong to the one repository"
    );
    assert!(builds[1].1.starts_with(cache.path()));
    for (_, dir) in &builds {
        let partition = dir.parent().expect("partition");
        assert!(
            partition.join(".aether-partition").is_file(),
            "Aether marks the partitions it makes: {}",
            partition.display()
        );
    }

    let report = h.runtime.storage_gc(true).expect("storage report");
    let reported = &report["build_cache"];
    assert_eq!(reported["partitions"], 2, "{reported}");
    assert_eq!(reported["repositories"], 1, "{reported}");
    assert!(reported["bytes"].as_u64().unwrap_or(0) > 0, "{reported}");
    assert!(
        report.get("workspaces").is_some(),
        "cache bytes sit beside workspaces, not in them"
    );
}

#[tokio::test]
async fn a_client_named_cache_is_used_and_inherited_by_children() {
    let (h, _managed, cache) = cache_harness(vec![
        ScriptedClient::tool_call(
            "delegate",
            json!({
                "role": "worker", "objective": "build",
                "model": "anthropic/claude-sonnet-5-5", "effort": "low", "wait": true
            }),
        ),
        ScriptedClient::tool_call("bash", json!({"command": BUILD})),
        ScriptedClient::tool_call("complete_task", json!({"summary": "built"})),
        ScriptedClient::tool_call("complete_task", json!({"summary": "parent done"})),
    ])
    .await;
    let named = cache.path().join("items/one");
    let mut asked = spec("build in a named cache");
    asked.config.as_mut().expect("config").build_cache = Some(named.clone());
    let (parent, _) = h
        .runtime
        .create_agent(asked, Actor::user())
        .expect("parent");
    assert_eq!(parent.config.build_cache.as_ref(), Some(&named));
    h.wait_until(parent.id, |status| status.is_terminal()).await;

    let [child] = <[_; 1]>::try_from(children(&h, parent.id)).expect("one child");
    assert_eq!(child.config.build_cache.as_ref(), Some(&named), "inherited");
    let (seen, dir) = build_output(&h, child.id);
    assert_eq!((seen.as_str(), dir), ("cold", named.join("cargo-target")));
    assert!(named.join("cargo-target/artifact").is_file());
}

#[tokio::test]
async fn a_named_cache_must_be_absolute_and_never_comes_from_a_device() {
    let h = harness(vec![]).await;
    for dir in ["relative/cache", "/srv/../cache", "/"] {
        let mut asked = spec("bad cache");
        asked.config.as_mut().expect("config").build_cache = Some(dir.into());
        let refused = h.runtime.create_agent(asked, Actor::user());
        assert!(refused.is_err(), "{dir}");
    }
    let mut asked = spec("device cache");
    asked.config.as_mut().expect("config").build_cache = Some("/srv/cache/items/one".into());
    let device = Actor::connected_user(aether_core::event::OperatorOrigin::Device {
        device: aether_core::id::DeviceId::new(),
        name: "phone".into(),
        connection: "device-test".into(),
    });
    let refused = h.runtime.create_agent(asked, device).expect_err("device");
    assert!(refused.to_string().contains("paired device"), "{refused}");
    assert!(h.store.agents(&AgentFilter::default()).unwrap().is_empty());
}
