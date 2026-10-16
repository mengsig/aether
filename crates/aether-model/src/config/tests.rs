#[test]
fn new_agent_network_defaults_are_configurable_without_changing_scope_defaults() {
    use aether_core::agent::NetworkPolicy;
    let default: super::AetherConfig = toml::from_str("").expect("defaults");
    assert_eq!(default.network.default, NetworkPolicy::Allowed);
    let template: super::AetherConfig = toml::from_str(super::CONFIG_TEMPLATE).expect("template");
    assert_eq!(template.network.default, NetworkPolicy::Allowed);
    let denied: super::AetherConfig = toml::from_str(
        r#"[network]
default = "denied"
"#,
    )
    .expect("configured denial");
    assert_eq!(denied.network.default, NetworkPolicy::Denied);
    assert_eq!(
        aether_core::agent::Scope::default().network,
        NetworkPolicy::Denied
    );
    assert!(
        toml::from_str::<super::AetherConfig>(
            r#"[network]
default = "unknown"
"#
        )
        .is_err()
    );
}

use super::*;
use crate::profile::{FamilyProfiles, ModelFamily};

/// The three accepted shapes of `[model] profiles`, plus a refusal: a
/// misspelled family must be an error, not a silently ignored key that
/// leaves the operator thinking they switched something.
#[test]
fn the_profiles_setting_takes_a_bool_or_a_family_table() {
    let load = |toml: &str| {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("config.toml"), toml).expect("write");
        load_config(dir.path()).map(|c| c.model.profiles)
    };
    assert_eq!(
        load("[model]\nprofiles = true\n").expect("bool"),
        ProfileSetting::Every(true)
    );
    assert_eq!(
        load("[model]\nprofiles = false\n").expect("bool"),
        ProfileSetting::Every(false)
    );
    assert_eq!(
        load("[model]\nprofiles = { claude = true, gpt = false }\n").expect("table"),
        ProfileSetting::PerFamily(FamilyProfiles {
            claude: Some(true),
            gpt: Some(false),
            codex: None,
        })
    );
    assert!(
        load("[model]\nprofiles = { clade = true }\n").is_err(),
        "a misspelled family must be refused, not ignored"
    );
}

/// Unwritten, "5m", and "1h" all parse; anything else is refused rather
/// than silently ignored.
#[test]
fn prompt_cache_ttl_is_opt_in_and_rejects_unknown_values() {
    use aether_core::model::PromptCacheTtl;
    let load = |toml: &str| {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("config.toml"), toml).expect("write");
        load_config(dir.path()).map(|c| c.model.prompt_cache_ttl)
    };
    let dir = tempfile::tempdir().expect("tempdir");
    assert_eq!(
        load_config(dir.path())
            .expect("load")
            .model
            .prompt_cache_ttl,
        None,
        "unset config preserves native cache settings"
    );
    assert_eq!(
        load("[model]\nprompt_cache_ttl = \"5m\"\n").expect("5m"),
        Some(PromptCacheTtl::FiveMinutes)
    );
    assert_eq!(
        load("[model]\nprompt_cache_ttl = \"1h\"\n").expect("1h"),
        Some(PromptCacheTtl::OneHour)
    );
    assert!(
        load("[model]\nprompt_cache_ttl = \"10m\"\n").is_err(),
        "an unsupported tier must be refused, not silently ignored"
    );
}

/// An empty config leaves every family at the default its eval earned.
#[test]
fn an_unwritten_profiles_setting_is_the_recorded_defaults() {
    let dir = tempfile::tempdir().expect("tempdir");
    let profiles = load_config(dir.path()).expect("load").model.profiles;
    for family in ModelFamily::ALL {
        assert_eq!(
            profiles.enabled(*family),
            family.on_by_default(),
            "{family:?}"
        );
    }
}

#[test]
fn the_documented_web_search_and_artifacts_sections_parse() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("config.toml"),
        r#"
[artifacts]
retain_days = 7
retain_bytes = 1024

[web.search]
endpoint = "https://api.example.com/res/v1/web/search"
results_pointer = "/web/results"
api_key_env = "BRAVE_API_KEY"
api_key_header = "X-Subscription-Token"
"#,
    )
    .expect("write");
    let config = load_config(dir.path()).expect("load");
    assert_eq!(config.artifacts.retain_days, 7);
    assert_eq!(config.artifacts.retain_bytes, 1024);
    let search = config.web.search.expect("the section is read");
    assert_eq!(search.endpoint, "https://api.example.com/res/v1/web/search");
    assert_eq!(search.results_pointer, "/web/results");
    assert_eq!(search.api_key_env.as_deref(), Some("BRAVE_API_KEY"));
    assert_eq!(search.api_key_header, "X-Subscription-Token");
    // The fields a provider usually leaves out fall back.
    assert_eq!(search.query_param, "q");
    assert_eq!(search.title_field, "title");
    assert_eq!(search.url_field, "url");
    assert_eq!(search.snippet_field, "description");
}

#[test]
fn zero_means_unbounded_for_both_retention_limits() {
    // Documented in README and ARCHITECTURE 12.5, and the daemon turns
    // each 0 into "no limit" rather than "keep nothing".
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("config.toml"),
        "[artifacts]
retain_days = 0
retain_bytes = 0
",
    )
    .expect("write");
    let artifacts = load_config(dir.path()).expect("load").artifacts;
    assert_eq!(artifacts.retain_days, 0);
    assert_eq!(artifacts.retain_bytes, 0);
    // Absent, the defaults are the documented 30 days and 2 GiB.
    let empty = tempfile::tempdir().expect("tempdir");
    let defaults = load_config(empty.path()).expect("load").artifacts;
    assert_eq!(defaults.retain_days, 30);
    assert_eq!(defaults.retain_bytes, 2 << 30);
}

#[test]
fn a_search_section_without_an_endpoint_is_refused_not_ignored() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("config.toml"),
        "[web.search]
results_pointer = \"/r\"\n",
    )
    .expect("write");
    assert!(
        load_config(dir.path()).is_err(),
        "a half-written provider must not silently disable search"
    );
}

#[test]
fn loads_default_and_parses() {
    let dir = tempfile::tempdir().expect("tempdir");
    assert_eq!(
        load_config(dir.path()).expect("load"),
        AetherConfig::default()
    );
    std::fs::write(
        dir.path().join("config.toml"),
        "[model]\ndefault = \"anthropic/opus\"\n",
    )
    .expect("write");
    assert_eq!(
        load_config(dir.path())
            .expect("load")
            .model
            .default
            .as_deref(),
        Some("anthropic/opus")
    );
    std::fs::write(dir.path().join("config.toml"), "not toml [").expect("write");
    assert!(load_config(dir.path()).is_err());
}

#[test]
fn global_concurrency_is_configurable_and_never_zero() {
    assert_eq!(SchedulerSection::default().concurrency().unwrap(), 32);
    let dir = tempfile::tempdir().unwrap();
    for value in [1, 16, 64] {
        std::fs::write(
            dir.path().join("config.toml"),
            format!("[scheduler]\nmax_concurrent_turns = {value}\n"),
        )
        .unwrap();
        assert_eq!(
            load_config(dir.path())
                .unwrap()
                .scheduler
                .concurrency()
                .unwrap(),
            value
        );
    }
    std::fs::write(
        dir.path().join("config.toml"),
        "[scheduler]\nmax_concurrent_turns = 0\n",
    )
    .unwrap();
    assert!(
        load_config(dir.path())
            .unwrap_err()
            .reason
            .contains("max_concurrent_turns")
    );
}

#[test]
fn a_wait_timeout_that_cannot_mean_what_it_says_is_reported() {
    let dir = tempfile::tempdir().expect("tempdir");
    let write = |body: &str| {
        std::fs::write(dir.path().join("config.toml"), body).expect("write");
        load_config(dir.path())
    };
    assert_eq!(
        write("[scheduler]\n")
            .unwrap()
            .scheduler
            .wait_timeout()
            .unwrap(),
        None,
        "idle agents must not acquire an implicit periodic wake"
    );
    let err = write("[scheduler]\nwait_timeout_minutes = 0\n")
        .expect_err("0 plainly means something, and it is not the default");
    assert!(err.reason.contains("wait_timeout_minutes"), "{err}");
    assert!(write("[scheduler]\nwait_timeout_minutes = 999999999999\n").is_err());
    assert_eq!(
        write("[scheduler]\nwait_timeout_minutes = 5\n")
            .expect("load")
            .scheduler
            .wait_timeout()
            .expect("valid"),
        Some(std::time::Duration::from_secs(300))
    );
}

#[test]
fn an_engine_transient_retry_window_is_bounded_and_may_be_zero() {
    let dir = tempfile::tempdir().expect("tempdir");
    let write = |body: &str| {
        std::fs::write(dir.path().join("config.toml"), body).expect("write");
        load_config(dir.path())
    };
    let err = write("[scheduler]\nengine_transient_retry_minutes = 99999999\n")
        .expect_err("past a week the agent is hung, not resilient");
    assert!(
        err.reason.contains("engine_transient_retry_minutes"),
        "{err}"
    );
    for (body, window) in [
        ("[scheduler]\nengine_transient_retry_minutes = 0\n", Some(0)),
        (
            "[scheduler]\nengine_transient_retry_minutes = 240\n",
            Some(240 * 60),
        ),
        ("[scheduler]\n", None),
    ] {
        assert_eq!(
            write(body)
                .expect("load")
                .scheduler
                .engine_transient_retry()
                .expect("valid"),
            window.map(std::time::Duration::from_secs),
            "{body}"
        );
    }
}

/// The knob is operator-facing (README, `docs/engines.md`), so a value
/// that cannot mean what it says has to be reported at load, not
/// silently replaced by the default.
#[test]
fn an_engine_park_grace_that_cannot_mean_what_it_says_is_reported() {
    let dir = tempfile::tempdir().expect("tempdir");
    let write = |body: &str| {
        std::fs::write(dir.path().join("config.toml"), body).expect("write");
        load_config(dir.path())
    };
    let err = write("[scheduler]\nengine_park_grace_secs = 0\n")
        .expect_err("0 would kill the engine before it could report anything");
    assert!(err.reason.contains("engine_park_grace_secs"), "{err}");
    assert!(write("[scheduler]\nengine_park_grace_secs = 99999\n").is_err());
    assert_eq!(
        write("[scheduler]\nengine_park_grace_secs = 45\n")
            .expect("load")
            .scheduler
            .engine_park_grace()
            .expect("valid"),
        Some(std::time::Duration::from_secs(45))
    );
    assert_eq!(
        write("[scheduler]\n")
            .expect("load")
            .scheduler
            .engine_park_grace()
            .expect("valid"),
        None,
        "unset leaves the runtime default in charge"
    );
}

/// Both knobs are operator-facing and both have a meaningful "unset";
/// a value that cannot mean what it says must be reported at load, not
/// silently replaced.
#[test]
fn the_engine_cap_and_run_budget_are_validated_at_load() {
    let dir = tempfile::tempdir().expect("tempdir");
    let write = |body: &str| {
        std::fs::write(dir.path().join("config.toml"), body).expect("write");
        load_config(dir.path())
    };
    let err = write("[scheduler]\nmax_concurrent_engine_turns = 0\n")
        .expect_err("0 would admit no engine turn at all");
    assert!(err.reason.contains("max_concurrent_engine_turns"), "{err}");
    assert_eq!(
        write("[scheduler]\nmax_concurrent_engine_turns = 4\n")
            .expect("load")
            .scheduler
            .engine_concurrency()
            .expect("valid"),
        Some(4)
    );
    assert_eq!(
        write("[scheduler]\n")
            .expect("load")
            .scheduler
            .engine_concurrency()
            .expect("valid"),
        None,
        "unset means no engine cap, today's behavior"
    );

    for bad in ["0.0", "-1.5", "nan"] {
        let err = write(&format!("[scheduler]\nrun_budget_usd = {bad}\n"))
            .expect_err("a budget that is not positive dollars cannot be enforced");
        assert!(err.reason.contains("run_budget_usd"), "{bad}: {err}");
    }
    assert_eq!(
        write("[scheduler]\nrun_budget_usd = 12.5\n")
            .expect("load")
            .scheduler
            .run_budget()
            .expect("valid"),
        Some(12.5)
    );
    assert_eq!(
        write("[scheduler]\n")
            .expect("load")
            .scheduler
            .run_budget()
            .expect("valid"),
        None
    );
}

#[test]
fn the_disk_floor_defaults_on_and_zero_turns_it_off() {
    use aether_core::disk::DiskFloor;
    let dir = tempfile::tempdir().expect("tempdir");
    let floor = |body: &str| {
        std::fs::write(dir.path().join("config.toml"), body).expect("write");
        load_config(dir.path()).map(|config| config.scheduler.disk_floor().expect("validated"))
    };
    assert_eq!(
        floor("[scheduler]\n").expect("load"),
        Some(DiskFloor::Default)
    );
    assert_eq!(
        floor("[scheduler]\nmin_free_disk_mib = 0\n").expect("load"),
        None
    );
    assert_eq!(
        floor("[scheduler]\nmin_free_disk_mib = 2048\n").expect("load"),
        Some(DiskFloor::Bytes(2 << 30))
    );
    let err = floor(&format!("[scheduler]\nmin_free_disk_mib = {}\n", i64::MAX))
        .expect_err("a floor past u64 bytes cannot mean what it says");
    assert!(err.reason.contains("min_free_disk_mib"), "{err}");
}

/// `[scheduler]` is a set of cost guards; a misspelled key must be
/// refused at load, not silently ignored while capping nothing.
#[test]
fn a_misspelled_scheduler_key_is_refused_at_load() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("config.toml"),
        "[scheduler]\nmax_concurrent_engine_turn = 4\n",
    )
    .expect("write");
    let err = load_config(dir.path()).expect_err("typo'd key must not load clean");
    assert!(err.reason.contains("max_concurrent_engine_turn"), "{err}");
}

#[test]
fn templates_are_written_once_and_parse() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("aether");
    let written = ensure_templates(&root).expect("write templates");
    assert_eq!(written.len(), 2);
    assert_eq!(
        load_config(&root).expect("template config"),
        AetherConfig::default()
    );
    let providers = crate::registry::load_providers_file(&root.join("providers.toml"))
        .expect("template providers")
        .expect("file exists");
    assert!(
        providers.providers.is_empty(),
        "template must not define providers"
    );

    std::fs::write(root.join("config.toml"), "[model]\ndefault = \"x/y\"\n").expect("write");
    assert!(ensure_templates(&root).expect("second run").is_empty());
    assert_eq!(
        load_config(&root).expect("kept").model.default.as_deref(),
        Some("x/y"),
        "existing files are never overwritten"
    );
}

#[test]
fn a_subscription_must_name_a_signing_secret() {
    let mut serve = ServeSection {
        subscriptions: vec![SubscriptionSection {
            url: "https://example.com/hook".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let refused = serve.address().expect_err("no secret_env");
    assert!(refused.contains("secret_env"), "{refused}");
    serve.subscriptions[0].secret_env = Some("  ".into());
    assert!(serve.address().is_err(), "a blank name is not a name");
    serve.subscriptions[0].secret_env = Some("OUT_SECRET".into());
    serve.address().expect("a named secret is all it needed");
}

/// The clients skip their keepalive below three seconds, so a shorter
/// timeout would drop every one of them on a timer.
#[test]
fn the_idle_timeout_floor_matches_the_floor_the_clients_keep_alive_at() {
    let mut listen = ListenSection {
        address: Some("127.0.0.1:7777".into()),
        ..Default::default()
    };
    for too_short in 0..MIN_IDLE_TIMEOUT_SECS {
        listen.idle_timeout_secs = too_short;
        let refused = listen.address().expect_err("below the floor");
        assert!(refused.contains("keepalive"), "{refused}");
    }
    listen.idle_timeout_secs = MIN_IDLE_TIMEOUT_SECS;
    listen.address().expect("the floor itself is allowed");
}

#[test]
fn two_mcp_servers_cannot_name_the_same_tools() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("config.toml"),
        "[[mcp.servers]]\nname = \"my-server\"\ncommand = \"a\"\n\n\
         [[mcp.servers]]\nname = \"my_server\"\ncommand = \"b\"\n",
    )
    .expect("write");
    let error = load_config(dir.path()).expect_err("names collapse to one prefix");
    assert!(error.reason.contains("same tools"), "{error}");

    std::fs::write(
        dir.path().join("config.toml"),
        "[[mcp.servers]]\nname = \"files\"\ncommand = \"a\"\nurl = \"http://x\"\n",
    )
    .expect("write");
    let error = load_config(dir.path()).expect_err("two transports");
    assert!(error.reason.contains("pick one"), "{error}");
}

#[test]
fn caller_meta_is_sent_only_to_a_server_that_opts_in() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("config.toml"),
        "[[mcp.servers]]\nname = \"existing\"\ncommand = \"a\"\n\n\
         [[mcp.servers]]\nname = \"aware\"\ncommand = \"b\"\nsend_caller_meta = true\n",
    )
    .expect("write");
    let config = load_config(dir.path()).expect("config");
    let servers = &config.mcp.servers;
    assert!(
        !servers[0].send_caller_meta,
        "an existing config keeps sending exactly what it sent"
    );
    assert!(servers[1].send_caller_meta);
}

#[test]
fn the_collision_check_refuses_exactly_what_would_collide() {
    // The check and the name the registry builds must agree: `Files` and
    // `files` produce distinct prefixes, so refusing them was over-strict.
    let section = McpSection {
        servers: vec![
            McpServerSection {
                name: "Files".into(),
                command: Some("a".into()),
                ..server()
            },
            McpServerSection {
                name: "files".into(),
                command: Some("b".into()),
                ..server()
            },
        ],
    };
    assert_ne!(
        aether_core::mcp::qualified_name("Files", "read"),
        aether_core::mcp::qualified_name("files", "read")
    );
    assert!(section.check_names_are_distinct().is_ok());

    let colliding = McpSection {
        servers: vec![
            McpServerSection {
                name: "my.files".into(),
                command: Some("a".into()),
                ..server()
            },
            McpServerSection {
                name: "my_files".into(),
                command: Some("b".into()),
                ..server()
            },
        ],
    };
    assert_eq!(
        aether_core::mcp::qualified_name("my.files", "read"),
        aether_core::mcp::qualified_name("my_files", "read")
    );
    assert!(colliding.check_names_are_distinct().is_err());
}

#[test]
fn a_reserved_name_and_a_cleartext_url_are_refused_at_load() {
    // `aether` is how Aether's own tools reach an engine, so a server
    // with that name would have its calls recorded as theirs.
    let reserved = McpServerSection {
        name: "aether".into(),
        command: Some("a".into()),
        ..server()
    };
    let e = reserved.validate().expect_err("reserved");
    assert!(e.contains("reserved"), "{e}");

    let remote_http = McpServerSection {
        name: "notes".into(),
        url: Some("http://notes.example/mcp".into()),
        token_env: Some("NOTES_TOKEN".into()),
        ..server()
    };
    let e = remote_http
        .validate()
        .expect_err("cleartext to a remote host");
    assert!(e.contains("cleartext"), "{e}");

    for allowed in [
        "https://notes.example/mcp",
        "http://127.0.0.1:9000/mcp",
        "http://localhost:9000/mcp",
        "http://[::1]:9000/mcp",
    ] {
        let ok = McpServerSection {
            name: "notes".into(),
            url: Some(allowed.into()),
            ..server()
        };
        assert!(ok.validate().is_ok(), "{allowed} should be served");
    }

    let odd = McpServerSection {
        name: "notes".into(),
        url: Some("ftp://notes.example/mcp".into()),
        ..server()
    };
    assert!(odd.validate().is_err(), "only http(s) is spoken");
}

#[test]
fn an_optional_token_needs_a_token_variable_and_defaults_to_required() {
    let parsed: McpServerSection =
        toml::from_str("name = \"notes\"\nurl = \"https://notes.example/mcp\"\ntoken_env = \"T\"")
            .expect("parses");
    assert_eq!(parsed.auth, McpAuthMode::Required);
    let optional: McpServerSection = toml::from_str(
        "name = \"notes\"\nurl = \"https://notes.example/mcp\"\ntoken_env = \"T\"\nauth = \"optional\"",
    )
    .expect("parses");
    assert_eq!(optional.auth, McpAuthMode::Optional);
    assert!(optional.validate().is_ok());

    let without_variable = McpServerSection {
        token_env: None,
        ..optional
    };
    let e = without_variable
        .validate()
        .expect_err("nothing to be optional about");
    assert!(e.contains("token_env"), "{e}");

    let empty = McpServerSection {
        token_env: Some(" ".into()),
        ..without_variable
    };
    let e = empty.validate().expect_err("an empty variable name");
    assert!(e.contains("empty"), "{e}");
}

/// An otherwise empty server section, for tests that vary one field.
fn server() -> McpServerSection {
    McpServerSection {
        name: String::new(),
        command: None,
        args: Vec::new(),
        env: BTreeMap::new(),
        cwd: None,
        url: None,
        token_env: None,
        auth: McpAuthMode::Required,
        classes: BTreeMap::new(),
        send_caller_meta: false,
    }
}

#[test]
fn providers_example_is_valid() {
    let file: crate::registry::ProvidersFile =
        toml::from_str(PROVIDERS_EXAMPLE).expect("example parses");
    let mut registry = crate::registry::Registry::builtin();
    registry.apply_overrides(file).expect("example applies");
    let local = registry.provider("local-llm").expect("example provider");
    assert_eq!(local.models.len(), 1);
    assert_eq!(local.models[0].capabilities.context_window, 65_536);
    // Absent, this model would be refused as metered on a subscription
    // fallback the moment someone follows this example verbatim.
    assert_eq!(
        local.models[0].capabilities.cost,
        Some(aether_core::model::ModelCost::default())
    );
}

#[test]
fn the_providers_template_carries_each_example_commented_out() {
    let template = providers_template();
    assert!(
        template
            .lines()
            .all(|line| line.is_empty() || line.starts_with('#')),
        "the template must define nothing until a line is uncommented"
    );
    let uncommented: String = template
        .lines()
        .map(|line| line.strip_prefix("# ").unwrap_or(""))
        .flat_map(|line| [line, "\n"])
        .collect();
    for example in [PROVIDERS_EXAMPLE, PROVIDERS_CLAUDE_1M_EXAMPLE] {
        assert!(uncommented.contains(example), "missing:\n{example}");
    }
}

#[test]
fn every_owned_config_section_rejects_unknown_keys() {
    for section in [
        "model",
        "approval",
        "network",
        "scheduler",
        "memory",
        "mcp",
        "artifacts",
        "web",
        "web.search",
        "listen",
        "remote",
        "serve",
        "retention",
    ] {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            format!("[{section}]\ndefualt = \"typo\"\n"),
        )
        .unwrap();
        let error = load_config(dir.path()).expect_err(section);
        assert!(
            error.reason.contains("unknown field `defualt`"),
            "{section}: {error}"
        );
        assert!(error.reason.contains(section), "{section}: {error}");
    }
}

#[test]
fn theme_settings_remain_owned_by_the_tui() {
    let config: AetherConfig =
        toml::from_str("[model]\ndefault = \"openai/sol\"\n[theme]\naccent = \"#112233\"\n")
            .unwrap();
    assert_eq!(config.model.default.as_deref(), Some("openai/sol"));
}

#[test]
fn nested_config_entries_reject_unknown_keys() {
    for (section, fields) in [
        ("mcp.servers", "name = \"notes\"\ncommand = \"notes-mcp\""),
        (
            "serve.webhook",
            "path = \"/hook\"\nagent = \"root\"\nsource = \"hook\"\nsecret_env = \"HOOK_SECRET\"",
        ),
        (
            "serve.subscription",
            "url = \"https://example.com/events\"\nsecret_env = \"HOOK_SECRET\"",
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            format!("[[{section}]]\n{fields}\ndefualt = true\n"),
        )
        .unwrap();
        let error = load_config(dir.path()).unwrap_err();
        assert!(
            error.reason.contains("unknown field `defualt`"),
            "{section}: {error}"
        );
    }
}

#[test]
fn root_typos_name_the_key_and_valid_sections_while_theme_is_allowed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    for key in ["netwrok", "aproval"] {
        std::fs::write(&path, format!("[{key}]\ndefault = 'allowed'\n")).unwrap();
        let error = super::load_config(dir.path()).unwrap_err().to_string();
        for expected in [
            key,
            "theme",
            "model",
            "approval",
            "network",
            "scheduler",
            "web",
            "artifacts",
            "retention",
            "mcp",
            "memory",
            "listen",
            "remote",
            "serve",
        ] {
            assert!(error.contains(expected), "{error}");
        }
    }
    std::fs::write(&path, "[theme.text]\nfg = '#ffffff'\n").unwrap();
    assert!(super::load_config(dir.path()).is_ok());
}

#[cfg(unix)]
#[test]
fn config_directory_creation_is_private_under_shared_umask() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("tempdir");
    let config = dir.path().join("aether");
    assert!(!config.exists());
    let status = std::process::Command::new("/bin/sh")
        .args(["-c", "umask 002; exec \"$1\" --exact config::tests::config_directory_umask_child --nocapture", "test"])
        .arg(std::env::current_exe().expect("test binary"))
        .env("AETHER_TEST_PRIVATE_CONFIG", &config)
        .status().expect("child test");
    assert!(status.success());
    assert!(config.join("config.toml").is_file());
    assert_eq!(
        std::fs::metadata(&config).unwrap().permissions().mode() & 0o777,
        0o700
    );
    std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o775)).unwrap();
    super::create_config_dir(&config).expect("existing config");
    assert_eq!(
        std::fs::metadata(&config).unwrap().permissions().mode() & 0o777,
        0o775
    );
}

#[cfg(unix)]
#[test]
fn config_directory_umask_child() {
    use std::os::unix::fs::PermissionsExt;
    let Some(path) = std::env::var_os("AETHER_TEST_PRIVATE_CONFIG") else {
        return;
    };
    let path = std::path::PathBuf::from(path);
    let written = super::ensure_templates(&path).expect("templates");
    assert_eq!(written.len(), 2);
    assert_eq!(
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o700
    );
}
