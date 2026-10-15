//! `aether`: one binary for the daemon, the TUI and the CLI.
//!
//! `aether` with no arguments starts the daemon if needed and attaches the
//! TUI. Everything else is a subcommand of the same executable.

mod cli;
mod daemon_stop;
mod doctor;
mod logging;

use std::path::PathBuf;

use anyhow::Context as _;
use clap::{CommandFactory, FromArgMatches};

mod args;
use args::*;

/// Answer one approval, keeping the answer when `--remember` is given.
async fn approve_with(
    socket: aether_client::Endpoint,
    approval: String,
    approve: bool,
    args: RememberArgs,
) -> anyhow::Result<()> {
    let scope = cli::RuleScopeArgs::from(args.scope).parse()?;
    if !args.remember && (args.tool_wide || scope.is_some()) {
        anyhow::bail!("--tool-wide and the scope flags only mean something with --remember");
    }
    cli::approve(
        socket,
        approval,
        approve,
        args.remember,
        scope,
        args.tool_wide,
        args.json,
    )
    .await
}

/// The `--yolo` / `--no-yolo` pair as one answer. Neither leaves the choice
/// to the saved preference and the daemon's configured default.
fn yolo_flag(yolo: bool, no_yolo: bool) -> Option<bool> {
    match (yolo, no_yolo) {
        (true, _) => Some(true),
        (_, true) => Some(false),
        _ => None,
    }
}

/// A `--flag` / `--no-flag` pair as a choice, or none when neither was typed.
fn flag_pair(on: bool, off: bool) -> Option<bool> {
    match (on, off) {
        (true, _) => Some(true),
        (_, true) => Some(false),
        _ => None,
    }
}

/// `--mouse` / `--no-mouse` as a choice, or none when neither was typed.
fn mouse_flag(mouse: bool, no_mouse: bool) -> Option<bool> {
    match (mouse, no_mouse) {
        (true, _) => Some(true),
        (_, true) => Some(false),
        _ => None,
    }
}

fn main() -> std::process::ExitCode {
    let matches = Args::command().get_matches();
    let mut command_matches = &matches;
    let mut json = false;
    loop {
        json |= command_matches
            .try_get_one::<bool>("json")
            .ok()
            .flatten()
            .copied()
            .unwrap_or(false);
        let Some((_, subcommand)) = command_matches.subcommand() else {
            break;
        };
        command_matches = subcommand;
    }
    let mut args = Args::from_arg_matches(&matches).unwrap_or_else(|error| error.exit());
    let json = json && !matches!(args.command, Some(Cmd::Bridge { .. }));
    if let Err(error) = cli::remote::check_selectors(&matches) {
        eprintln!("Error: {error}");
        return std::process::ExitCode::from(2);
    }
    if matches.value_source("socket") == Some(clap::parser::ValueSource::CommandLine)
        && matches.value_source("remote") != Some(clap::parser::ValueSource::CommandLine)
    {
        args.remote = None;
    }
    let socket = args
        .socket
        .clone()
        .unwrap_or_else(aether_protocol::paths::socket_path);
    match execute(args, socket.clone()) {
        Ok(code) => u8::try_from(code).map_or(std::process::ExitCode::FAILURE, Into::into),
        Err(error) => {
            if json {
                use std::io::Write;
                if let Err(write_error) = writeln!(
                    std::io::stdout().lock(),
                    "{}",
                    cli::error_json(&error, &socket)
                ) && write_error.kind() != std::io::ErrorKind::BrokenPipe
                {
                    eprintln!("writing JSON error: {write_error}");
                }
            }
            eprintln!("Error: {}", cli::error_message(&error));
            let backtrace = error.backtrace();
            if backtrace.status() == std::backtrace::BacktraceStatus::Captured {
                eprintln!("\nStack backtrace:\n{backtrace}");
            }
            std::process::ExitCode::FAILURE
        }
    }
}

fn execute(args: Args, socket: PathBuf) -> anyhow::Result<i32> {
    let explicit_socket = args.socket.is_some();
    if args.remote.is_some() && !args.local {
        check_remote_command(args.command.as_ref())?;
    }
    if let Some(Cmd::Bridge { stdio }) = args.command.as_ref() {
        let stdio = *stdio;
        return run_with_runtime(
            async move { cli::bridge::bridge(socket, stdio).await.map(|()| 0) },
        );
    }
    let profiles_only = matches!(
        args.command.as_ref(),
        Some(Cmd::Remote {
            command: RemoteCmd::Add { .. } | RemoteCmd::List { .. } | RemoteCmd::Remove { .. }
        })
    );
    let endpoint = if profiles_only {
        aether_client::Endpoint::Socket(socket.clone())
    } else {
        cli::remote::endpoint(socket.clone(), args.remote.as_deref(), args.local)?
    };
    let theme = args.theme.clone();
    let reduced_motion = args.reduced_motion;
    let plain = args.plain;
    let ascii = args.ascii;
    let cancellable = endpoint.is_remote()
        || matches!(
            args.command,
            Some(Cmd::Remote {
                command: RemoteCmd::Add { .. }
            })
        );
    run_with_runtime(remote_action(cancellable, async move {
        let result = match args.command.unwrap_or(Cmd::Tui {
            agent: None,
            no_mouse: false,
            mouse: false,
            yolo: false,
            no_yolo: false,
        }) {
            Cmd::Tui {
                agent,
                no_mouse,
                mouse,
                yolo,
                no_yolo,
            } => {
                cli::tui(
                    endpoint,
                    explicit_socket,
                    agent,
                    mouse_flag(mouse, no_mouse),
                    yolo_flag(yolo, no_yolo),
                    cli::TuiDisplayOptions {
                        view_mode: args.view_mode.clone(),
                        theme,
                        reduced_motion,
                        plain,
                        ascii,
                    },
                )
                .await
            }
            Cmd::Daemon {
                foreground,
                detached,
                workspace,
                max_concurrent,
                yolo,
                allow_remote,
            } => {
                cli::daemon(
                    socket,
                    foreground,
                    detached,
                    workspace,
                    max_concurrent,
                    yolo,
                    allow_remote,
                )
                .await
            }
            Cmd::Serve { allow_remote } => cli::serve(socket, allow_remote).await,
            Cmd::Schema => cli::schema(),
            Cmd::Storage { command, json } => {
                let (gc, dry_run, json) = match command {
                    Some(StorageCmd::Gc {
                        dry_run,
                        json: gc_json,
                    }) => (true, dry_run, json || gc_json),
                    None => (false, true, json),
                };
                cli::storage(endpoint, gc, dry_run, json).await
            }
            Cmd::Retention { command } => match command {
                RetentionCmd::Preview { json } => cli::retention_preview(endpoint, json).await,
            },
            Cmd::Outbox { command } => settle_outbox(endpoint, command).await,
            Cmd::Status { json } => cli::status(endpoint, json).await,
            Cmd::Archive {
                target,
                all_finished,
                undo,
                json,
            } => cli::archive(endpoint, target, all_finished, undo, json).await,
            Cmd::Stop { json } => cli::stop_endpoint(endpoint, json).await,
            Cmd::Run {
                workspace,
                objective,
                model,
                role,
                effort,
                yes,
                yolo,
                no_yolo,
                backend,
                network,
                repository_root,
                json,
            } => {
                return cli::run(
                    endpoint,
                    explicit_socket,
                    cli::RunOptions {
                        workspace,
                        objective: objective.join(" "),
                        model,
                        role,
                        effort,
                        yes,
                        yolo: yolo_flag(yolo, no_yolo),
                        backend,
                        network,
                        repository_root,
                        json,
                    },
                )
                .await;
            }
            Cmd::Login {
                provider,
                api_key,
                method,
            } => cli::login(provider, api_key, method).await,
            Cmd::Logout { provider, json } => cli::logout(provider, json).await,
            Cmd::Auth { json } => cli::auth(endpoint, json).await,
            Cmd::Diagnose {
                out,
                run,
                last,
                json,
            } => cli::diagnose(socket, explicit_socket, out, run, last as usize, json).await,
            Cmd::Doctor { json } => {
                // The report is the answer, so a bad check is an exit code,
                // never an error a caller has to parse out of stderr.
                let code = cli::doctor(socket, explicit_socket, json).await?;
                return Ok(code);
            }
            Cmd::Update {
                check,
                version,
                json,
            } => {
                // `--check` answers with its exit code, as `doctor` does.
                let options = cli::UpdateOptions {
                    check,
                    version,
                    json,
                };
                return cli::update(socket, explicit_socket, options).await;
            }
            Cmd::Models { json } => cli::models(endpoint, json).await,
            Cmd::Providers { json } => cli::providers(endpoint, json).await,
            Cmd::Runs { command } => match command {
                RunsCmd::List { limit, json } => cli::runs_list(endpoint, limit, json).await,
                RunsCmd::Report { run, json, md, out } => {
                    cli::run_report(endpoint, cli::ReportOptions { run, json, md, out }).await
                }
                RunsCmd::Export { run, out, json } => cli::run_export(socket, run, out, json).await,
                RunsCmd::Import { file, json } => cli::run_import(socket, file, json).await,
            },
            Cmd::Agents { all, json } => cli::agents(endpoint, all, json).await,
            Cmd::New { name, model, json } => cli::new_agent(endpoint, name, model, json).await,
            Cmd::Tasks { json } => cli::tasks(endpoint, json).await,
            Cmd::Task {
                objective,
                owner,
                depends_on,
                json,
            } => cli::task_create(endpoint, objective.join(" "), owner, depends_on, json).await,
            Cmd::Replay { agent, json } => cli::replay(endpoint, agent, json).await,
            Cmd::Events {
                follow,
                after,
                agent,
                json,
            } => cli::events(endpoint, follow, after, agent, json).await,
            Cmd::Send {
                agent,
                text,
                images,
                subtree,
                json,
            } => cli::send(endpoint, agent, text.join(" "), images, subtree, json).await,
            Cmd::Approve { approval, remember } => {
                approve_with(endpoint, approval, true, remember).await
            }
            Cmd::Deny { approval, remember } => {
                approve_with(endpoint, approval, false, remember).await
            }
            Cmd::Artifacts { command } => match command {
                ArtifactsCmd::List {
                    agent,
                    task,
                    with_bytes,
                    limit,
                    json,
                } => cli::artifacts_list(endpoint, agent, task, with_bytes, limit, json).await,
                ArtifactsCmd::Show {
                    id,
                    max_bytes,
                    json,
                } => cli::artifact_show(endpoint, id, max_bytes, json).await,
            },
            Cmd::Mcp { command } => match command {
                McpCmd::List { json } => cli::mcp_list(endpoint, false, None, json).await,
                McpCmd::Refresh { server, json } => {
                    cli::mcp_list(endpoint, true, server, json).await
                }
                McpCmd::Serve {
                    stdio: _,
                    http,
                    token_env,
                    allow_remote,
                } => cli::mcp_serve(endpoint, http, token_env, allow_remote).await,
            },
            Cmd::Voice { command } => match command {
                VoiceCmd::Status { json } => cli::voice::status(socket, json).await,
                VoiceCmd::Reply { json } => cli::voice::reply(socket, json).await,
                VoiceCmd::Cancel { json } => cli::voice::cancel(socket, json).await,
            },
            Cmd::Rules { command } => match command {
                RulesCmd::List { all, json } => cli::rules_list(endpoint, all, json).await,
                RulesCmd::Allow {
                    matcher,
                    scope,
                    note,
                    json,
                } => {
                    cli::rules_add(
                        endpoint,
                        true,
                        matcher.tool,
                        matcher.argument,
                        matcher.resource,
                        scope.into(),
                        note,
                        json,
                    )
                    .await
                }
                RulesCmd::Deny {
                    matcher,
                    scope,
                    note,
                    json,
                } => {
                    cli::rules_add(
                        endpoint,
                        false,
                        matcher.tool,
                        matcher.argument,
                        matcher.resource,
                        scope.into(),
                        note,
                        json,
                    )
                    .await
                }
                RulesCmd::Revoke { id, reason, json } => {
                    cli::rules_revoke(endpoint, id, reason, json).await
                }
            },
            Cmd::Cancel { agent, json } => cli::cancel(endpoint, agent, json).await,
            Cmd::Interrupt { agent, json } => cli::interrupt(endpoint, agent, json).await,
            Cmd::Schedule { command } => cli::schedules::run(endpoint, command).await,
            Cmd::GithubWatcher { command } => cli::github_watcher::run(endpoint, command).await,
            Cmd::Trigger {
                agent,
                source,
                payload,
                json,
            } => cli::trigger(endpoint, agent, source, payload, json).await,
            Cmd::Restart { agent, json } => cli::restart(endpoint, agent, json).await,
            Cmd::Wake { agent, json } => cli::wake(endpoint, agent, json).await,
            Cmd::Pause {
                agent,
                subtree,
                json,
            } => cli::pause(endpoint, agent, subtree, json).await,
            Cmd::Resume {
                agent,
                subtree,
                json,
            } => cli::resume(endpoint, agent, subtree, json).await,
            Cmd::Set { agent, knobs, json } => cli::set(endpoint, agent, knobs, json).await,
            Cmd::Compact { agent, json } => cli::compact(endpoint, agent, json).await,
            Cmd::Memory { command } => match command {
                MemoryCmd::Feedback { command } => cli::memory_feedback(endpoint, command).await,
                MemoryCmd::Add { memory, json } => cli::memory_add(endpoint, memory, json).await,
                MemoryCmd::Accept { id, reason, json } => {
                    cli::memory_accept(endpoint, id, reason, json).await
                }
                MemoryCmd::Reject { id, reason, json } => {
                    cli::memory_reject(endpoint, id, reason, json).await
                }
                MemoryCmd::List {
                    scope,
                    text,
                    tag,
                    recorded_by,
                    all,
                    proposed,
                    limit,
                    json,
                } => {
                    let filter = cli::MemoryFilter {
                        scope,
                        text,
                        tag,
                        recorded_by,
                        all,
                        proposed,
                        limit,
                    };
                    cli::memory_list(endpoint, filter, json).await
                }
                MemoryCmd::Show { id, json } => cli::memory_show(endpoint, id, json).await,
                MemoryCmd::Forget { id, reason, json } => {
                    cli::memory_forget(endpoint, id, reason, json).await
                }
                MemoryCmd::Restore { id, json } => cli::memory_restore(endpoint, id, json).await,
                MemoryCmd::Pin { id, unpin, json } => {
                    cli::memory_pin(endpoint, id, unpin, json).await
                }
                MemoryCmd::Sync { root, json } => cli::memory_sync(endpoint, root, json).await,
                MemoryCmd::Alias {
                    alias,
                    root,
                    repository,
                    dry_run,
                    json,
                } => {
                    let target = cli::AliasTarget { root, repository };
                    cli::memory_alias(endpoint, alias, target, dry_run, json).await
                }
                MemoryCmd::Aliases { json } => cli::memory_aliases(endpoint, json).await,
                MemoryCmd::Context {
                    task,
                    invocation,
                    json,
                } => cli::memory_context(endpoint, task, invocation, json).await,
            },
            Cmd::Remote { command } => cli::remote::run(socket, command).await,
            Cmd::Bridge { stdio } => cli::bridge::bridge(socket, stdio).await,
            Cmd::McpBridge { agent, token } => cli::mcp_bridge(socket, agent, token).await,
        };
        result.map(|()| 0)
    }))
}

async fn remote_action(
    cancellable: bool,
    action: impl std::future::Future<Output = anyhow::Result<i32>>,
) -> anyhow::Result<i32> {
    if !cancellable {
        return action.await;
    }
    tokio::select! {
        result = action => result,
        signal = tokio::signal::ctrl_c() => {
            signal.context("waiting for remote command cancellation")?;
            anyhow::bail!("remote command interrupted; delivery unconfirmed for pending actions; check host state before retrying")
        }
    }
}

async fn settle_outbox(
    endpoint: aether_client::Endpoint,
    command: Option<OutboxCmd>,
) -> anyhow::Result<()> {
    use aether_client::Settlement;
    let (key, choice, json) = match command.unwrap_or(OutboxCmd::List { json: false }) {
        OutboxCmd::List { json } => return cli::outbox_list(endpoint, json).await,
        OutboxCmd::Delivered { key, json } => (key, Settlement::MarkDelivered, json),
        OutboxCmd::Resend { key, json } => (key, Settlement::SendAgain, json),
        OutboxCmd::Discard { key, json } => (key, Settlement::Discard, json),
    };
    cli::outbox_settle(endpoint, &key, choice, json).await
}

fn check_remote_command(command: Option<&Cmd>) -> anyhow::Result<()> {
    let feature = match command {
        Some(Cmd::Daemon { .. }) => Some("daemon"),
        Some(Cmd::Serve { .. }) => Some("serve"),
        Some(Cmd::Login { .. }) => Some("provider login"),
        Some(Cmd::Logout { .. }) => Some("provider logout"),
        Some(Cmd::Diagnose { .. } | Cmd::Doctor { .. }) => Some("local diagnostics"),
        Some(Cmd::Update { .. }) => Some("update"),
        Some(Cmd::Bridge { .. } | Cmd::McpBridge { .. }) => Some("bridge"),
        Some(Cmd::Voice { .. }) => Some("voice"),
        Some(Cmd::Runs {
            command: RunsCmd::Export { .. } | RunsCmd::Import { .. },
        }) => Some("export/import of local paths"),
        Some(Cmd::Remote { command })
            if !matches!(
                command,
                RemoteCmd::Add { .. } | RemoteCmd::List { .. } | RemoteCmd::Remove { .. }
            ) =>
        {
            Some("remote administration")
        }
        _ => None,
    };
    if let Some(feature) = feature {
        anyhow::bail!("{feature} is local-only; run `aether --local` on the host to use it");
    }
    Ok(())
}

fn run_with_runtime(
    future: impl std::future::Future<Output = anyhow::Result<i32>>,
) -> anyhow::Result<i32> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("tokio runtime")?;
    let result = runtime.block_on(future);
    // Orderly daemon shutdown and store cleanup finish before bounding leftover blocking work.
    runtime.shutdown_timeout(std::time::Duration::from_secs(2));
    result
}

#[cfg(test)]
#[path = "main_tests.rs"]
mod main_tests;
