//! Attach the prepared backend command to a controlling terminal.
use crate::tool::ToolError;

pub(crate) fn spawn(
    command: &tokio::process::Command,
) -> Result<(tokio::process::Child, pty_process::Pty), ToolError> {
    let (pty, pts) = pty_process::open().map_err(pty_error)?;
    pty.resize(pty_process::Size::new(24, 80))
        .map_err(pty_error)?;
    let source = command.as_std();
    // Copy the explicit backend argv, cwd and environment, including removals.
    // Stdio and process_group are intentionally replaced: PTY spawn establishes
    // a new session/process group and makes the slave its controlling terminal.
    let mut cmd = pty_process::Command::new(source.get_program())
        .args(source.get_args())
        .kill_on_drop(true);
    if let Some(cwd) = source.get_current_dir() {
        cmd = cmd.current_dir(cwd);
    }
    for (key, value) in source.get_envs() {
        cmd = match value {
            Some(value) => cmd.env(key, value),
            None => cmd.env_remove(key),
        };
    }
    let child = aether_spawn::reset_pty_signals(cmd)
        .spawn(pts)
        .map_err(pty_error)?;
    Ok((child, pty))
}

fn pty_error(error: pty_process::Error) -> ToolError {
    ToolError::Internal(format!("terminal setup failed: {error}"))
}
