//! Puts each operation an earlier connection left unanswered to the host and
//! acts on the answer; docs/remote.md has the algorithm.
use crate::outbox::{Claim, Claimed, Outbox, Settled, Unreadable, Unresolved};
use crate::{Client, ClientError, Result};
use aether_protocol::{Command, ErrorReply, Reply, SubmissionResolution, UnprovenAbsence};
use std::sync::Arc;

/// What a person decides about an operation the host cannot account for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settlement {
    /// It did happen: stop tracking it.
    MarkDelivered,
    /// Send it again under the same key, accepting that it may run twice.
    SendAgain,
    /// It is no longer wanted: stop tracking it without sending.
    Discard,
}

const HOST_CANNOT_RESOLVE: &str =
    "the host speaks a protocol older than 18 and cannot say what became of a lost send";

fn unproven(reason: UnprovenAbsence) -> &'static str {
    match reason {
        UnprovenAbsence::NoHorizon => "it was sent before this client kept a horizon for the host",
        UnprovenAbsence::ForeignHorizon => {
            "the host's receipt ledger was started again after it was sent"
        }
        UnprovenAbsence::HorizonAhead => "the host's store was rolled back to before it was sent",
        UnprovenAbsence::Forgotten => "the host has dropped receipts recorded after it was sent",
    }
}

/// What waits on a person: operations the host could not account for, and
/// outbox files that are not operations at all.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Undecided {
    pub unconfirmed: Vec<Unresolved>,
    pub unreadable: Vec<Unreadable>,
}

impl Undecided {
    pub fn len(&self) -> usize {
        self.unconfirmed.len() + self.unreadable.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// What the host's answer means for one entry.
enum Fate {
    Executed(Box<Reply>),
    NeverRan,
    Unconfirmed(String),
}

impl Client {
    /// Resolve what an earlier connection left in `outbox`, then keep it for
    /// this one. An unconfirmed operation is not an error; a dead link is.
    pub(crate) async fn adopt_outbox(&mut self, outbox: Arc<Outbox>) -> Result<()> {
        assert!(
            self.welcome.is_some(),
            "an outbox is adopted after the hello"
        );
        assert!(self.outbox.is_none(), "a connection adopts one outbox");
        let resolving = self.horizon().is_some();
        for listed in outbox.pending().await?.entries {
            // One a live process is sending, or that was just retired, is not ours.
            let Claimed::Held(claim, entry) = outbox.claim(listed.key()).await? else {
                continue;
            };
            let fate = if resolving {
                self.fate_of(&entry).await?
            } else {
                Fate::Unconfirmed(HOST_CANNOT_RESOLVE.into())
            };
            self.apply(&outbox, &claim, *entry, fate).await?;
        }
        self.outbox = Some(outbox);
        Ok(())
    }

    /// The horizon this connection's sends are recorded under.
    pub(crate) fn horizon(&self) -> Option<&str> {
        self.welcome.as_ref()?.submission_horizon.as_deref()
    }

    async fn fate_of(&self, entry: &Unresolved) -> Result<Fate> {
        let asking = Command::ResolveSubmission {
            command: Box::new(entry.command.clone()),
            horizon: entry.horizon.clone(),
        };
        let fate = match self.request_inner(asking).await {
            Ok(response) => match response.reply {
                Reply::SubmissionResolved(SubmissionResolution::Executed { reply, .. }) => {
                    Fate::Executed(reply)
                }
                Reply::SubmissionResolved(SubmissionResolution::AbsentSafe { .. }) => {
                    Fate::NeverRan
                }
                Reply::SubmissionResolved(SubmissionResolution::Unknown { reason, .. }) => {
                    Fate::Unconfirmed(unproven(reason).into())
                }
                other => Fate::Unconfirmed(format!(
                    "the host answered the resolution with `{}`",
                    crate::reply_name(&other)
                )),
            },
            // The host answered and would not say; that is not a lost link.
            Err(ClientError::Protocol(refusal)) => Fate::Unconfirmed(format!(
                "the host refused to resolve it: {}",
                refusal.message
            )),
            Err(error) => return Err(error),
        };
        Ok(fate)
    }

    async fn apply(
        &mut self,
        outbox: &Outbox,
        claim: &Claim,
        entry: Unresolved,
        fate: Fate,
    ) -> Result<()> {
        match fate {
            Fate::Executed(reply) => {
                self.retire(outbox, claim, &entry, Settled::Executed, Ok(*reply))
                    .await
            }
            // Once the host has said it cannot tell, a later "never ran" is
            // not believed: only a person sends it again.
            Fate::NeverRan if entry.unconfirmed.is_none() => {
                let answer = self.send_unchanged(&entry).await?;
                self.retire(outbox, claim, &entry, Settled::Resent, answer)
                    .await
            }
            Fate::NeverRan => Ok(()),
            Fate::Unconfirmed(reason) => {
                if entry.unconfirmed.is_none() {
                    outbox.log(&entry, Settled::Unconfirmed, &reason).await?;
                }
                outbox.mark_unconfirmed(claim, &entry, &reason).await
            }
        }
    }

    /// Send an entry's command exactly as recorded, key included. A reply or
    /// a refusal is the host's answer; anything else leaves it unresolved.
    async fn send_unchanged(
        &self,
        entry: &Unresolved,
    ) -> Result<std::result::Result<Reply, ErrorReply>> {
        debug_assert!(!entry.key().is_empty());
        match self.request_inner(entry.command.clone()).await {
            Ok(response) => Ok(Ok(response.reply)),
            Err(ClientError::Protocol(refusal)) => Ok(Err(refusal)),
            Err(error) => Err(ClientError::DeliveryUnconfirmed(format!(
                "{} was sent again and not answered: {error}",
                entry.command.name()
            ))),
        }
    }

    async fn retire(
        &mut self,
        outbox: &Outbox,
        claim: &Claim,
        entry: &Unresolved,
        settled: Settled,
        answer: std::result::Result<Reply, ErrorReply>,
    ) -> Result<()> {
        let detail = answer
            .as_ref()
            .err()
            .map(|refusal| refusal.message.as_str());
        outbox
            .log(entry, settled, detail.unwrap_or_default())
            .await?;
        outbox.acknowledge(claim).await?;
        self.recovered.push((entry.command.clone(), answer));
        Ok(())
    }

    /// What waits on a person, oldest first. Each stays here, on every
    /// connection, until [`Self::settle`] decides it.
    pub async fn undecided(&self) -> Result<Undecided> {
        let Some(outbox) = &self.outbox else {
            return Ok(Undecided::default());
        };
        let pending = outbox.pending().await?;
        let mut unconfirmed = pending.entries;
        unconfirmed.retain(|entry| entry.unconfirmed.is_some());
        Ok(Undecided {
            unconfirmed,
            unreadable: pending.unreadable,
        })
    }

    /// Apply a person's decision to the waiting operation under `key`, and
    /// record it. `SendAgain` returns the host's answer. An unreadable file
    /// can only be discarded.
    pub async fn settle(&self, key: &str, choice: Settlement) -> Result<Option<Reply>> {
        let outbox = self.outbox.as_ref().ok_or(ClientError::Disconnected)?;
        let undecided = self.undecided().await?;
        if let Some(bad) = undecided.unreadable.iter().find(|bad| bad.key == key)
            && !undecided.unconfirmed.iter().any(|entry| entry.key() == key)
        {
            if choice != Settlement::Discard {
                return Err(ClientError::InvalidUsage(format!(
                    "`{key}` is not a readable operation; it can only be discarded"
                )));
            }
            outbox.discard_unreadable(bad).await?;
            return Ok(None);
        }
        if !undecided.unconfirmed.iter().any(|entry| entry.key() == key) {
            return Err(ClientError::InvalidUsage(format!(
                "no unconfirmed operation has the key `{key}`"
            )));
        }
        let (claim, entry) = match outbox.claim(key).await? {
            Claimed::Held(claim, entry) if entry.unconfirmed.is_some() => (claim, entry),
            Claimed::Busy => {
                return Err(ClientError::InvalidUsage(format!(
                    "`{key}` is being sent by another aether process; try again when it is done"
                )));
            }
            Claimed::Held(..) | Claimed::Gone => {
                return Err(ClientError::InvalidUsage(format!(
                    "no unconfirmed operation has the key `{key}`"
                )));
            }
        };
        let (settled, answer) = match choice {
            Settlement::MarkDelivered => (Settled::MarkedDelivered, None),
            Settlement::Discard => (Settled::Discarded, None),
            Settlement::SendAgain => {
                self.sendable(&entry.command)?;
                (Settled::SentAgain, Some(self.send_unchanged(&entry).await?))
            }
        };
        let refusal = answer.as_ref().and_then(|answer| answer.as_ref().err());
        let detail = refusal.map(|refusal| refusal.message.as_str());
        outbox
            .log(&entry, settled, detail.unwrap_or_default())
            .await?;
        outbox.acknowledge(&claim).await?;
        answer.transpose().map_err(ClientError::Protocol)
    }

    /// A host older than protocol 18 takes a key on six commands only, and
    /// sending any other without its key would not be the recorded operation.
    fn sendable(&self, command: &Command) -> Result<()> {
        let mut probe = command.clone();
        if self.prepare_remote_command(&mut probe)? {
            return Ok(());
        }
        Err(ClientError::InvalidUsage(format!(
            "this host no longer accepts an idempotency key on `{}`; mark it delivered or \
             discard it",
            command.name()
        )))
    }
}

#[cfg(test)]
#[path = "resolution_tests.rs"]
mod tests;
