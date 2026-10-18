#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! `aether serve`, both doors, against in-process HTTP.
//!
//! Inbound is driven with real requests at a real listener; outbound posts to
//! a receiver in this process that can be told to fail.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aether_client::serve::{ServeConfig, Subscription, Webhook, signature};
use aether_client::sink::CommandSink;
use aether_core::agent::{Agent, AgentStatus};
use aether_core::event::{Actor, Event, EventKind};
use aether_protocol::{Command, Reply};
use async_trait::async_trait;
use http_body_util::{BodyExt as _, Full};
use hyper::body::Bytes;
use tokio_util::sync::CancellationToken;

const SECRET: &str = "test-only-signing-secret-32-bytes-long";

/// A daemon double: answers plausibly and remembers what it was asked.
#[derive(Debug, Default)]
struct Recording {
    seen: Mutex<Vec<Command>>,
    /// What `ListAgents` answers with, filtered the way the store filters it.
    agents: Vec<Agent>,
}

#[async_trait]
impl CommandSink for Recording {
    async fn call(&self, command: Command) -> anyhow::Result<Reply> {
        self.seen.lock().expect("lock").push(command.clone());
        match command {
            Command::Trigger { .. } => Ok(Reply::Ok),
            // The store excludes ended agents exactly when `live_only` is
            // set, so the double has to as well or the flag proves nothing.
            Command::ListAgents { query } => Ok(Reply::Agents(
                self.agents
                    .iter()
                    .filter(|agent| !query.live_only || is_live(agent))
                    .cloned()
                    .collect(),
            )),
            other => anyhow::bail!("the double was not asked for {other:?}"),
        }
    }
}

fn is_live(agent: &Agent) -> bool {
    !matches!(
        agent.status,
        AgentStatus::Completed { .. } | AgentStatus::Failed { .. } | AgentStatus::Cancelled { .. }
    )
}

fn named_agent(name: &str, status: AgentStatus) -> Agent {
    let at = aether_core::time::now();
    Agent {
        id: aether_core::id::AgentId::new(),
        parent: None,
        role: aether_core::agent::Role::new("worker"),
        name: name.into(),
        scope: aether_core::agent::Scope::default(),
        model: aether_core::model::ModelRef::new("mock", "mock-1"),
        fallbacks: Vec::new(),
        status,
        archived: false,
        current_task: None,
        inbox_cursor: 0,
        checkpoint: None,
        engine_session: None,
        engine_usage: None,
        engine_sent: None,
        status_since: None,
        paused: false,
        paused_via: None,
        resume_wait: None,
        restarted_from: None,
        restarted_as: None,
        config: aether_core::agent::AgentConfig::default(),
        stats: Default::default(),
        run: aether_core::id::RunId::new(),
        created_at: at,
        updated_at: at,
    }
}

impl Recording {
    fn triggers(&self) -> Vec<Command> {
        self.seen
            .lock()
            .expect("lock")
            .iter()
            .filter(|command| matches!(command, Command::Trigger { .. }))
            .cloned()
            .collect()
    }
}

struct Serving {
    address: SocketAddr,
    sink: Arc<Recording>,
    shutdown: CancellationToken,
    _state: tempfile::TempDir,
}

impl Serving {
    async fn start(webhooks: Vec<Webhook>) -> Serving {
        Serving::start_with(webhooks, Vec::new()).await
    }

    async fn start_with(webhooks: Vec<Webhook>, agents: Vec<Agent>) -> Serving {
        let state = tempfile::tempdir().expect("replay state");
        Self::start_in(webhooks, agents, state).await
    }

    async fn start_in(
        webhooks: Vec<Webhook>,
        agents: Vec<Agent>,
        state: tempfile::TempDir,
    ) -> Serving {
        let sink = Arc::new(Recording {
            agents,
            ..Default::default()
        });
        let config = ServeConfig {
            address: Some("127.0.0.1:0".parse().expect("addr")),
            allow_remote: false,
            replay_path: state.path().join("webhook-replay.db"),
            webhooks,
            subscriptions: Vec::new(),
        };
        let shutdown = CancellationToken::new();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let serving = sink.clone();
        let token = shutdown.clone();
        tokio::spawn(async move {
            let _ = aether_client::serve::serve_webhooks(serving, &config, token, |bound| {
                let _ = tx.send(bound.expect("an inbound address"));
            })
            .await;
        });
        let address = rx.await.expect("the listener reported its address");
        Serving {
            address,
            sink,
            shutdown,
            _state: state,
        }
    }

    /// Post exactly these bytes with exactly these headers, so a test can
    /// send the same delivery twice.
    async fn post(&self, path: &str, delivery: &Delivery) -> (u16, String) {
        let mut request = reqwest::Client::new()
            .post(format!("http://{}{path}", self.address))
            .body(delivery.body.clone());
        for (name, value) in &delivery.headers {
            request = request.header(*name, value);
        }
        let response = request.send().await.expect("post");
        let status = response.status().as_u16();
        (status, response.text().await.unwrap_or_default())
    }
}

/// One request as it goes on the wire.
#[derive(Clone)]
struct Delivery {
    body: String,
    headers: Vec<(&'static str, String)>,
}

impl Delivery {
    /// A correctly stamped and signed delivery, as `aether serve` sends one.
    fn signed(body: &str, secret: &str) -> Delivery {
        let stamp = signature::Stamp::now();
        Delivery::stamped(body, secret, stamp)
    }

    fn stamped(body: &str, secret: &str, stamp: signature::Stamp) -> Delivery {
        let signed = signature::sign(secret, &signature::signed_payload(&stamp, body.as_bytes()))
            .expect("signed");
        Delivery {
            body: body.to_owned(),
            headers: vec![
                (signature::HEADER, signed),
                (signature::HEADER_TIMESTAMP, stamp.timestamp.to_string()),
                (signature::HEADER_NONCE, stamp.nonce),
            ],
        }
    }

    /// The same stamp, with the signature replaced by whatever is given.
    fn with_signature(mut self, presented: Option<&str>) -> Delivery {
        self.headers.retain(|(name, _)| *name != signature::HEADER);
        if let Some(value) = presented {
            self.headers.push((signature::HEADER, value.to_owned()));
        }
        self
    }

    fn without_stamp(mut self) -> Delivery {
        self.headers.retain(|(name, _)| *name == signature::HEADER);
        self
    }
}

fn hook() -> Webhook {
    Webhook {
        path: "/hooks/ci".into(),
        agent: "agt_01ARZ3NDEKTSV4RRFFQ69G5FAV".into(),
        source: "ci".into(),
        secret: SECRET.into(),
    }
}

#[path = "serve/hardening.rs"]
mod hardening;
#[path = "serve/inbound.rs"]
mod inbound;
#[path = "serve/lifecycle.rs"]
mod lifecycle;
#[path = "serve/outbound.rs"]
mod outbound;
#[path = "serve/reconnect.rs"]
mod reconnect;
