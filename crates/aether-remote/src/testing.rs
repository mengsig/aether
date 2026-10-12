//! Doubles the crate's tests share: a device store in memory, a daemon that
//! echoes, and a real P-256 key to sign with.

use std::sync::Arc;

use aether_protocol::remote::{DeviceId, DeviceProof, DeviceRole};
use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair as _};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_util::sync::CancellationToken;

use crate::auth::{AuthError, DeviceRecord, DeviceSession, DeviceStore};
use crate::{DeviceGate, Ended, RemoteConfig, RemoteHost, RemoteServer, Tuning};

pub(crate) const INSTANCE: &str = "inst";

#[derive(Debug, Default)]
pub(crate) struct MemoryDevices {
    records: parking_lot::Mutex<Vec<DeviceRecord>>,
    /// Every outcome the authenticator reported, in order.
    pub outcomes: parking_lot::Mutex<Vec<String>>,
    /// Revoke a device as soon as it has been read this many times.
    pub revoke_after_reads: parking_lot::Mutex<Option<u32>>,
}

impl DeviceStore for MemoryDevices {
    fn device(&self, id: DeviceId) -> Result<Option<DeviceRecord>, AuthError> {
        let mut records = self.records.lock();
        let found = records.iter().find(|d| d.id == id).cloned();
        let mut countdown = self.revoke_after_reads.lock();
        if let Some(left) = countdown.as_mut() {
            *left = left.saturating_sub(1);
            if *left == 0 {
                *countdown = None;
                for record in records.iter_mut().filter(|d| d.id == id) {
                    record.revoked_at = Some(aether_core::time::now());
                }
            }
        }
        Ok(found)
    }

    fn devices(&self, include_revoked: bool) -> Result<Vec<DeviceRecord>, AuthError> {
        let records = self.records.lock();
        Ok(records
            .iter()
            .filter(|d| include_revoked || d.revoked_at.is_none())
            .cloned()
            .collect())
    }

    fn accepted(&self, device: &DeviceRecord, origin: &str) -> Result<(), AuthError> {
        self.outcomes
            .lock()
            .push(format!("accepted {} {origin}", device.id));
        Ok(())
    }

    fn refused(&self, device: DeviceId, origin: &str, reason: &AuthError) {
        self.outcomes
            .lock()
            .push(format!("refused {device} {origin}: {reason}"));
    }
}

impl MemoryDevices {
    pub(crate) fn put(&self, record: &DeviceRecord) {
        let mut records = self.records.lock();
        records.retain(|d| d.id != record.id);
        records.push(record.clone());
    }
}

pub(crate) fn key_pair() -> EcdsaKeyPair {
    let rng = SystemRandom::new();
    let document =
        EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).expect("generate");
    EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, document.as_ref()).expect("parse")
}

/// A fixed-width signature, base64url: what WebCrypto produces.
pub(crate) fn sign(key: &EcdsaKeyPair, message: &[u8]) -> String {
    let signature = key.sign(&SystemRandom::new(), message).expect("sign");
    URL_SAFE_NO_PAD.encode(signature.as_ref())
}

/// An unrevoked operator record holding `key`'s public half.
pub(crate) fn record(key: &EcdsaKeyPair) -> DeviceRecord {
    DeviceRecord {
        id: DeviceId::new(),
        name: "phone".into(),
        role: DeviceRole::Operator,
        public_key: key.public_key().as_ref().to_vec(),
        created_at: aether_core::time::now(),
        last_seen_at: None,
        revoked_at: None,
    }
}

/// Lines the `flood` command writes, each [`FLOOD_LINE_BYTES`] long.
pub(crate) const FLOOD_LINES: usize = 4096;
pub(crate) const FLOOD_LINE_BYTES: usize = 1024;

/// A daemon in miniature. It sends `challenge <nonce>`, takes
/// `hello <device> <signature>` through the gate, and echoes anything else;
/// `flood`, `fail` and `idle` provoke the endings a carrier must report.
#[derive(Debug, Default)]
pub(crate) struct EchoHost;

impl EchoHost {
    fn greet(gate: &DeviceGate, hello: &str) -> Result<DeviceSession, String> {
        let (id, signature) = hello
            .split_once(' ')
            .ok_or("a hello is `<device> <signature>`")?;
        let proof = DeviceProof {
            id: id.parse().map_err(|_| "not a device id")?,
            signature: signature.to_owned(),
        };
        gate.authenticator
            .authenticate(&gate.challenge, &proof)
            .map_err(|refused| refused.to_string())
    }
}

#[async_trait::async_trait]
impl RemoteHost for EchoHost {
    fn instance_id(&self) -> &str {
        INSTANCE
    }

    async fn serve(
        &self,
        stream: tokio::io::DuplexStream,
        gate: DeviceGate,
        shutdown: CancellationToken,
    ) -> anyhow::Result<Ended> {
        let (read, mut write) = tokio::io::split(stream);
        let mut lines = BufReader::new(read).lines();
        let challenge = format!("challenge {}\n", gate.challenge.nonce);
        write.write_all(challenge.as_bytes()).await?;
        // Replaced by the session's own token once a hello is accepted.
        let mut revoked = CancellationToken::new();
        loop {
            let line = tokio::select! {
                line = lines.next_line() => line?,
                _ = shutdown.cancelled() => return Ok(Ended::ShuttingDown),
                _ = revoked.cancelled() => return Ok(Ended::Revoked),
            };
            let Some(line) = line else {
                return Ok(Ended::Closed);
            };
            let answer = match line.as_str() {
                "fail" => anyhow::bail!("the daemon broke"),
                "idle" => return Ok(Ended::TimedOut),
                "flood" => format!("{}\n", "x".repeat(FLOOD_LINE_BYTES - 1)).repeat(FLOOD_LINES),
                other => match other.strip_prefix("hello ") {
                    None => format!("echo {other}\n"),
                    Some(hello) => match Self::greet(&gate, hello) {
                        Ok(session) => {
                            revoked = session.closed;
                            format!("welcome {}\n", session.identity.name)
                        }
                        Err(refused) => {
                            write
                                .write_all(format!("refused {refused}\n").as_bytes())
                                .await?;
                            return Ok(Ended::Refused);
                        }
                    },
                },
            };
            write.write_all(answer.as_bytes()).await?;
        }
    }
}

/// The hello line [`EchoHost`] takes: `record`'s device answering `nonce`
/// as a page at `origin` would.
pub(crate) fn hello(
    key: &EcdsaKeyPair,
    record: &DeviceRecord,
    origin: &str,
    nonce: &str,
) -> String {
    let message = aether_protocol::remote::signing_input(INSTANCE, origin, record.id, nonce);
    format!("hello {} {}", record.id, sign(key, &message))
}

pub(crate) fn config() -> RemoteConfig {
    RemoteConfig {
        address: "127.0.0.1:0".parse().expect("addr"),
        allow_remote: false,
        max_connections: 4,
        idle_timeout: std::time::Duration::from_secs(300),
        push_previews: false,
        push_subject: None,
    }
}

/// A server over the doubles, not yet listening, with its daemon's
/// shutdown token.
pub(crate) fn server(tuning: Tuning) -> (Arc<RemoteServer>, Arc<MemoryDevices>, CancellationToken) {
    let devices = Arc::new(MemoryDevices::default());
    let shutdown = CancellationToken::new();
    let server = RemoteServer::with_tuning(
        config(),
        tuning,
        Arc::new(EchoHost),
        devices.clone(),
        shutdown.clone(),
    )
    .expect("a loopback listener is allowed");
    (Arc::new(server), devices, shutdown)
}
