//! A one-line notice for an ignored request that someone expects an answer
//! to. It names the repository, thread, login and reason, never the text, and
//! never starts a turn: it is a transcript note or a feed event.
use super::poll;
use crate::{Result, Runtime, RuntimeError};
use aether_core::{
    agent::Agent,
    event::{Actor, EventDraft, EventKind},
    github::{GithubDispatchMode, GithubRepositoryConfig},
    note::{Note, NoteContext, NoteLevel},
};
use aether_store::{Commit, Mutation};
use serde_json::json;

/// The `custom` event a notice becomes when its thread has no live agent.
pub(super) const IGNORED_EVENT: &str = "github.watcher.ignored";

fn notice_key(repo: &str, number: u64, refusal: poll::Refusal) -> String {
    format!("github.watcher.notice:{repo}:{number}:{}", refusal.slug())
}

impl Runtime {
    /// Stage the notice for an ignored `candidate`, once per thread and
    /// refusal: a digest note on the thread's live target, or else a
    /// `github.watcher.ignored` event in the daemon's event log.
    pub(super) fn github_stage_notice(
        &self,
        commit: &mut Commit,
        repo: &GithubRepositoryConfig,
        candidate: &poll::Candidate,
        refusal: poll::Refusal,
    ) -> Result<()> {
        debug_assert!(candidate.number > 0, "GitHub thread numbers are positive");
        debug_assert!(poll::valid_login(&candidate.login), "logins are validated");
        let key = notice_key(&repo.repository, candidate.number, refusal);
        if self.store.meta(&key)?.is_some() {
            return Ok(());
        }
        let text = format!(
            "GitHub watcher ignored a request by `{}` on {}#{}: {}.",
            candidate.login,
            repo.repository,
            candidate.number,
            refusal.reason()
        );
        let draft = match self.github_notice_target(repo, candidate.number)? {
            Some(agent) => {
                let mut note = Note {
                    text,
                    links: vec![candidate.url.clone()],
                    context: NoteContext::Digest,
                    level: NoteLevel::Info,
                };
                // GitHub supplies the URL; a link the note refuses is dropped.
                if note.check().is_err() {
                    note.links.clear();
                }
                note.draft(agent.id, agent.run, Actor::System)
                    .map_err(|e| RuntimeError::Internal(format!("GitHub notice: {e}")))?
            }
            None => EventDraft::new(
                Actor::System,
                EventKind::Custom {
                    name: IGNORED_EVENT.into(),
                    payload: json!({
                        "repository": repo.repository,
                        "number": candidate.number,
                        "requester": candidate.login,
                        "reason": refusal.reason(),
                        "url": candidate.url,
                        "notice": text,
                    }),
                },
            ),
        };
        commit.push_event(draft);
        commit.push_mutation(Mutation::SetMeta {
            key,
            value: refusal.reason().into(),
        });
        Ok(())
    }

    /// The agent that would have answered this thread, if one is live now.
    /// Nothing is created or prepared for an ignored request.
    fn github_notice_target(
        &self,
        repo: &GithubRepositoryConfig,
        number: u64,
    ) -> Result<Option<Agent>> {
        match &repo.dispatch {
            GithubDispatchMode::Fixed => {
                let Some(id) = repo.agent else {
                    return Ok(None);
                };
                let agent = self.store.agent(id)?;
                Ok(agent.filter(|agent| !agent.status.is_terminal()))
            }
            GithubDispatchMode::Thread(_) => self.live_thread_agent(repo, number, false),
            GithubDispatchMode::Automatic => self.live_thread_agent(repo, number, true),
        }
    }
}
