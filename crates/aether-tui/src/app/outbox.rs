//! What the client's outbox reported: the operations still unconfirmed, and
//! how a person's decision about one of them went.
use super::{App, ClientError, Reply};
use aether_client::{Settlement, Undecided};

impl App {
    pub(super) fn outbox_read(
        &mut self,
        settled: Option<(Settlement, Result<Option<Reply>, ClientError>)>,
        undecided: Result<Undecided, ClientError>,
    ) {
        let announce = settled.is_none();
        match settled {
            Some((choice, Ok(_))) => self.flash(match choice {
                Settlement::MarkDelivered => "marked delivered; nothing was sent",
                Settlement::SendAgain => "sent again; the host accepted it",
                Settlement::Discard => "discarded; nothing was sent",
            }),
            Some((_, Err(error))) => self.flash_error(format!("not settled: {error}")),
            None => {}
        }
        let undecided = match undecided {
            Ok(undecided) => undecided,
            Err(error) => {
                self.flash_error(format!("could not read the remote outbox: {error}"));
                return;
            }
        };
        debug_assert!(
            undecided
                .unconfirmed
                .iter()
                .all(|entry| entry.unconfirmed.is_some())
        );
        let more = undecided.len() > self.undelivered.len() + self.unreadable.len();
        if announce && more {
            self.flash_error(crate::ui_format!(
                self.theme,
                "{} remote operation(s) may or may not have run · /outbox to decide",
                "{} remote operation(s) may or may not have run | /outbox to decide",
                undecided.len()
            ));
        }
        self.undelivered = undecided.unconfirmed;
        self.unreadable = undecided.unreadable;
        self.dirty = true;
    }
}
