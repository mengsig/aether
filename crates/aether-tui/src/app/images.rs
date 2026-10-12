//! Images attached to the draft: how they arrive (a pasted path, `/image`,
//! the clipboard), the list the chips show, and the message they leave with.

use aether_core::attachment::{
    ImageAttachment, MAX_IMAGES_PER_MESSAGE, MAX_MESSAGE_IMAGE_BYTES, human_size,
};

use super::{App, Effect};
use crate::commands::ImageArg;
use crate::image_input::ImageSource;
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

impl Outgoing {
    /// The message as the transcript will show it once the daemon has it.
    pub(crate) fn shown_text(&self) -> String {
        let labels = self.images.iter().map(image_label);
        with_image_lines(&self.text, labels)
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
    }

    /// `Effect::AttachImage` finished.
    pub(crate) fn image_read(&mut self, image: Result<ImageAttachment, String>) {
        match image {
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
        self.input.images.push(image);
        debug_assert!(self.input.images.len() <= MAX_IMAGES_PER_MESSAGE);
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
        vec![Effect::AttachImage(ImageSource::File(path))]
    }

    /// Ask for the image on the system clipboard; it arrives as `ImageRead`.
    pub(crate) fn paste_clipboard_image(&mut self) -> Vec<Effect> {
        if self.input.images.len() >= MAX_IMAGES_PER_MESSAGE {
            self.flash_error(format!(
                "not attached: a message takes at most {MAX_IMAGES_PER_MESSAGE} images"
            ));
            return Vec::new();
        }
        vec![Effect::AttachImage(ImageSource::Clipboard)]
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
