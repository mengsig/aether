#![allow(dead_code, clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! Both ends of remote control as a test drives them: the host's socket,
//! and a phone that is a real P-256 key, raw HTTP and a WebSocket.

use std::net::SocketAddr;
use std::time::Duration;

use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair as _};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use futures::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;

use crate::support::{RawConn, TestDaemon};

const CONFIG: &str = "[remote]\naddress = \"127.0.0.1:0\"\nmax_connections = 8\n";

/// A daemon and the operator's own connection to it.
pub struct Host {
    pub daemon: TestDaemon,
    pub conn: RawConn,
    /// Where the remote listener is bound, once enabled.
    pub listening: Option<SocketAddr>,
}

impl Host {
    /// A daemon with remote control configured and not yet enabled.
    pub async fn start() -> Host {
        let daemon = TestDaemon::start_with_config(CONFIG).await;
        Self::over(daemon).await
    }

    pub async fn over(daemon: TestDaemon) -> Host {
        let mut conn = RawConn::connect(&daemon.socket).await.expect("connect");
        conn.hello(aether_protocol::PROTOCOL_VERSION)
            .await
            .expect("welcome");
        let mut host = Host {
            daemon,
            conn,
            listening: None,
        };
        let status = host.ok(json!({"cmd": "get_remote"})).await;
        host.listening = listening_of(&status);
        host
    }

    /// A daemon listening, with pairing on.
    pub async fn enabled() -> Host {
        let mut host = Self::start().await;
        host.configure(json!({"enabled": true, "pairing": true}))
            .await;
        host
    }

    pub async fn ok(&mut self, frame: Value) -> Value {
        let answer = self.conn.call(frame).await;
        let ok = answer["result"]
            .get("Ok")
            .unwrap_or_else(|| panic!("refused: {answer}"));
        ok["data"].clone()
    }

    /// The error a request is refused with.
    pub async fn err(&mut self, frame: Value) -> Value {
        let answer = self.conn.call(frame).await;
        let refused = answer["result"].get("Err");
        refused
            .unwrap_or_else(|| panic!("accepted: {answer}"))
            .clone()
    }

    /// Replace the settings at the current revision; returns the status.
    pub async fn configure(&mut self, settings: Value) -> Value {
        let current = self.ok(json!({"cmd": "get_remote"})).await;
        let status = self
            .ok(json!({
                "cmd": "configure_remote",
                "settings": settings,
                "expected_revision": current["revision"],
            }))
            .await;
        self.listening = listening_of(&status);
        status
    }

    pub fn listening(&self) -> SocketAddr {
        self.listening.expect("the remote listener is open")
    }

    /// The origin a browser on the host itself has.
    pub fn origin(&self) -> String {
        format!("http://{}", self.listening())
    }

    /// Pair `phone` all the way: start, claim, confirm. Returns the device id.
    pub async fn pair(&mut self, phone: &Phone, name: &str, role: &str) -> String {
        let pairing = self.ok(json!({"cmd": "start_pairing", "role": role})).await;
        let claim = phone.claim(pairing["code"].as_str().expect("a code"), name);
        let claimed = http(self.listening(), &post(self, "/api/pair/claim", &claim)).await;
        assert_eq!(claimed.status, 200, "{}", claimed.body);
        let confirmed = self
            .ok(json!({"cmd": "confirm_pairing", "id": pairing["id"], "accept": true}))
            .await;
        assert_eq!(confirmed["state"], "enrolled", "{confirmed}");
        confirmed["device"]["id"]
            .as_str()
            .expect("a device id")
            .to_owned()
    }

    /// Open a socket and greet as `device`, which `phone` holds the key of.
    pub async fn connect(&self, phone: &Phone, device: &str) -> Socket {
        let mut socket = Socket::open(self.listening(), &self.origin()).await;
        let proof = phone.proof(&socket, &self.origin(), device);
        let welcome = socket
            .hello(Some(proof))
            .await
            .expect("the device is welcome");
        assert_eq!(welcome["device"]["id"], device, "{welcome}");
        socket
    }

    /// The payloads of every audit event called `name`, oldest first.
    pub async fn audit(&mut self, name: &str) -> Vec<Value> {
        let query = json!({"types": ["custom"]});
        let events = self
            .ok(json!({"cmd": "get_events", "after": 0, "limit": 1000, "query": query}))
            .await;
        let events = events.as_array().expect("events");
        let named = events.iter().filter(|event| event["kind"]["name"] == name);
        named
            .map(|event| event["kind"]["payload"].clone())
            .collect()
    }
}

fn listening_of(status: &Value) -> Option<SocketAddr> {
    let listening = status["listening"].as_str()?;
    Some(listening.parse().expect("a socket address"))
}

/// A device: nothing but a P-256 key, as a browser would hold one.
pub struct Phone {
    key: EcdsaKeyPair,
}

impl Phone {
    pub fn new() -> Phone {
        let rng = SystemRandom::new();
        let document = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng);
        let document = document.expect("generate");
        let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, document.as_ref());
        Phone {
            key: key.expect("parse"),
        }
    }

    pub fn public_key(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.key.public_key().as_ref())
    }

    /// The body of `POST /api/pair/claim`.
    pub fn claim(&self, code: &str, name: &str) -> Value {
        json!({"code": code, "public_key": self.public_key(), "device_name": name})
    }

    /// The answer to `socket`'s challenge, as `device` at `origin`.
    pub fn proof(&self, socket: &Socket, origin: &str, device: &str) -> Value {
        let id = device.parse().expect("a device id");
        let message =
            aether_protocol::remote::signing_input(&socket.instance_id, origin, id, &socket.nonce);
        let signature = self.key.sign(&SystemRandom::new(), &message).expect("sign");
        json!({"id": device, "signature": URL_SAFE_NO_PAD.encode(signature.as_ref())})
    }
}

pub struct HttpAnswer {
    pub status: u16,
    /// The status line and headers, lowercased.
    pub head: String,
    pub body: Value,
}

impl HttpAnswer {
    pub fn error_code(&self) -> &str {
        self.body["error"]["code"].as_str().unwrap_or_default()
    }
}

/// Send one raw request that asks to close, and read the whole answer.
pub async fn http(listening: SocketAddr, request: &str) -> HttpAnswer {
    let mut stream = TcpStream::connect(listening).await.expect("connect");
    stream.write_all(request.as_bytes()).await.expect("write");
    let mut answer = String::new();
    let reading = stream.read_to_string(&mut answer);
    let read = tokio::time::timeout(Duration::from_secs(10), reading).await;
    read.expect("an answer in time").expect("read");
    let (head, body) = answer.split_once("\r\n\r\n").expect("a head and a body");
    let status = head
        .split(' ')
        .nth(1)
        .expect("a status")
        .parse()
        .expect("a number");
    HttpAnswer {
        status,
        head: head.to_lowercase(),
        body: serde_json::from_str(body).unwrap_or(Value::Null),
    }
}

/// A JSON `POST` as the page served by `host` would send it.
pub fn post(host: &Host, path: &str, body: &Value) -> String {
    post_from(&host.listening().to_string(), &host.origin(), path, body)
}

/// A JSON `POST` naming an arbitrary `Host` and `Origin`.
pub fn post_from(host: &str, origin: &str, path: &str, body: &Value) -> String {
    let body = body.to_string();
    format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nOrigin: {origin}\r\nContent-Type: \
         application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// A device's WebSocket, speaking the daemon protocol by hand.
pub struct Socket {
    ws: WebSocketStream<TcpStream>,
    next_id: u64,
    /// The challenge the daemon opened with.
    pub nonce: String,
    pub instance_id: String,
}

impl Socket {
    pub async fn open(listening: SocketAddr, origin: &str) -> Socket {
        let host = listening.to_string();
        let opened = Self::try_open(listening, &host, origin).await;
        opened.unwrap_or_else(|status| panic!("the upgrade was refused with {status}"))
    }

    /// Upgrade naming `host` and `origin`; the HTTP status if refused.
    pub async fn try_open(listening: SocketAddr, host: &str, origin: &str) -> Result<Socket, u16> {
        let mut request = format!("ws://{listening}/ws")
            .into_client_request()
            .expect("a request");
        let headers = request.headers_mut();
        headers.insert("host", host.parse().expect("host"));
        headers.insert("origin", origin.parse().expect("origin"));
        headers.insert("sec-websocket-protocol", "aether".parse().expect("token"));
        let stream = TcpStream::connect(listening).await.expect("connect");
        let ws = match tokio_tungstenite::client_async(request, stream).await {
            Ok((ws, _)) => ws,
            Err(tokio_tungstenite::tungstenite::Error::Http(refused)) => {
                return Err(refused.status().as_u16());
            }
            Err(other) => panic!("the upgrade failed: {other}"),
        };
        let mut socket = Socket {
            ws,
            next_id: 1,
            nonce: String::new(),
            instance_id: String::new(),
        };
        let challenge = socket.frame().await.expect("a challenge comes first");
        assert_eq!(challenge["frame"], "challenge", "{challenge}");
        socket.nonce = challenge["nonce"].as_str().expect("a nonce").to_owned();
        socket.instance_id = challenge["instance_id"]
            .as_str()
            .expect("an instance")
            .to_owned();
        Ok(socket)
    }

    /// The next protocol frame, or the close code that ended the socket.
    async fn frame(&mut self) -> Result<Value, u16> {
        loop {
            let next = tokio::time::timeout(Duration::from_secs(15), self.ws.next()).await;
            match next
                .expect("a frame in time")
                .expect("a close before the end")
            {
                Ok(Message::Text(text)) => return Ok(serde_json::from_str(&text).expect("JSON")),
                Ok(Message::Close(frame)) => return Err(frame.expect("a close code").code.into()),
                Ok(_) => {}
                Err(error) => panic!("the socket failed before closing: {error}"),
            }
        }
    }

    pub async fn send_text(&mut self, text: String) {
        // A daemon that has already hung up refuses the write; the close
        // code read next says why.
        let _ = self.ws.send(Message::text(text)).await;
    }

    /// Send one request and read the reply to it.
    pub async fn call(&mut self, mut frame: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        frame["req"] = id.into();
        self.send_text(frame.to_string()).await;
        loop {
            let answer = self.frame().await.expect("a reply before any close");
            if answer["frame"] == "reply" {
                assert_eq!(answer["id"], id, "reply out of order: {answer}");
                return answer;
            }
        }
    }

    /// Greet, with or without a device proof.
    pub async fn hello(&mut self, proof: Option<Value>) -> Result<Value, Value> {
        let mut hello = crate::support::hello_frame(aether_protocol::PROTOCOL_VERSION, None);
        if let Some(proof) = proof {
            hello["device"] = proof;
        }
        crate::support::read_answer(self.call(hello).await)
    }

    /// Read to the close: the frames before it, and its code.
    pub async fn closed(mut self) -> (Vec<Value>, u16) {
        let mut frames = Vec::new();
        loop {
            match self.frame().await {
                Ok(frame) => frames.push(frame),
                Err(code) => return (frames, code),
            }
        }
    }
}
