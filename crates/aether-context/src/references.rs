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
    if !has_reference_turn(task, vision) {
        return false;
    }
    debug_assert!(!images.is_empty());
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

/// Whether a request for `task` on a model with `vision` leads with the
/// reference turn: the one test both its builder and the image budget use.
pub fn has_reference_turn(task: Option<&Task>, vision: bool) -> bool {
    vision
        && task.is_some_and(|task| {
            task.references
                .iter()
                .any(|reference| reference.image.is_some())
        })
}
