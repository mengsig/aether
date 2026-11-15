//! A task's reference images in a model request: one leading user turn that
//! is rebuilt from the task every request, so compaction cannot drop it.

use aether_core::model::{ContentBlock, Message};
use aether_core::task::Task;

/// Opens the leading turn; the assignment lists every reference by path.
pub const REFERENCE_INTRO: &str = "Reference images attached to your task (listed with their paths in your assignment; \
     re-attached to every request, so they survive compaction):";

/// Insert the task's reference images as the first message, ahead of the
/// tail. Returns whether it added one; a model without vision gets none and
/// reads the paths its assignment lists instead.
pub fn attach_reference_images(
    messages: &mut Vec<Message>,
    task: Option<&Task>,
    vision: bool,
) -> bool {
    let images: Vec<ContentBlock> = task
        .into_iter()
        .flat_map(|task| &task.references)
        .filter_map(|reference| reference.image.clone())
        .map(ContentBlock::ImageRef)
        .collect();
    if images.is_empty() || !vision {
        return false;
    }
    debug_assert!(images.len() <= aether_core::reference::MAX_REFERENCE_IMAGES);
    let before = messages.len();
    let mut content = Vec::with_capacity(1 + images.len());
    content.push(ContentBlock::text(REFERENCE_INTRO));
    content.extend(images);
    // Its own turn, never merged into the first: the newest user turn is
    // recorded in the transcript, and these must never be.
    messages.insert(0, Message::user_blocks(content));
    debug_assert_eq!(messages.len(), before + 1);
    true
}

/// Whether `message` is the leading turn [`attach_reference_images`] built,
/// whose images a request's image budget holds room for first.
pub fn is_reference_turn(message: &Message) -> bool {
    message.role == aether_core::model::MessageRole::User
        && matches!(message.content.first(), Some(ContentBlock::Text { text, .. }) if text == REFERENCE_INTRO)
}
