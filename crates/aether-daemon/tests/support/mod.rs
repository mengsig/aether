#![allow(dead_code, clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! A real daemon on a scratch socket, and a raw NDJSON client for tests that
//! must assert what actually goes over the wire.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use aether_daemon::TcpListen;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::{TcpStream, UnixStream};
use tokio_util::sync::CancellationToken;

/// A daemon with its own socket, store and config directory. Everything it
/// writes lives under one temporary directory, removed on drop.
pub struct TestDaemon {
    _dir: tempfile::TempDir,
    pub socket: PathBuf,
    /// The TCP address, when this daemon was started with that transport.
    pub tcp: Option<SocketAddr>,
    shutdown: CancellationToken,
    serving: tokio::task::JoinHandle<anyhow::Result<()>>,
}

/// A loopback address nothing is listening on. Reserved by binding and
/// releasing, which is as close to a free port as a test can get.
pub fn free_loopback_addr() -> SocketAddr {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("reserve a port");
    listener.local_addr().expect("local addr")
}

impl TestDaemon {
    pub async fn start() -> TestDaemon {
        Self::launch(None, None, None).await
    }

    /// A daemon with the TCP transport open too. `tune` sees the listener
    /// with the reserved address and the token already set.
    pub async fn start_with_tcp(token: &str, tune: impl FnOnce(&mut TcpListen)) -> TestDaemon {
        let mut listen = TcpListen::new(free_loopback_addr(), token);
        tune(&mut listen);
        Self::launch(Some(listen), None, None).await
    }

    pub async fn start_with_concurrency(
        configured: Option<usize>,
        override_limit: Option<usize>,
    ) -> TestDaemon {
        Self::launch(None, configured, override_limit).await
    }

    /// A daemon whose `config.toml` is exactly `toml`.
    pub async fn start_with_config(toml: &str) -> TestDaemon {
        let dir = tempfile::tempdir().expect("tempdir");
        Self::launch_in(dir, None, Some(toml.to_owned()), None).await
    }

    /// Stop, then start again over the same store and configuration.
    pub async fn restart(self) -> TestDaemon {
        self.shutdown.cancel();
        let stopped = tokio::time::timeout(Duration::from_secs(10), self.serving).await;
        stopped
            .expect("the daemon stops in time")
            .expect("the daemon task joins")
            .expect("the daemon stops cleanly");
        Self::launch_in(self._dir, None, None, None).await
    }

    async fn launch(
        tcp: Option<TcpListen>,
        configured: Option<usize>,
        override_limit: Option<usize>,
    ) -> TestDaemon {
        let dir = tempfile::tempdir().expect("tempdir");
        let toml = configured.map(|limit| format!("[scheduler]\nmax_concurrent_turns = {limit}\n"));
        Self::launch_in(dir, tcp, toml, override_limit).await
    }

    /// Start a daemon in `dir`, writing `toml` as its configuration first.
    async fn launch_in(
        dir: tempfile::TempDir,
        tcp: Option<TcpListen>,
        toml: Option<String>,
        override_limit: Option<usize>,
    ) -> TestDaemon {
        let root = dir.path().to_path_buf();
        let socket = root.join("aetherd.sock");
        let address = tcp.as_ref().map(|listen| listen.addr);
        let config = aether_daemon::DaemonConfig {
            socket: socket.clone(),
            store_path: root.join("aether.db"),
            config_dir: Some(root.join("config")),
            workspace_root: Some(root.join("workspace")),
            max_concurrent_turns: override_limit,
            yolo: false,
            tcp,
            allow_remote: false,
        };
        std::fs::create_dir_all(root.join("workspace")).expect("workspace");
        if let Some(toml) = toml {
            std::fs::create_dir_all(root.join("config")).expect("config dir");
            std::fs::write(root.join("config/config.toml"), toml).expect("config");
        }
        let shutdown = CancellationToken::new();
        let serving = tokio::spawn(aether_daemon::run(config, shutdown.clone()));
        let daemon = TestDaemon {
            _dir: dir,
            socket,
            tcp: address,
            shutdown,
            serving,
        };
        daemon.await_answering().await;
        daemon
    }

    /// The database `launch` put beside the socket.
    pub fn store_path(&self) -> PathBuf {
        self.socket.with_file_name("aether.db")
    }

    pub fn tcp_addr(&self) -> SocketAddr {
        self.tcp
            .expect("this daemon was started with a TCP transport")
    }

    /// Wait for a daemon that answers, not merely for a socket path.
    async fn await_answering(&self) {
        for _ in 0..200 {
            if let Ok(mut conn) = RawConn::connect(&self.socket).await
                && conn.hello(aether_protocol::PROTOCOL_VERSION).await.is_ok()
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("daemon never answered on {}", self.socket.display());
    }

    pub async fn stop(self) {
        self.shutdown.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(10), self.serving).await;
    }
}

/// One connection, spoken by hand: the point is to send frames no compiled
/// client would build any more.
pub struct RawConn {
    lines: BufReader<Box<dyn tokio::io::AsyncRead + Send + Unpin>>,
    write: Box<dyn AsyncWrite + Send + Unpin>,
    next_id: u64,
    pub local_addr: Option<SocketAddr>,
}

impl RawConn {
    pub async fn connect(socket: &std::path::Path) -> std::io::Result<RawConn> {
        let (read, write) = UnixStream::connect(socket).await?.into_split();
        Ok(RawConn::over(Box::new(read), Box::new(write)))
    }

    pub async fn connect_tcp(addr: SocketAddr) -> std::io::Result<RawConn> {
        let stream = TcpStream::connect(addr).await?;
        let local_addr = stream.local_addr()?;
        let (read, write) = stream.into_split();
        let mut conn = RawConn::over(Box::new(read), Box::new(write));
        conn.local_addr = Some(local_addr);
        Ok(conn)
    }

    fn over(
        read: Box<dyn tokio::io::AsyncRead + Send + Unpin>,
        write: Box<dyn AsyncWrite + Send + Unpin>,
    ) -> RawConn {
        RawConn {
            lines: BufReader::new(read),
            write,
            next_id: 1,
            local_addr: None,
        }
    }

    /// Send one frame, adding the request id, and read the reply to it.
    pub async fn call(&mut self, frame: Value) -> Value {
        let id = self.request(frame).await;
        self.await_reply(id).await
    }

    /// Send one frame without waiting for its answer. Returns the request id.
    pub async fn request(&mut self, mut frame: Value) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        frame["req"] = id.into();
        self.send(&frame).await;
        id
    }

    pub async fn await_reply(&mut self, id: u64) -> Value {
        loop {
            let answer = self.recv().await.expect("a reply");
            if answer["frame"] == "reply" && answer["req"] != Value::Null {
                panic!("a server frame must not carry req: {answer}");
            }
            if answer["frame"] == "reply" {
                assert_eq!(answer["id"], id, "reply out of order: {answer}");
                return answer;
            }
        }
    }

    /// Exactly these bytes and a newline. For frames no encoder would emit.
    pub async fn send_line(&mut self, line: &str) {
        self.write
            .write_all(format!("{line}\n").as_bytes())
            .await
            .expect("write line");
    }

    pub async fn send(&mut self, frame: &Value) {
        let line = format!("{frame}\n");
        self.write
            .write_all(line.as_bytes())
            .await
            .expect("write frame");
    }

    /// The next frame, or `None` when the daemon closed the connection.
    pub async fn recv(&mut self) -> Option<Value> {
        let mut line = String::new();
        let read = tokio::time::timeout(Duration::from_secs(10), self.lines.read_line(&mut line))
            .await
            .expect("a frame within ten seconds")
            .expect("read frame");
        if read == 0 {
            return None;
        }
        Some(serde_json::from_str(&line).expect("a JSON frame"))
    }

    /// Whether the daemon has ended this connection. Either an orderly EOF or
    /// a reset: dropping a socket with bytes still unread sends an RST, and
    /// both mean the same thing to the peer.
    pub async fn is_closed(&mut self) -> bool {
        let mut line = String::new();
        match tokio::time::timeout(Duration::from_secs(10), self.lines.read_line(&mut line)).await {
            Ok(Ok(0)) | Ok(Err(_)) => true,
            Ok(Ok(_)) | Err(_) => false,
        }
    }

    /// Handshake at a chosen protocol version. Returns the welcome, or the
    /// error the daemon refused with.
    pub async fn hello(&mut self, version: u32) -> Result<Value, Value> {
        self.hello_with(version, None).await
    }

    /// Handshake presenting a token, as a tokened transport requires.
    pub async fn hello_with(&mut self, version: u32, token: Option<&str>) -> Result<Value, Value> {
        let answer = self.call(hello_frame(version, token)).await;
        read_answer(answer)
    }
}

/// A hello frame with no request id yet, for a test that must send one and
/// not wait for the answer.
pub fn hello_frame(version: u32, token: Option<&str>) -> Value {
    let mut frame = serde_json::json!({
        "cmd": "hello",
        "protocol_version": version,
        "client_name": "skew-test",
        "client_version": "0",
        "features": [],
    });
    if let Some(token) = token {
        frame["token"] = token.into();
    }
    frame
}

/// The welcome, or the error the daemon refused with.
pub fn read_answer(answer: Value) -> Result<Value, Value> {
    match answer["result"].get("Ok") {
        Some(ok) => Ok(ok["data"].clone()),
        None => Err(answer["result"]["Err"].clone()),
    }
}
