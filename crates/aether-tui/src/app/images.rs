//! Images attached to the draft: how they arrive (a pasted path, `/image`,
//! the clipboard), the list the chips show, and the message they leave with.

use aether_core::attachment::{
    ImageAttachment, MAX_IMAGES_PER_MESSAGE, MAX_MESSAGE_IMAGE_BYTES, human_size,
};

use super::{App, Effect};
use crate::commands::{ImageArg, InputAction};
use crate::image_input::{ClipboardContent, ImageSource};
use crate::workspace;

/// A message on its way to an agent: its text and the images attached to it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outgoing {
    pub text: String,
    pub images: Vec<ImageAttachment>,
}

impl From<String> for Outgoing {
    fn from(text: String) -> Self {
        Self {
            text,
            images: Vec::new(),
        }
    }
}

impl From<&str> for Outgoing {
    fn from(text: &str) -> Self {
        text.to_owned().into()
    }
}

/// `[image: shot.png, 1.2 MB]`, the line a sent image leaves in the transcript.
pub(crate) fn image_label(image: &ImageAttachment) -> String {
    format!("[image: {}]", image.summary())
}

/// `text`, then one line per image label.
pub(crate) fn with_image_lines(text: &str, labels: impl Iterator<Item = String>) -> String {
    let mut lines: Vec<String> = Vec::new();
    let text = text.trim();
    if !text.is_empty() {
        lines.push(text.to_owned());
    }
    lines.extend(labels);
    lines.join("\n")
}

impl App {
    pub(super) fn restore_agent_message(&mut self, agent: super::AgentId, message: Outgoing) {
        if self.draft_owner == Some(agent) {
            debug_assert!(!self.drafts.contains_key(&agent));
            self.input.restore_message(message);
        } else {
            let mut parked = super::InputState::default();
            parked.swap_draft(self.drafts.remove(&agent).unwrap_or_default());
            parked.restore_message(message);
            self.drafts
                .insert(agent, parked.swap_draft(Default::default()));
            debug_assert!(self.drafts.contains_key(&agent));
        }
        self.dirty = true;
    }

    pub(crate) fn terminal_image_paste(&mut self) -> Vec<Effect> {
        if self.paste_reaches_composer() {
            return self.paste_clipboard_image();
        }
        Vec::new()
    }

    /// Ctrl+V: the clipboard image, else its text. Alt+V: only an image.
    pub(crate) fn paste_key(&mut self, image_only: bool) -> Vec<Effect> {
        if !self.paste_reaches_composer() {
            self.flash_hidden_composer("paste ignored");
            return Vec::new();
        }
        if image_only {
            return self.paste_clipboard_image();
        }
        self.paste_clipboard()
    }

    pub(crate) fn paste_clipboard(&mut self) -> Vec<Effect> {
        if self.refuse_while_stale_reads() {
            return Vec::new();
        }
        self.input.image_reads += 1;
        vec![Effect::PasteClipboard]
    }

    pub(crate) fn clipboard_read(
        &mut self,
        content: Result<ClipboardContent, String>,
    ) -> Vec<Effect> {
        if self.drop_stale_read() {
            return Vec::new();
        }
        debug_assert!(
            self.input.image_reads > 0,
            "a clipboard read nothing asked for"
        );
        self.input.image_reads = self.input.image_reads.saturating_sub(1);
        debug_assert!(self.input.images.len() <= MAX_IMAGES_PER_MESSAGE);
        match content {
            Ok(_) if crate::viewer::hides_composer(self) => {
                self.flash("paste ignored: the file viewer is full screen (Tab returns to chat)");
            }
            Ok(ClipboardContent::Image(image)) if self.paste_reaches_composer() => {
                self.attach_image(image);
            }
            Ok(ClipboardContent::Image(_)) => self.flash_hidden_composer("image not attached"),
            Ok(ClipboardContent::Text(text)) => return self.dispatch_paste(&text),
            Ok(ClipboardContent::ImagePath { text, image }) => {
                return self.image_path_pasted(&text, image);
            }
            Err(reason) => self.flash_error(format!("nothing pasted: {reason}")),
        }
        Vec::new()
    }

    /// A paste that named an image file. The composer attaches the image;
    /// whatever else holds the paste (a view, find, an approval) gets the path.
    pub(crate) fn image_path_pasted(
        &mut self,
        text: &str,
        image: Result<ImageAttachment, String>,
    ) -> Vec<Effect> {
        if !self.paste_reaches_composer() {
            return self.dispatch_paste(text);
        }
        match image {
            Ok(image) => {
                self.attach_image(image);
                Vec::new()
            }
            Err(reason) => {
                let effects = self.dispatch_paste(text);
                self.flash_error(format!("pasted as text, not attached: {reason}"));
                effects
            }
        }
    }

    /// Mirrors `dispatch_paste`: only an uncovered composer takes a paste.
    fn paste_reaches_composer(&mut self) -> bool {
        self.track_approvals();
        self.shown_approval().is_none()
            && !self.board.focused()
            && self.bottom.view_stack.is_empty()
            && self.bottom.find.is_none()
            && !self.search_focused()
            && !crate::viewer::hides_composer(self)
    }

    /// Nothing lands in a draft the person cannot see; say what hides it.
    fn flash_hidden_composer(&mut self, outcome: &str) {
        debug_assert!(!outcome.is_empty());
        let reason = if self.shown_approval().is_some() {
            "answer the approval first"
        } else if self.board.focused() {
            "the board has focus (esc leaves it)"
        } else {
            "esc closes what covers the draft first"
        };
        self.flash(format!("{outcome}: {reason}"));
    }

    /// A read finishing after the operator moved to another agent belongs to
    /// the old draft: drop it rather than attach it to the wrong agent.
    fn drop_stale_read(&mut self) -> bool {
        if self.input.stale_reads == 0 {
            return false;
        }
        self.input.stale_reads -= 1;
        self.flash("paste dropped: you switched agents while it was read; paste again");
        true
    }

    /// Replies carry no owner, so while an old agent's reads are outstanding
    /// no new read starts: every reply that arrives is then known to be stale.
    fn refuse_while_stale_reads(&mut self) -> bool {
        if self.input.stale_reads == 0 {
            return false;
        }
        self.flash("a paste for the previous agent is still being read: try again in a moment");
        true
    }

    /// `Effect::AttachImage` finished.
    pub(crate) fn image_read(&mut self, image: Result<ImageAttachment, String>) {
        if self.drop_stale_read() {
            return;
        }
        debug_assert!(self.input.image_reads > 0, "an image nothing asked for");
        self.input.image_reads = self.input.image_reads.saturating_sub(1);
        match image {
            Ok(_) if crate::viewer::hides_composer(self) => {
                self.flash("image not attached: return to chat before pasting");
            }
            Ok(image) => self.attach_image(image),
            Err(reason) => self.flash_error(format!("no image attached: {reason}")),
        }
    }

    /// Add `image` to the draft, unless the message would be more than the daemon takes.
    fn attach_image(&mut self, image: ImageAttachment) {
        self.dirty = true;
        let attached = &self.input.images;
        if attached.len() >= MAX_IMAGES_PER_MESSAGE {
            self.flash_error(format!(
                "not attached: a message takes at most {MAX_IMAGES_PER_MESSAGE} images"
            ));
            return;
        }
        let total = attached
            .iter()
            .chain([&image])
            .map(ImageAttachment::size_bytes)
            .sum::<u64>();
        if total > MAX_MESSAGE_IMAGE_BYTES as u64 {
            self.flash_error(format!(
                "not attached: the images of one message can total {} at most",
                human_size(MAX_MESSAGE_IMAGE_BYTES as u64)
            ));
            return;
        }
        // Said as well as shown: plain mode has no chips.
        self.flash(format!("attached {}", image.summary()));
        self.input.images.push(image);
        debug_assert!(self.input.images.len() <= MAX_IMAGES_PER_MESSAGE);
    }

    /// Why the chips cannot leave as one message; merged drafts can exceed what `attach_image` allows.
    pub(super) fn oversized_images(&self) -> Option<String> {
        let images = &self.input.images;
        if images.len() > MAX_IMAGES_PER_MESSAGE {
            return Some(format!(
                "a message takes at most {MAX_IMAGES_PER_MESSAGE} images ({} attached)",
                images.len()
            ));
        }
        let total = images.iter().map(ImageAttachment::size_bytes).sum::<u64>();
        (total > MAX_MESSAGE_IMAGE_BYTES as u64).then(|| {
            format!(
                "the images of one message can total {} at most",
                human_size(MAX_MESSAGE_IMAGE_BYTES as u64)
            )
        })
    }

    /// `/image`: the clipboard image, a file, or `--clear`.
    pub(crate) fn run_image_command(&mut self, arg: ImageArg) -> Vec<Effect> {
        let typed = match arg {
            ImageArg::Clipboard => return self.paste_clipboard_image(),
            ImageArg::Clear => {
                self.clear_images();
                return Vec::new();
            }
            ImageArg::File(typed) => typed,
        };
        let home = self.options.home.as_deref();
        let Some(path) = workspace::resolve(&typed, home, self.active_workspace()) else {
            self.flash_error(format!("no such file: {typed}"));
            return Vec::new();
        };
        self.read_image(ImageSource::File(path))
    }

    /// Ask for the image on the system clipboard; it arrives as `ImageRead`.
    pub(crate) fn paste_clipboard_image(&mut self) -> Vec<Effect> {
        if self.input.images.len() >= MAX_IMAGES_PER_MESSAGE {
            self.flash_error(format!(
                "not attached: a message takes at most {MAX_IMAGES_PER_MESSAGE} images"
            ));
            return Vec::new();
        }
        self.read_image(ImageSource::Clipboard)
    }

    fn read_image(&mut self, source: ImageSource) -> Vec<Effect> {
        if self.refuse_while_stale_reads() {
            return Vec::new();
        }
        self.input.image_reads += 1;
        vec![Effect::AttachImage(source)]
    }

    /// A message sent while an image is still being read would leave without it.
    pub(crate) fn refuse_unread_image(&mut self) -> bool {
        if self.input.image_reads == 0 {
            return false;
        }
        if matches!(self.parse_input(&self.input.text), InputAction::Command(_)) {
            return false;
        }
        self.flash("a paste or image is still being read: send again once it finishes");
        true
    }

    fn clear_images(&mut self) {
        let dropped = self.input.images.len();
        self.input.images.clear();
        self.dirty = true;
        self.flash(match dropped {
            0 => "no images attached".to_owned(),
            1 => "1 image removed".to_owned(),
            n => format!("{n} images removed"),
        });
    }

    /// Backspace at the very start of the draft takes the last chip away.
    pub(crate) fn remove_last_image(&mut self) -> bool {
        if self.input.cursor != 0 {
            return false;
        }
        let removed = self.input.images.pop().is_some();
        self.dirty |= removed;
        removed
    }

    /// Images cannot wait in a text-only queue: say so and keep the draft.
    pub(crate) fn refuse_queued_images(&mut self) -> bool {
        if self.input.images.is_empty() {
            return false;
        }
        let send = self
            .keys
            .display(crate::keys::KeyAction::Send, self.theme.ascii());
        self.flash(format!(
            "not queued: images cannot wait in the queue ({send} sends them now)"
        ));
        true
    }
}
