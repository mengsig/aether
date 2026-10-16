//! Execution backends: where a shell command actually runs.
//!
//! The backend is part of an agent's scope, so it is a durable decision a
//! child can narrow but never widen. Both backends produce a plain
//! [`tokio::process::Command`]; capture, timeouts and cancellation stay in
//! one place in the shell tool, and only the shape of the process differs.
//!
//! Before a command is built it is checked against the scope: an agent that
//! owns named paths cannot use git to throw away working-tree changes outside
//! them (see [`crate::git_guard`]).
//!
//! The container backend mounts the workspace at the same path it has on the
//! host, so every path an agent has already seen still means what it meant.
//! Anything else in scope is mounted read-only, and the network follows the
//! scope's policy.
//! Extra mount trees are checked for sockets and devices before preparation;
//! live host changes during or after inspection can still expose them.

use std::process::Stdio;
use std::time::Duration;

use aether_core::agent::{ExecutionBackend, NetworkPolicy};

use crate::tool::{ToolContext, ToolError};

/// Resource ceilings for a container. A pid cap alone leaves memory and CPU
/// free to exhaust, which is the same denial of service one level up.
const CONTAINER_MEMORY: &str = "2g";
const CONTAINER_CPUS: &str = "2";

/// The label every container Aether starts carries, so an orphan left by a
/// daemon that died mid-call can be found and killed.
pub const AGENT_LABEL: &str = "aether.agent";

/// The daemon instance that started the container. Two daemons on one
/// machine are a supported shape, so a sweep filters on this as well: each
/// reaps only what it, or a predecessor on its own data directory, left.
pub const INSTANCE_LABEL: &str = "aether.instance";

const ORPHAN_SWEEP_TIMEOUT: Duration = Duration::from_secs(10);

/// A command built for a backend, plus whatever must be ended besides its
/// process group.
#[derive(Debug)]
pub struct PreparedCommand {
    pub command: tokio::process::Command,
    pub teardown: Option<Teardown>,
}

/// Cleanup a killed process group does not do on its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Teardown {
    /// `docker run` attaches rather than owns: killing the client leaves the
    /// container running, so it is killed by name.
    Container { name: String },
}

impl Teardown {
    /// Never blocking: the caller may be inside `Drop`. Reaped through the
    /// runtime, so a daemon that runs thousands of calls leaves no zombies.
    /// `--rm` removes the container once it stops.
    pub fn run(&self) {
        let Self::Container { name } = self;
        let name = name.clone();
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            // No runtime to reap for us: wait, rather than leak the child.
            Self::kill_blocking(&name);
            return;
        };
        handle.spawn(async move {
            let killed = tokio::process::Command::new("docker")
                .args(["kill", &name])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(false)
                .status()
                .await;
            if let Err(e) = killed {
                tracing::error!(container = %name, error = %e, "cannot kill container");
            }
        });
    }

    fn kill_blocking(name: &str) {
        match std::process::Command::new("docker")
            .args(["kill", name])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
        {
            Ok(_) => {}
            Err(e) => tracing::error!(container = %name, error = %e, "cannot kill container"),
        }
    }
}

/// Build the process that runs `command` under this agent's backend.
pub fn prepare(command: &str, ctx: &ToolContext) -> Result<PreparedCommand, ToolError> {
    prepare_interactive(command, ctx, false)
}

pub(crate) fn prepare_interactive(
    command: &str,
    ctx: &ToolContext,
    terminal: bool,
) -> Result<PreparedCommand, ToolError> {
    // The scope bounds the file tools; a shell would otherwise be the way
    // around it. Only the git commands that throw a sibling's uncommitted
    // work away are refused - see `git_guard` for what it cannot see.
    if let Some(reason) = crate::git_guard::refusal(command, &ctx.cwd, &ctx.scope.allowed_paths) {
        return Err(ToolError::Denied(reason));
    }
    match &ctx.scope.backend {
        ExecutionBackend::Host => Ok(PreparedCommand {
            command: host_command(command, ctx)?,
            teardown: None,
        }),
        ExecutionBackend::Container { image } => container_command(command, image, ctx, terminal),
    }
}

fn host_command(command: &str, ctx: &ToolContext) -> Result<tokio::process::Command, ToolError> {
    let mut cmd = tokio::process::Command::new("bash");
    cmd.arg("-lc")
        .arg(command)
        .current_dir(&ctx.cwd)
        .envs(&ctx.env);
    pipes(&mut cmd, ctx);
    if let Some(dir) = &ctx.temp_dir {
        aether_core::agent_temp::ensure_leaf(dir, &ctx.instance_id)
            .map_err(|e| ToolError::Internal(format!("agent temp dir: {e}")))?;
        for name in aether_core::agent_temp::ENV_VARS {
            cmd.env(name, dir);
        }
    }
    export_build_env(&mut cmd, &ctx.build_env)?;
    Ok(cmd)
}

/// Point the build cache variables at their directories, created now so a
/// tool that only reads the variable still finds one.
fn export_build_env(
    cmd: &mut tokio::process::Command,
    build_env: &[(String, std::path::PathBuf)],
) -> Result<(), ToolError> {
    use std::os::unix::fs::DirBuilderExt as _;

    for (name, dir) in build_env {
        debug_assert!(dir.is_absolute(), "{} is not absolute", dir.display());
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(|e| {
                ToolError::Internal(format!("build cache {name} {}: {e}", dir.display()))
            })?;
        cmd.env(name, dir);
    }
    Ok(())
}

fn container_command(
    command: &str,
    image: &str,
    ctx: &ToolContext,
    terminal: bool,
) -> Result<PreparedCommand, ToolError> {
    let workspace = ctx.scope.workspace_root.as_deref().unwrap_or(&ctx.cwd);
    let workspace = workspace.to_str().ok_or_else(|| {
        ToolError::Internal(format!(
            "workspace path is not valid UTF-8: {}",
            workspace.display()
        ))
    })?;
    let cwd = ctx.cwd.to_str().ok_or_else(|| {
        ToolError::Internal(format!(
            "working directory is not valid UTF-8: {}",
            ctx.cwd.display()
        ))
    })?;
    // The same check the extra roots below get: these become a `-v` mount
    // spec and a `-w` argument docker's own parser reads, so a colon would
    // be read as a different mount and a leading `-` as a flag.
    mountable("workspace root", workspace)?;
    mountable("working directory", cwd)?;
    // An image is an argv element docker's own parser reads: one starting
    // with `-` would become a flag, and a flag could widen the sandbox the
    // scope is meant to narrow.
    if image.starts_with('-') || image.trim().is_empty() {
        return Err(ToolError::Denied(format!(
            "`{image}` is not a container image"
        )));
    }
    let name = container_name(ctx);
    let mut cmd = tokio::process::Command::new("docker");
    cmd.args(["run", "--rm", "--init"])
        // The image is not ours; drop what the workload cannot need, so a
        // setuid binary inside it cannot climb back to root.
        .args(["--cap-drop", "ALL"])
        .args(["--security-opt", "no-new-privileges"])
        .args(["--pids-limit", "512"])
        .args(["--memory", CONTAINER_MEMORY])
        .args(["--cpus", CONTAINER_CPUS])
        // Nothing outside the mounts is meant to survive the call, and /tmp
        // is where a shell expects to be able to write.
        .arg("--read-only")
        .args(["--tmpfs", "/tmp:rw,nosuid,nodev,size=256m"])
        .args(["--name", &name])
        .args(["--label", &format!("{AGENT_LABEL}={}", ctx.agent_id)])
        .args(["--label", &format!("{INSTANCE_LABEL}={}", ctx.instance_id)])
        // Files the agent writes belong to whoever runs the daemon, not root.
        .args([
            "--user",
            &format!("{}:{}", nix::unistd::getuid(), nix::unistd::getgid()),
        ])
        .args(["-v", &format!("{workspace}:{workspace}")])
        .args(["-w", cwd]);
    for extra in &ctx.scope.allowed_paths {
        let Some(path) = extra.to_str() else {
            return Err(ToolError::Internal(format!(
                "allowed path is not valid UTF-8: {}",
                extra.display()
            )));
        };
        mountable("allowed path", path)?;
        validate_extra_mount(extra)?;
        // Read-only: outside the workspace the agent may look, not change.
        cmd.args(["-v", &format!("{path}:{path}:ro")]);
    }
    if ctx.scope.network == NetworkPolicy::Denied {
        cmd.args(["--network", "none"]);
    }
    for (key, value) in &ctx.env {
        // `pipes()` strips these from the docker *client's* environment; a
        // hidden name here would travel into the container as an argument.
        if ctx.hidden_env.iter().any(|hidden| hidden == key) {
            continue;
        }
        cmd.args(["-e", &format!("{key}={value}")]);
    }
    if terminal {
        cmd.arg("-it");
    }
    cmd.arg(image).args(["bash", "-lc", command]);
    pipes(&mut cmd, ctx);
    Ok(PreparedCommand {
        command: cmd,
        teardown: Some(Teardown::Container { name }),
    })
}

fn validate_extra_mount(root: &std::path::Path) -> Result<(), ToolError> {
    use std::os::unix::fs::FileTypeExt as _;

    let inspection_error = |path: &std::path::Path, error: std::io::Error| {
        ToolError::Denied(format!(
            "cannot inspect allowed mount `{}`: {error}",
            path.display()
        ))
    };
    // Docker follows the mount root, but symlinks beneath it resolve inside
    // the container. Following those on the host would scan unrelated trees.
    let metadata = std::fs::metadata(root).map_err(|error| inspection_error(root, error))?;
    let mut pending = vec![(root.to_path_buf(), metadata.file_type())];
    while let Some((path, kind)) = pending.pop() {
        if kind.is_socket() || kind.is_char_device() || kind.is_block_device() {
            return Err(ToolError::Denied(format!(
                "allowed path `{}` is a socket or device and cannot be mounted: \
                 read-only does not contain one",
                path.display()
            )));
        }
        if !kind.is_dir() {
            continue;
        }
        let entries = std::fs::read_dir(&path).map_err(|error| inspection_error(&path, error))?;
        for entry in entries {
            let entry = entry.map_err(|error| inspection_error(&path, error))?;
            let child = entry.path();
            let kind = entry
                .file_type()
                .map_err(|error| inspection_error(&child, error))?;
            if !kind.is_file() && !kind.is_symlink() {
                pending.push((child, kind));
            }
        }
    }
    Ok(())
}

/// Kill containers a previous daemon on *this* instance left running.
///
/// `docker run --rm` removes a container when it stops, but a daemon that
/// died between `docker run` and its teardown leaves one running with the
/// agent's workspace mounted read-write and nothing to reap it. The
/// `aether.agent` label exists exactly so those can be found.
///
/// Scoped by `aether.instance` as well, because two daemons on one machine
/// are a supported shape: without it, starting the second kills the first's
/// in-flight container calls, which surface as failed tool calls in a run
/// that has nothing to do with it. The instance id is the store's, stable
/// across restarts on one data directory, so a daemon still finds its own
/// predecessors.
pub async fn sweep_orphan_containers(instance_id: &str) {
    // Docker may target an unavailable remote service. Bound the whole sweep,
    // including kills, so recovery cannot wait forever before publishing its socket.
    if tokio::time::timeout(
        ORPHAN_SWEEP_TIMEOUT,
        sweep_orphan_containers_inner(instance_id),
    )
    .await
    .is_err()
    {
        tracing::warn!(
            instance = %instance_id,
            timeout_secs = ORPHAN_SWEEP_TIMEOUT.as_secs(),
            "orphan container sweep timed out; some containers may still be running"
        );
    }
}

async fn sweep_orphan_containers_inner(instance_id: &str) {
    let agents = format!("label={}", AGENT_LABEL);
    let ours = format!("label={}={instance_id}", INSTANCE_LABEL);
    let listed = tokio::process::Command::new("docker")
        .args(["ps", "--quiet", "--filter", &agents, "--filter", &ours])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .output()
        .await;
    let listed = match listed {
        Ok(output) if output.status.success() => output.stdout,
        // No docker, or docker refused: there is nothing this daemon can do
        // about containers it cannot see, and it is not a reason not to start.
        Ok(_) => return,
        Err(e) => {
            tracing::debug!(error = %e, "no docker to sweep orphaned containers with");
            return;
        }
    };
    let orphans: Vec<String> = String::from_utf8_lossy(&listed)
        .lines()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .collect();
    if orphans.is_empty() {
        return;
    }
    tracing::warn!(
        count = orphans.len(),
        instance = %instance_id,
        "killing containers left by an earlier daemon on this instance"
    );
    for id in orphans {
        let killed = tokio::process::Command::new("docker")
            .args(["kill", &id])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .status()
            .await;
        if let Err(e) = killed {
            tracing::error!(container = %id, error = %e, "cannot kill an orphaned container");
        }
    }
}

/// Refuse a path docker's argv parser would read as something other than the
/// path. `:` separates the fields of a mount spec, and a leading `-` is a
/// flag - either would widen or redirect what the scope granted.
fn mountable(what: &str, path: &str) -> Result<(), ToolError> {
    if path.contains(':') {
        return Err(ToolError::Denied(format!(
            "{what} `{path}` cannot be mounted: it contains a colon"
        )));
    }
    if path.starts_with('-') {
        return Err(ToolError::Denied(format!(
            "{what} `{path}` cannot be mounted: it would read as a docker flag"
        )));
    }
    Ok(())
}

/// A name unique to this call, so teardown can never reach another's
/// container. Docker names allow `[a-zA-Z0-9][a-zA-Z0-9_.-]*`.
fn container_name(ctx: &ToolContext) -> String {
    format!(
        "aether-{}-{}",
        ctx.agent_id,
        aether_core::id::ProcessId::new()
    )
    .chars()
    .map(|c| {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
            c
        } else {
            '-'
        }
    })
    .collect()
}

fn pipes(cmd: &mut tokio::process::Command, ctx: &ToolContext) {
    // A command an agent chose must not be able to read the daemon's secrets.
    for name in ctx.hidden_env.iter() {
        cmd.env_remove(name);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .process_group(0);
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use super::*;
    use crate::testing::ctx_in;

    /// What the process would be, as a readable line.
    fn rendered(cmd: &tokio::process::Command) -> String {
        let std = cmd.as_std();
        let mut text = std.get_program().to_string_lossy().into_owned();
        for arg in std.get_args() {
            text.push(' ');
            text.push_str(&arg.to_string_lossy());
        }
        text
    }

    #[test]
    fn the_host_backend_runs_bash_in_the_workspace() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ctx = ctx_in(dir.path());
        let prepared = prepare("echo hi", &ctx).expect("prepare");
        assert!(prepared.teardown.is_none());
        assert_eq!(rendered(&prepared.command), "bash -lc echo hi");
    }

    /// `ctx` with its agent's temp dir under a fresh root in `dir`.
    fn with_temp_dir(dir: &std::path::Path) -> ToolContext {
        let mut ctx = ctx_in(dir);
        let root = dir.join("t");
        aether_core::agent_temp::prepare_root(&root, &ctx.instance_id).expect("root");
        ctx.temp_dir = Some(aether_core::agent_temp::leaf_path(&root, ctx.agent_id));
        ctx
    }

    #[tokio::test]
    async fn host_commands_get_the_agent_temp_dir_and_it_is_recreated() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ctx = with_temp_dir(dir.path());
        let leaf = ctx.temp_dir.clone().expect("temp dir");
        let script = r#"printf '%s|%s|%s' "$TMPDIR" "$TMP" "$TEMP" && touch "$TMPDIR/x""#;
        let expected = format!("{0}|{0}|{0}", leaf.display());
        for round in ["created", "recreated"] {
            let output = prepare(script, &ctx)
                .expect("prepare")
                .command
                .output()
                .await
                .expect("run");
            assert!(output.status.success(), "{round}: {output:?}");
            assert_eq!(String::from_utf8_lossy(&output.stdout), expected, "{round}");
            assert!(leaf.join("x").exists(), "{round}");
            std::fs::remove_dir_all(&leaf).expect("delete");
        }
    }

    #[tokio::test]
    async fn host_commands_build_into_the_cache_and_containers_do_not() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cache = dir.path().join("cache/repo/run/cargo-target");
        let mut ctx = ctx_in(dir.path());
        ctx.build_env = vec![("CARGO_TARGET_DIR".into(), cache.clone())];
        let output = prepare(
            r#"printf '%s' "$CARGO_TARGET_DIR" && touch "$CARGO_TARGET_DIR/x""#,
            &ctx,
        )
        .expect("prepare")
        .command
        .output()
        .await
        .expect("run");
        assert!(output.status.success(), "{output:?}");
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            cache.display().to_string()
        );
        assert!(cache.join("x").is_file());

        std::fs::remove_dir_all(dir.path().join("cache")).expect("delete");
        ctx.scope.backend = ExecutionBackend::Container {
            image: "alpine:3".into(),
        };
        let prepared = prepare("true", &ctx).expect("prepare");
        assert!(!rendered(&prepared.command).contains("CARGO_TARGET_DIR"));
        assert!(!cache.exists(), "nothing is created for a container");
    }

    #[test]
    fn a_temp_dir_that_is_not_ours_refuses_the_command() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ctx = with_temp_dir(dir.path());
        let leaf = ctx.temp_dir.clone().expect("temp dir");
        std::os::unix::fs::symlink(dir.path(), &leaf).expect("symlink");
        let e = prepare("true", &ctx).expect_err("a symlinked temp dir");
        assert!(
            matches!(&e, ToolError::Internal(m) if m.contains("not a real directory")),
            "{e:?}"
        );
    }

    #[test]
    fn the_container_backend_keeps_the_image_temp_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut ctx = with_temp_dir(dir.path());
        ctx.scope.backend = ExecutionBackend::Container {
            image: "alpine:3".into(),
        };
        let prepared = prepare("true", &ctx).expect("prepare");
        let line = rendered(&prepared.command);
        assert!(!line.contains("TMPDIR") && !line.contains("TEMP"), "{line}");
        let envs: Vec<_> = prepared
            .command
            .as_std()
            .get_envs()
            .map(|(k, _)| k.to_owned())
            .collect();
        assert!(!envs.iter().any(|k| k == "TMPDIR"), "{envs:?}");
        assert!(
            !ctx.temp_dir.expect("temp dir").exists(),
            "nothing is created for a container"
        );
    }

    #[test]
    fn the_container_backend_mounts_the_scope_and_follows_the_network_policy() {
        let dir = tempfile::tempdir().expect("tempdir");
        let reference = tempfile::tempdir().expect("reference");
        let mut ctx = ctx_in(dir.path());
        let root = dir.path().to_string_lossy().into_owned();
        ctx.scope.backend = ExecutionBackend::Container {
            image: "alpine:3".into(),
        };
        ctx.scope.allowed_paths = vec![reference.path().to_path_buf()];
        ctx.env = BTreeMap::from([("K".to_owned(), "v".to_owned())]);
        let prepared = prepare("echo hi", &ctx).expect("prepare");
        let line = rendered(&prepared.command);

        assert!(line.starts_with("docker run --rm --init"), "{line}");
        assert!(line.contains(&format!("-v {root}:{root}")), "{line}");
        assert!(line.contains(&format!("-w {root}")), "{line}");
        // Outside the workspace is readable, never writable.
        assert!(
            line.contains(&format!("-v {0}:{0}:ro", reference.path().display())),
            "{line}"
        );
        assert!(line.contains("--network none"), "{line}");
        assert!(line.contains("-e K=v"), "{line}");
        assert!(line.ends_with("alpine:3 bash -lc echo hi"), "{line}");

        // A variable the daemon hides must not travel in as an argument
        // either; `pipes()` only strips it from the docker client.
        ctx.hidden_env = vec!["K".to_owned()].into();
        let hidden = rendered(&prepare("echo hi", &ctx).expect("prepare").command);
        assert!(!hidden.contains("-e K=v"), "{hidden}");
        ctx.hidden_env = Vec::new().into();

        // The container is named so a cancel can reach exactly this one.
        let Some(Teardown::Container { name }) = &prepared.teardown else {
            panic!("a container must be tearable down");
        };
        assert!(line.contains(&format!("--name {name}")), "{line}");
        assert!(name.starts_with("aether-"), "{name}");

        ctx.scope.network = NetworkPolicy::Allowed;
        let allowed = rendered(&prepare("echo hi", &ctx).expect("prepare").command);
        assert!(!allowed.contains("--network none"), "{allowed}");
    }

    #[test]
    fn the_container_is_locked_down_before_the_image_is_trusted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut ctx = ctx_in(dir.path());
        ctx.scope.backend = ExecutionBackend::Container {
            image: "alpine:3".into(),
        };
        let line = rendered(&prepare("true", &ctx).expect("prepare").command);
        for flag in [
            "--cap-drop ALL",
            "--security-opt no-new-privileges",
            "--pids-limit 512",
            "--memory 2g",
            "--cpus 2",
            "--read-only",
            "--tmpfs /tmp:rw,nosuid,nodev,size=256m",
        ] {
            assert!(line.contains(flag), "{flag} missing from {line}");
        }

        // An image is an argv element docker's own parser reads: one that
        // could be a flag would widen the sandbox rather than narrow it.
        ctx.scope.backend = ExecutionBackend::Container {
            image: "--privileged".into(),
        };
        assert!(matches!(prepare("true", &ctx), Err(ToolError::Denied(_))));

        // `:` separates the fields of a mount spec.
        ctx.scope.backend = ExecutionBackend::Container {
            image: "alpine:3".into(),
        };
        ctx.scope.allowed_paths = vec![PathBuf::from("/srv/a:b")];
        assert!(matches!(prepare("true", &ctx), Err(ToolError::Denied(_))));
    }

    #[test]
    fn a_socket_is_never_mounted_however_read_only_it_is() {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket = dir.path().join("docker.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&socket).expect("socket");
        let mut ctx = ctx_in(dir.path());
        ctx.scope.backend = ExecutionBackend::Container {
            image: "alpine:3".into(),
        };
        ctx.scope.allowed_paths = vec![socket];
        // `:ro` on the docker socket is still the whole docker API, which is
        // root on this machine.
        assert!(matches!(prepare("true", &ctx), Err(ToolError::Denied(_))));
    }

    #[test]
    fn an_allowed_directory_containing_a_nested_socket_is_refused() {
        let workspace = tempfile::tempdir().expect("workspace");
        let outside = tempfile::tempdir().expect("outside");
        let nested = outside.path().join("run/service");
        std::fs::create_dir_all(&nested).expect("nested directories");
        let socket = nested.join("api.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&socket).expect("socket");
        let mut ctx = ctx_in(workspace.path());
        ctx.scope.backend = ExecutionBackend::Container {
            image: "alpine:3".into(),
        };
        ctx.scope.allowed_paths = vec![outside.path().to_path_buf()];

        let error = prepare("true", &ctx).expect_err("a directory exposes its nested socket");
        assert!(matches!(error, ToolError::Denied(_)), "{error:?}");
        assert!(error.to_string().contains(&socket.display().to_string()));

        let alias = workspace.path().join("reference");
        std::os::unix::fs::symlink(outside.path(), &alias).expect("root symlink");
        ctx.scope.allowed_paths = vec![alias];
        assert!(matches!(prepare("true", &ctx), Err(ToolError::Denied(_))));
        assert!(matches!(
            prepare_interactive("true", &ctx, true),
            Err(ToolError::Denied(_))
        ));
    }

    #[test]
    fn ordinary_mounts_preserve_nested_symlinks_without_following_host_targets() {
        let workspace = tempfile::tempdir().expect("workspace");
        let outside = tempfile::tempdir().expect("outside");
        let unmounted = tempfile::tempdir().expect("unmounted");
        let nested = outside.path().join("nested");
        std::fs::create_dir(&nested).expect("nested directory");
        let file = nested.join("reference.txt");
        std::fs::write(&file, "reference").expect("reference file");
        std::os::unix::fs::symlink("..", nested.join("parent")).expect("symlink loop");
        std::os::unix::fs::symlink("missing", nested.join("dangling")).expect("dangling link");
        let socket = unmounted.path().join("host.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&socket).expect("socket");
        std::os::unix::fs::symlink(&socket, nested.join("socket-link")).expect("socket link");
        let mut ctx = ctx_in(workspace.path());
        ctx.scope.backend = ExecutionBackend::Container {
            image: "alpine:3".into(),
        };

        let alias = workspace.path().join("reference");
        std::os::unix::fs::symlink(outside.path(), &alias).expect("root symlink");
        for path in [outside.path().to_path_buf(), file, alias] {
            ctx.scope.allowed_paths = vec![path.clone()];
            let line = rendered(&prepare("true", &ctx).expect("ordinary mount").command);
            assert!(line.contains(&format!("-v {0}:{0}:ro", path.display())));
        }
    }

    #[test]
    fn an_allowed_mount_that_cannot_be_inspected_is_refused() {
        let workspace = tempfile::tempdir().expect("workspace");
        let missing = workspace.path().join("missing");
        let dangling = workspace.path().join("dangling");
        std::os::unix::fs::symlink(&missing, &dangling).expect("dangling root symlink");
        let mut ctx = ctx_in(workspace.path());
        ctx.scope.backend = ExecutionBackend::Container {
            image: "alpine:3".into(),
        };

        for path in [missing, dangling] {
            ctx.scope.allowed_paths = vec![path.clone()];
            let error = prepare("true", &ctx).expect_err("an uninspected mount is unsafe");
            assert!(matches!(error, ToolError::Denied(_)), "{error:?}");
            assert!(error.to_string().contains(&path.display().to_string()));
        }
    }

    #[test]
    fn every_container_is_labelled_with_its_daemon_instance() {
        // Two daemons on one machine are a supported shape, and the orphan
        // sweep filters on this label: without it, starting the second kills
        // the first's running calls.
        let dir = tempfile::tempdir().expect("tempdir");
        let mut ctx = ctx_in(dir.path());
        ctx.instance_id = "01JINSTANCEA".into();
        ctx.scope.backend = ExecutionBackend::Container {
            image: "alpine:3".into(),
        };
        let line = rendered(&prepare("true", &ctx).expect("prepare").command);
        assert!(
            line.contains(&format!("--label {INSTANCE_LABEL}=01JINSTANCEA")),
            "{line}"
        );
        assert!(
            line.contains(&format!("--label {AGENT_LABEL}={}", ctx.agent_id)),
            "{line}"
        );
    }

    #[test]
    fn a_workspace_or_cwd_docker_would_misread_is_refused() {
        // These become a `-v` mount spec and a `-w` argument docker's own
        // parser reads, exactly like the extra roots that are already checked.
        let dir = tempfile::tempdir().expect("tempdir");
        let container = ExecutionBackend::Container {
            image: "alpine:3".into(),
        };

        let mut colon_workspace = ctx_in(dir.path());
        colon_workspace.scope.backend = container.clone();
        colon_workspace.scope.workspace_root = Some(PathBuf::from("/srv/a:b"));
        let e = prepare("true", &colon_workspace).expect_err("a colon is a field separator");
        assert!(matches!(e, ToolError::Denied(_)), "{e:?}");

        let mut flag_cwd = ctx_in(dir.path());
        flag_cwd.scope.backend = container.clone();
        flag_cwd.scope.workspace_root = Some(dir.path().to_path_buf());
        flag_cwd.cwd = PathBuf::from("--privileged");
        let e = prepare("true", &flag_cwd).expect_err("a leading dash is a flag");
        assert!(matches!(e, ToolError::Denied(_)), "{e:?}");

        // The host backend hands docker nothing, so it is unaffected.
        let mut host = ctx_in(dir.path());
        host.scope.workspace_root = Some(PathBuf::from("/srv/a:b"));
        assert!(prepare("true", &host).is_ok());
    }

    #[test]
    fn a_scope_that_owns_paths_is_enforced_before_the_command_is_built() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut ctx = ctx_in(dir.path());
        // No owned paths: the shell is unguarded, as it always was.
        assert!(prepare("git stash", &ctx).is_ok());

        ctx.scope.allowed_paths = vec![dir.path().join("mine")];
        let e = prepare("git diff && git stash", &ctx).expect_err("a bare stash takes everything");
        assert!(matches!(e, ToolError::Denied(_)), "{e:?}");
        // Reading, and writing inside what the agent owns, still run.
        assert!(prepare("git stash list", &ctx).is_ok());
        assert!(prepare("git checkout -- mine/a.rs", &ctx).is_ok());

        // The guard sits in `prepare_interactive`, so background and terminal
        // calls cannot route around it.
        let e = prepare_interactive("git clean -fd", &ctx, true).expect_err("guarded");
        assert!(matches!(e, ToolError::Denied(_)), "{e:?}");
    }

    #[test]
    fn two_calls_never_share_a_container_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut ctx = ctx_in(dir.path());
        ctx.scope.backend = ExecutionBackend::Container {
            image: "alpine:3".into(),
        };
        let one = prepare("true", &ctx).expect("prepare").teardown;
        let two = prepare("true", &ctx).expect("prepare").teardown;
        assert_ne!(one, two);
    }
}
