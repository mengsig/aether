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
    socket: PathBuf,
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
    while let Some((_, subcommand)) = command_matches.subcommand() {
        command_matches = subcommand;
    }
    let json = command_matches
        .try_get_one::<bool>("json")
        .ok()
        .flatten()
        .copied()
        .unwrap_or(false);
    let args = Args::from_arg_matches(&matches).unwrap_or_else(|error| error.exit());
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
    let theme = args.theme.clone();
    let reduced_motion = args.reduced_motion;
    let plain = args.plain;
    let ascii = args.ascii;
    run_with_runtime(async move {
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
                    socket,
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
            Cmd::Retention { command } => match command {
                RetentionCmd::Preview { json } => cli::retention_preview(socket, json).await,
            },
            Cmd::Status { json } => cli::status(socket, json).await,
            Cmd::Archive {
                target,
                all_finished,
                undo,
                json,
            } => cli::archive(socket, target, all_finished, undo, json).await,
            Cmd::Stop { json } => cli::stop(socket, json).await,
            Cmd::Run {
                objective,
                model,
                role,
                effort,
                yes,
                yolo,
                no_yolo,
                backend,
                network,
                json,
            } => {
                return cli::run(
                    socket,
                    explicit_socket,
                    cli::RunOptions {
                        objective: objective.join(" "),
                        model,
                        role,
                        effort,
                        yes,
                        yolo: yolo_flag(yolo, no_yolo),
                        backend,
                        network,
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
            Cmd::Auth { json } => cli::auth(socket, json).await,
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
            Cmd::Models { json } => cli::models(socket, json).await,
            Cmd::Providers { json } => cli::providers(socket, json).await,
            Cmd::Runs { command } => match command {
                RunsCmd::List { limit, json } => cli::runs_list(socket, limit, json).await,
                RunsCmd::Report { run, json, md, out } => {
                    cli::run_report(socket, cli::ReportOptions { run, json, md, out }).await
                }
                RunsCmd::Export { run, out, json } => cli::run_export(socket, run, out, json).await,
                RunsCmd::Import { file, json } => cli::run_import(socket, file, json).await,
            },
            Cmd::Agents { all, json } => cli::agents(socket, all, json).await,
            Cmd::New { name, model, json } => cli::new_agent(socket, name, model, json).await,
            Cmd::Tasks { json } => cli::tasks(socket, json).await,
            Cmd::Task {
                objective,
                owner,
                depends_on,
                json,
            } => cli::task_create(socket, objective.join(" "), owner, depends_on, json).await,
            Cmd::Replay { agent, json } => cli::replay(socket, agent, json).await,
            Cmd::Events {
                follow,
                after,
                agent,
                json,
            } => cli::events(socket, follow, after, agent, json).await,
            Cmd::Send {
                agent,
                text,
                images,
                subtree,
                json,
            } => cli::send(socket, agent, text.join(" "), images, subtree, json).await,
            Cmd::Approve { approval, remember } => {
                approve_with(socket, approval, true, remember).await
            }
            Cmd::Deny { approval, remember } => {
                approve_with(socket, approval, false, remember).await
            }
            Cmd::Artifacts { command } => match command {
                ArtifactsCmd::List {
                    agent,
                    task,
                    with_bytes,
                    limit,
                    json,
                } => cli::artifacts_list(socket, agent, task, with_bytes, limit, json).await,
                ArtifactsCmd::Show {
                    id,
                    max_bytes,
                    json,
                } => cli::artifact_show(socket, id, max_bytes, json).await,
            },
            Cmd::Mcp { command } => match command {
                McpCmd::List { json } => cli::mcp_list(socket, false, None, json).await,
                McpCmd::Refresh { server, json } => cli::mcp_list(socket, true, server, json).await,
                McpCmd::Serve {
                    stdio: _,
                    http,
                    token_env,
                    allow_remote,
                } => cli::mcp_serve(socket, http, token_env, allow_remote).await,
            },
            Cmd::Voice { command } => match command {
                VoiceCmd::Status { json } => cli::voice::status(socket, json).await,
                VoiceCmd::Reply { json } => cli::voice::reply(socket, json).await,
                VoiceCmd::Cancel { json } => cli::voice::cancel(socket, json).await,
            },
            Cmd::Rules { command } => match command {
                RulesCmd::List { all, json } => cli::rules_list(socket, all, json).await,
                RulesCmd::Allow {
                    matcher,
                    scope,
                    note,
                    json,
                } => {
                    cli::rules_add(
                        socket,
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
                        socket,
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
                    cli::rules_revoke(socket, id, reason, json).await
                }
            },
            Cmd::Cancel { agent, json } => cli::cancel(socket, agent, json).await,
            Cmd::Interrupt { agent, json } => cli::interrupt(socket, agent, json).await,
            Cmd::Schedule { command } => cli::schedules::run(socket, command).await,
            Cmd::GithubWatcher { command } => cli::github_watcher::run(socket, command).await,
            Cmd::Trigger {
                agent,
                source,
                payload,
                json,
            } => cli::trigger(socket, agent, source, payload, json).await,
            Cmd::Restart { agent, json } => cli::restart(socket, agent, json).await,
            Cmd::Wake { agent, json } => cli::wake(socket, agent, json).await,
            Cmd::Pause {
                agent,
                subtree,
                json,
            } => cli::pause(socket, agent, subtree, json).await,
            Cmd::Resume {
                agent,
                subtree,
                json,
            } => cli::resume(socket, agent, subtree, json).await,
            Cmd::Set { agent, knobs, json } => cli::set(socket, agent, knobs, json).await,
            Cmd::Compact { agent, json } => cli::compact(socket, agent, json).await,
            Cmd::Memory { command } => match command {
                MemoryCmd::List {
                    scope,
                    text,
                    tag,
                    all,
                    limit,
                    json,
                } => cli::memory_list(socket, scope, text, tag, all, limit, json).await,
                MemoryCmd::Show { id, json } => cli::memory_show(socket, id, json).await,
                MemoryCmd::Forget { id, reason, json } => {
                    cli::memory_forget(socket, id, reason, json).await
                }
                MemoryCmd::Restore { id, json } => cli::memory_restore(socket, id, json).await,
                MemoryCmd::Pin { id, unpin, json } => {
                    cli::memory_pin(socket, id, unpin, json).await
                }
                MemoryCmd::Sync { root, json } => cli::memory_sync(socket, root, json).await,
            },
            Cmd::McpBridge { agent, token } => cli::mcp_bridge(socket, agent, token).await,
        };
        result.map(|()| 0)
    })
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
