//! Network-denied Linux Codex turns run commands in a `codex exec-server` under bwrap's
//! private netns (only `lo`) plus seccomp barring unix/vsock sockets; the app-server stays out.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufRead, AsyncBufReadExt as _, AsyncReadExt as _};
use tokio::net::{TcpListener, UnixStream};

use crate::EngineError;

/// The Codex environment id for the sandboxed exec-server.
pub(super) const ENVIRONMENT_ID: &str = "aether-loopback";
/// The exec-server's port inside the namespace, which is new and so has
/// nothing else bound.
const INNER_PORT: u16 = 47_001;
/// How long the exec-server may take to listen.
const READY_TIMEOUT: Duration = Duration::from_secs(30);
/// How much of the sandbox's stderr a startup failure reports.
const STDERR_LIMIT: usize = 2_000;
/// How much of one stdout line the readiness check keeps.
const LINE_LIMIT: usize = 4_096;
/// How long a startup failure waits for the rest of that stderr.
const STDERR_GRACE: Duration = Duration::from_secs(2);
/// The seccomp filter is written for these little-endian architectures only.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const AUDIT_ARCH: Option<u32> = Some(0xC000_003E);
#[cfg(all(target_os = "linux", target_arch = "aarch64", target_endian = "little"))]
const AUDIT_ARCH: Option<u32> = Some(0xC000_00B7);
#[cfg(all(
    target_os = "linux",
    not(any(
        target_arch = "x86_64",
        all(target_arch = "aarch64", target_endian = "little")
    ))
))]
const AUDIT_ARCH: Option<u32> = None;

/// What `environment/add` registers with the app-server.
#[derive(Clone, PartialEq, Eq)]
pub(super) struct Environment {
    pub(super) url: String,
    pub(super) token: String,
}

impl std::fmt::Debug for Environment {
    // The token is a bearer credential for the exec-server.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Environment")
            .field("url", &self.url)
            .field("token", &"<redacted>")
            .finish()
    }
}

impl Environment {
    pub(super) fn add_params(&self) -> Value {
        json!({
            "environmentId": ENVIRONMENT_ID,
            "execServerUrl": self.url,
            "authBearerToken": self.token,
        })
    }
}

/// True when a `thread/start`, `thread/resume` or `thread/read` result shows
/// that the thread runs only in the sandboxed environment.
pub(super) fn confirms(thread: &Value) -> bool {
    thread["thread"]["environments"]
        .as_array()
        .is_some_and(|environments| {
            !environments.is_empty()
                && environments
                    .iter()
                    .all(|environment| environment["environmentId"] == ENVIRONMENT_ID)
        })
}

/// The running sandbox; dropping it stops everything inside.
pub(super) struct Sandbox {
    child: tokio::process::Child,
    bridge: tokio::task::JoinHandle<()>,
    // Closing it ends the sandbox even if Aether dies without dropping this.
    _stdin: tokio::process::ChildStdin,
    _dir: tempfile::TempDir,
    pub(super) environment: Environment,
}

impl Sandbox {
    /// Resolves when the namespace ends; before Drop, that means it died under the turn.
    pub(super) async fn exited(&mut self) -> String {
        match self.child.wait().await {
            Ok(status) => status.to_string(),
            Err(e) => format!("cannot wait for it: {e}"),
        }
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        self.bridge.abort();
        kill_namespace(&self.child);
    }
}

/// Killing bwrap alone leaves the namespace running. Its init shares bwrap's
/// group, and the kernel kills the whole namespace once init dies.
fn kill_namespace(child: &tokio::process::Child) {
    // None once reaped: the namespace is gone by then and the group id may be recycled.
    let Some(pid) = child.id() else {
        return;
    };
    assert!(
        (2..=i32::MAX as u32).contains(&pid),
        "not a sandbox pid: {pid}"
    );
    let group = nix::unistd::Pid::from_raw(pid as i32);
    match nix::sys::signal::killpg(group, nix::sys::signal::Signal::SIGKILL) {
        Ok(()) | Err(nix::errno::Errno::ESRCH) => {}
        Err(errno) => tracing::warn!(pid, %errno, "could not stop the codex loopback sandbox"),
    }
}

/// bwrap's seccomp program: only namespaced IP and netlink sockets plus connected unix pairs,
/// since host unix and vsock listeners could proxy egress; no io_uring, which skips `socket(2)`.
#[cfg(target_os = "linux")]
pub(super) fn seccomp_filter() -> Option<Vec<u8>> {
    use nix::libc;
    let arch = AUDIT_ARCH?;
    let load = (libc::BPF_LD | libc::BPF_W | libc::BPF_ABS) as u16;
    let equal = (libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K) as u16;
    let any_bit = (libc::BPF_JMP | libc::BPF_JSET | libc::BPF_K) as u16;
    let and = (libc::BPF_ALU | libc::BPF_AND | libc::BPF_K) as u16;
    let ret = (libc::BPF_RET | libc::BPF_K) as u16;
    let kill = libc::SECCOMP_RET_KILL_PROCESS;
    let errno = |errno: i32| libc::SECCOMP_RET_ERRNO | errno as u32;
    // (code, skip if true, skip if false, k); loads read `struct seccomp_data`,
    // whose arguments start at 16 and keep their int-sized low half first.
    let program = [
        (load, 0, 0, 4),     // 0
        (equal, 1, 0, arch), // 1
        (ret, 0, 0, kill),   // 2
        (load, 0, 0, 0),     // 3
        // x32 system calls would dodge the numbers below.
        (any_bit, 0, 1, 0x4000_0000),                   // 4
        (ret, 0, 0, kill),                              // 5
        (equal, 0, 1, libc::SYS_io_uring_setup as u32), // 6
        (ret, 0, 0, errno(libc::ENOSYS)),               // 7
        (equal, 0, 4, libc::SYS_socket as u32),         // 8
        (load, 0, 0, 16),                               // 9
        (equal, 10, 0, libc::AF_INET as u32),           // 10
        (equal, 9, 0, libc::AF_INET6 as u32),           // 11
        (equal, 8, 7, libc::AF_NETLINK as u32),         // 12
        (equal, 0, 7, libc::SYS_socketpair as u32),     // 13
        (load, 0, 0, 16),                               // 14
        (equal, 0, 4, libc::AF_UNIX as u32),            // 15
        (load, 0, 0, 24),                               // 16
        // The socket type without SOCK_NONBLOCK and SOCK_CLOEXEC.
        (and, 0, 0, 0xf), // 17
        // A datagram pair can still connect or send to any bound unix socket.
        (equal, 2, 0, libc::SOCK_STREAM as u32),    // 18
        (equal, 1, 0, libc::SOCK_SEQPACKET as u32), // 19
        (ret, 0, 0, errno(libc::EPERM)),            // 20
        (ret, 0, 0, libc::SECCOMP_RET_ALLOW),       // 21
    ];
    Some(
        program
            .into_iter()
            .flat_map(|(code, jt, jf, k): (u16, u8, u8, u32)| {
                let mut instruction = code.to_ne_bytes().to_vec();
                instruction.extend([jt, jf]);
                instruction.extend(k.to_ne_bytes());
                instruction
            })
            .collect(),
    )
}

/// Seccomp is Linux-only, and `loopback_sandboxed` never selects this sandbox elsewhere.
#[cfg(not(target_os = "linux"))]
pub(super) fn seccomp_filter() -> Option<Vec<u8>> {
    None
}

/// The bwrap command line; the shell script receives the socket, program,
/// token digest, port and seccomp file as `$1`..`$5`, never re-parsed.
pub(super) fn bwrap_args(
    socket: &Path,
    program: &str,
    token_sha256: &str,
    seccomp: &Path,
) -> Vec<String> {
    // The script is the namespace's pid 1, so the kernel kills everything when it ends:
    // once the exec-server exits or Aether's stdin closes. The inner pid namespace hides socat.
    let script = r#"socat "UNIX-LISTEN:$1,fork" "TCP:127.0.0.1:$4" &
bwrap --dev-bind / / --unshare-pid --proc /proc --seccomp 9 -- "$2" exec-server --listen "ws://127.0.0.1:$4" --ws-auth capability-token --ws-token-sha256 "$3" </dev/null 9<"$5" &
server=$!
exec 3<&0
{ cat <&3 >/dev/null; kill "$server"; } &
wait "$server""#;
    [
        "--dev-bind",
        "/",
        "/",
        "--unshare-net",
        "--unshare-pid",
        "--as-pid-1",
        "--die-with-parent",
        "--",
        "sh",
        "-c",
        script,
        "sh",
    ]
    .into_iter()
    .map(str::to_owned)
    .chain([
        socket.to_string_lossy().into_owned(),
        program.to_owned(),
        token_sha256.to_owned(),
        INNER_PORT.to_string(),
        seccomp.to_string_lossy().into_owned(),
    ])
    .collect()
}

/// Start the sandboxed exec-server and the host bridge to it.
pub(super) async fn start(
    program: &str,
    cwd: &Path,
    hidden_env: &[String],
    temp_dir: Option<&Path>,
    build_env: &[(String, std::path::PathBuf)],
) -> Result<Sandbox, EngineError> {
    use std::os::unix::process::CommandExt as _;

    let dir = tempfile::Builder::new()
        .prefix("aether-codex-")
        .tempdir()
        .map_err(|e| unsupported(format!("cannot create its socket directory: {e}")))?;
    let socket = dir.path().join("exec.sock");
    // socat parses `:` and `,` in its address argument.
    if !socket.to_str().is_some_and(|path| {
        path.chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-".contains(c))
    }) {
        return Err(unsupported(format!(
            "socket path {} is not safe to pass to socat",
            socket.display()
        )));
    }
    let filter = seccomp_filter()
        .ok_or_else(|| unsupported("its seccomp filter does not support this OS or CPU".into()))?;
    let seccomp = dir.path().join("unix.bpf");
    tokio::fs::write(&seccomp, filter)
        .await
        .map_err(|e| unsupported(format!("cannot write its seccomp filter: {e}")))?;
    // Bound before bwrap starts, so no later failure can leave the namespace running.
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|e| unsupported(format!("cannot bind its host bridge: {e}")))?;
    let port = listener
        .local_addr()
        .map_err(|e| unsupported(format!("cannot read its host bridge address: {e}")))?
        .port();
    let mut raw = [0u8; 32];
    rand::fill(&mut raw);
    let token = hex::encode(raw);
    let digest = hex::encode(Sha256::digest(token.as_bytes()));

    let mut command = tokio::process::Command::new("bwrap");
    command
        .args(bwrap_args(&socket, program, &digest, &seccomp))
        .current_dir(cwd)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    command.as_std_mut().process_group(0);
    crate::strip_hidden(&mut command, hidden_env);
    crate::export_temp_dir(&mut command, temp_dir);
    crate::export_build_env(&mut command, build_env);
    let mut child = command
        .spawn()
        .map_err(|e| unsupported(format!("cannot start bwrap: {e}")))?;
    let (Some(stdin), Some(stdout), Some(stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        kill_namespace(&child);
        return Err(unsupported("bwrap has no stdio pipes".into()));
    };
    let mut stderr = tokio::spawn(stderr_head(stderr));
    let mut stdout = tokio::io::BufReader::new(stdout);
    // The exec-server prints its URL once it listens.
    let ready = tokio::time::timeout(READY_TIMEOUT, async {
        loop {
            tokio::select! {
                line = capped_line(&mut stdout) => match line {
                    Ok(Some(line)) if line.starts_with(b"ws://") => return wait_for(&socket).await,
                    Ok(Some(_)) => {}
                    Ok(None) | Err(_) => return false,
                },
                _ = child.wait() => return false,
            }
        }
    })
    .await
    .unwrap_or(false);
    if !ready {
        kill_namespace(&child);
        let _ = child.wait().await;
        // Bounded in case something outside the namespace inherited the pipe.
        let detail = match tokio::time::timeout(STDERR_GRACE, &mut stderr).await {
            Ok(head) => head.unwrap_or_default(),
            Err(_) => {
                stderr.abort();
                String::new()
            }
        };
        return Err(unsupported(format!(
            "the sandboxed codex exec-server did not start{}",
            if detail.trim().is_empty() {
                String::new()
            } else {
                format!(": {}", detail.trim())
            }
        )));
    }
    // A closed pipe would kill the exec-server on its next write.
    tokio::spawn(async move { tokio::io::copy(&mut stdout, &mut tokio::io::sink()).await });
    let bridge = tokio::spawn(bridge(listener, socket));
    Ok(Sandbox {
        child,
        bridge,
        _stdin: stdin,
        _dir: dir,
        environment: Environment {
            url: format!("ws://127.0.0.1:{port}"),
            token,
        },
    })
}

fn unsupported(reason: String) -> EngineError {
    EngineError::Unsupported(format!(
        "Network-denied Codex turns could not set up their loopback-only sandbox: {reason}"
    ))
}

/// socat creates the socket before the exec-server starts, so this rarely waits.
async fn wait_for(socket: &Path) -> bool {
    for _ in 0..100 {
        if socket.exists() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
}

/// The next line without its newline, cut to [`LINE_LIMIT`] bytes so a
/// newline-free stream cannot grow memory; `None` at end of stream.
pub(super) async fn capped_line(
    reader: &mut (impl AsyncBufRead + Unpin),
) -> std::io::Result<Option<Vec<u8>>> {
    let mut line = Vec::new();
    loop {
        let buffer = reader.fill_buf().await?;
        if buffer.is_empty() {
            return Ok((!line.is_empty()).then_some(line));
        }
        let newline = buffer.iter().position(|&byte| byte == b'\n');
        let end = newline.unwrap_or(buffer.len());
        let room = LINE_LIMIT.saturating_sub(line.len());
        line.extend_from_slice(&buffer[..end.min(room)]);
        reader.consume(newline.map_or(end, |at| at + 1));
        if newline.is_some() {
            return Ok(Some(line));
        }
    }
}

/// Keep the first [`STDERR_LIMIT`] bytes of stderr for a startup error, and
/// drain the rest so the sandbox never blocks on a full pipe.
pub(super) async fn stderr_head(mut stderr: impl tokio::io::AsyncRead + Unpin) -> String {
    let mut head = Vec::new();
    let mut chunk = [0u8; 4_096];
    while let Ok(read @ 1..) = stderr.read(&mut chunk).await {
        let room = STDERR_LIMIT.saturating_sub(head.len());
        head.extend_from_slice(&chunk[..read.min(room)]);
    }
    String::from_utf8_lossy(&head).into_owned()
}

/// Forward every host connection to the sandbox's unix socket. Aborting this
/// task drops the open connections with it.
async fn bridge(listener: TcpListener, socket: PathBuf) {
    let mut connections = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let mut tcp = match accepted {
                    Ok((tcp, _)) => tcp,
                    Err(e) => {
                        tracing::warn!(error = %e, "codex loopback bridge could not accept");
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                };
                let socket = socket.clone();
                connections.spawn(async move {
                    match UnixStream::connect(&socket).await {
                        Ok(mut unix) => {
                            let _ = tokio::io::copy_bidirectional(&mut tcp, &mut unix).await;
                        }
                        Err(e) => tracing::warn!(error = %e, "codex loopback bridge lost the sandbox"),
                    }
                });
            }
            Some(_) = connections.join_next() => {}
        }
    }
}
