use super::{
    AgentId, Client, ColorDepth, ModelChain, TuiDisplayOptions, TuiOptions, UiPrefs,
    connect_or_start,
};
use aether_tui::prefs::MouseMode;
use aether_tui::ui::theme::files::{self, ThemeError};
use aether_tui::ui::theme::{Builtin, ThemeChoice, ThemeSpec, probe};
use anyhow::anyhow;

/// Validate a saved model chain (`ui.toml`) against the daemon's live model
/// list before it reaches `CreateAgent`. A primary that no longer resolves
/// (removed from config.toml, its provider dropped) would otherwise fail
/// every agent creation with a message that never points at `ui.toml` as the
/// cause - fall through to the daemon's own default instead, and say so
/// once. A stale fallback is dropped the same way rather than reaching
/// creation and failing there. Shared by `aether` (TUI) and `aether run`,
/// which both read the same file.
pub(super) async fn validated_model_chain(
    client: &Client,
    saved: Option<ModelChain>,
) -> Option<ModelChain> {
    let chain = saved?;
    let models = match client.list_models().await {
        Ok(models) => models,
        Err(e) => {
            eprintln!(
                "could not check saved model {} against the daemon ({}); using it unvalidated",
                super::sane(&chain.primary.to_string()),
                super::sane(&e.to_string())
            );
            return Some(chain);
        }
    };
    let known: Vec<_> = models.into_iter().map(|m| m.model).collect();
    let primary = chain.primary.clone();
    let fallbacks_before = chain.fallbacks.len();
    match filter_known_models(chain, &known) {
        Some(filtered) => {
            if filtered.fallbacks.len() != fallbacks_before {
                eprintln!(
                    "ignoring stale fallback model(s) in the saved chain · they are no longer available"
                );
            }
            Some(filtered)
        }
        None => {
            eprintln!(
                "ignoring saved model {} · it is no longer available, using the daemon default",
                super::sane(&primary.to_string())
            );
            None
        }
    }
}

/// Keep only the chain entries the daemon can still create. A stale
/// `primary` drops the whole chain (the daemon default takes over); a stale
/// `fallback` is dropped on its own, so it never reaches `create_agent` and
/// fails there instead. Split out from
/// `validated_model_chain` so the decision is testable without a daemon.
pub(super) fn filter_known_models(
    mut chain: ModelChain,
    known: &[aether_core::model::ModelRef],
) -> Option<ModelChain> {
    if !known.contains(&chain.primary) {
        return None;
    }
    chain.fallbacks.retain(|m| known.contains(m));
    Some(chain)
}

/// `mouse` is `--mouse`/`--no-mouse` when either was typed; otherwise the
/// TUI's own default decides.
pub async fn tui(
    socket: impl Into<aether_client::Endpoint>,
    explicit_socket: bool,
    agent: Option<super::AgentRef>,
    mouse: Option<bool>,
    yolo: Option<bool>,
    display: TuiDisplayOptions,
) -> anyhow::Result<()> {
    let socket = socket.into();
    let TuiDisplayOptions {
        theme,
        view_mode,
        reduced_motion,
        plain,
        ascii,
        host_commands,
    } = display;
    let _guard = crate::logging::init("aether.log", false)?;
    // A pipe or a screen reader gets lines whether or not --plain was typed:
    // the full-screen UI is meaningless without a terminal.
    let plain = wants_plain(
        plain,
        std::io::IsTerminal::is_terminal(&std::io::stdout()),
        std::env::var("TERM").ok().as_deref(),
    );
    let update_check = launch_update_check(&socket, plain);
    let theme_flag = theme
        .as_deref()
        .map(|t| {
            t.parse::<ThemeChoice>()
                .map_err(|e| anyhow!("--theme: {e}"))
        })
        .transpose()?;
    validate_startup_display(&socket, ascii)?;
    let client = connect_or_start(&socket).await?;
    if let Some(check) = update_check {
        super::update::offer(check, socket.clone(), explicit_socket).await?;
    }
    let instance_id = client.welcome().map(|w| w.instance_id.clone());
    let definitions_dir = aether_tui::prefs::dir();
    let prefs_dir = aether_tui::prefs::session_dir(
        &socket,
        instance_id.as_deref().unwrap_or("unknown"),
        &definitions_dir,
    );
    // A broken ui.toml must not stop the TUI: say so and paint the default.
    let prefs = match UiPrefs::load(&prefs_dir) {
        Ok(prefs) => prefs,
        Err(e) => {
            eprintln!("ignoring bad preferences: {}", super::sane(&e.to_string()));
            UiPrefs::default()
        }
    };
    let mut view_modes = aether_tui::view_modes::discover(&definitions_dir);
    let (view_mode, notice) = view_modes
        .resolve(
            view_mode.as_deref(),
            std::env::var_os("AETHER_VIEW_MODE").as_deref(),
            prefs.view_mode.as_deref(),
        )
        .map_err(anyhow::Error::msg)?;
    view_modes.diagnostics.extend(notice);
    let flagged = theme_flag.is_some();
    let no_color = std::env::var("NO_COLOR").ok();
    let no_color_theme = aether_tui::prefs::no_color_picked_theme(flagged, no_color.as_deref());
    let theme_choice = prefs.theme(theme_flag, no_color.as_deref());
    // A typed theme that does not exist stops here; a saved one paints the
    // default and says why, like a broken file does.
    let (scheme, theme_error) = match files::scheme(&theme_choice, Some(&definitions_dir)) {
        Ok(scheme) => (scheme, None),
        Err(ThemeError {
            message,
            fallback: None,
        }) if flagged => return Err(anyhow!("--theme: {message}")),
        Err(ThemeError { message, fallback }) => (
            fallback.map(Builtin::scheme).unwrap_or_default(),
            Some(message),
        ),
    };
    let theme_overrides =
        aether_tui::ui::theme::overrides::Overrides::load(&definitions_dir.join("config.toml"))?;
    let theme_spec = ThemeSpec {
        mode: theme_choice.mode,
        depth: ColorDepth::detect(),
        scheme,
        overrides: theme_overrides.hues,
        background_hint: probe::background_of(None, std::env::var("COLORFGBG").ok().as_deref()),
        ..ThemeSpec::default()
    };
    let reduced_motion = prefs.reduced_motion(reduced_motion);
    let ascii_env = ascii_environment()?;
    let ascii = prefs
        .ascii(ascii, ascii_env.as_deref())
        .map_err(anyhow::Error::msg)?;
    // A flag overrides for this run; otherwise what was saved, otherwise the
    // daemon's `[approval] default`.
    let yolo = yolo.or(prefs.yolo);
    // The session default the operator last chose wins over the daemon's
    // configured one; the daemon's is the fallback.
    let saved_chain = validated_model_chain(&client, prefs.model_chain()).await;
    let default_model = match saved_chain.as_ref().map(|c| c.primary.clone()) {
        Some(model) => Some(model),
        None => client.status().await.ok().and_then(|s| s.default_model),
    };
    let focus = match agent {
        Some(agent) => Some(agent.resolve(&client, &socket).await?),
        None => None,
    };
    let options = TuiOptions {
        view_modes,
        view_mode: Some(view_mode),
        keys: prefs.keys.clone(),
        focus,
        default_model,
        default_fallbacks: saved_chain.map(|c| c.fallbacks).unwrap_or_default(),
        recent_models: prefs.recent_models.clone(),
        show_archived: prefs.show_archived.unwrap_or(false),
        auto_create_root: !socket.is_remote(),
        socket_path: socket.local_path().map(std::path::Path::to_path_buf),
        endpoint: Some(socket.clone()),
        instance_id,
        resume_seq: prefs.last_seq,
        draft: if plain { None } else { prefs.draft.clone() },
        explicit_socket,
        mouse: match mouse {
            Some(true) => MouseMode::Scroll,
            Some(false) => MouseMode::Off,
            None => prefs.mouse.unwrap_or_default(),
        },
        yolo,
        cwd: if socket.is_remote() {
            None
        } else {
            std::env::current_dir().ok()
        },
        home: if socket.is_remote() {
            None
        } else {
            aether_tui::workspace::home_dir()
        },
        recent_workspaces: prefs.recent_workspaces.clone(),
        pinned_workspaces: prefs.pinned_workspaces.clone(),
        workspace_roots: prefs.workspace_roots.clone(),
        workspace_depth: prefs.workspace_depth,
        theme_spec,
        theme: theme_choice,
        no_color_theme,
        startup_flash: theme_overrides.ignored_notice(),
        startup_error: theme_error,
        prefs_dir: Some(prefs_dir),
        definitions_dir: Some(definitions_dir),
        reduced_motion,
        ascii,
        board_open: prefs.board_open,
        // A missing or broken file warns in the TUI rather than stopping it.
        host_commands: host_commands
            .as_deref()
            .map(aether_tui::commands::HostCommands::load)
            .unwrap_or_default(),
        // Keys are ids this client wrote; a hand-edited bad key is not an agent to restore to.
        // Plain mode has no queue to show or edit, so it must not send these unseen.
        queued: if plain {
            Default::default()
        } else {
            prefs
                .queued
                .iter()
                .filter_map(|(id, msgs)| Some((id.parse::<AgentId>().ok()?, msgs.clone())))
                .collect()
        },
    };
    if plain {
        return aether_tui::run_plain(client, options).await;
    }
    aether_tui::run(client, options).await
}

/// The launch update check, for a person at a terminal on a local daemon:
/// a remote daemon is not updated by this binary, and a prompt needs a person.
fn launch_update_check(
    socket: &aether_client::Endpoint,
    plain: bool,
) -> Option<super::update::LaunchCheck> {
    let interactive = !plain && std::io::IsTerminal::is_terminal(&std::io::stdin());
    if socket.is_remote() || !interactive {
        return None;
    }
    super::update::LaunchCheck::start(
        &aether_tui::prefs::dir(),
        &aether_protocol::paths::data_dir(),
    )
}

/// A dumb terminal cannot honour the alternate screen, mouse or keyboard modes.
fn wants_plain(requested: bool, stdout_is_terminal: bool, term: Option<&str>) -> bool {
    requested || !stdout_is_terminal || term.is_some_and(|t| t.eq_ignore_ascii_case("dumb"))
}

fn ascii_environment() -> anyhow::Result<Option<String>> {
    match std::env::var("AETHER_ASCII") {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => {
            Err(anyhow!("AETHER_ASCII must contain a UTF-8 boolean value"))
        }
    }
}

/// Whether plain commands print ASCII only: the `--ascii` flag, `AETHER_ASCII`,
/// the saved preference, or a locale that is not UTF-8.
pub(super) fn display_ascii(flag: bool) -> anyhow::Result<bool> {
    let var = |name| std::env::var(name).ok();
    let locale_ascii = super::table::locale_is_ascii(
        var("LC_ALL").as_deref(),
        var("LC_CTYPE").as_deref(),
        var("LANG").as_deref(),
    );
    let prefs = UiPrefs::load(&aether_tui::prefs::dir()).unwrap_or_default();
    let chosen = prefs
        .ascii(flag, ascii_environment()?.as_deref())
        .map_err(anyhow::Error::msg)?;
    Ok(chosen || locale_ascii)
}

fn validate_startup_display(endpoint: &aether_client::Endpoint, ascii: bool) -> anyhow::Result<()> {
    let prefs = if endpoint.is_remote() {
        UiPrefs::default()
    } else {
        match UiPrefs::load(&aether_tui::prefs::dir()) {
            Ok(prefs) => prefs,
            Err(error) => {
                eprintln!(
                    "ignoring bad preferences: {}",
                    super::sane(&error.to_string())
                );
                UiPrefs::default()
            }
        }
    };
    prefs
        .ascii(ascii, ascii_environment()?.as_deref())
        .map_err(anyhow::Error::msg)?;
    Ok(())
}

#[cfg(test)]
mod plain_choice_tests {
    use super::wants_plain;

    #[test]
    fn a_dumb_terminal_gets_plain_mode() {
        assert!(wants_plain(false, true, Some("dumb")));
        assert!(wants_plain(false, true, Some("DUMB")));
    }

    #[test]
    fn a_capable_terminal_keeps_the_full_screen_ui() {
        assert!(!wants_plain(false, true, Some("xterm-256color")));
        assert!(!wants_plain(false, true, None));
    }

    #[test]
    fn a_pipe_or_the_flag_still_forces_plain_mode() {
        assert!(wants_plain(false, false, Some("xterm")));
        assert!(wants_plain(true, true, Some("xterm")));
    }
}
