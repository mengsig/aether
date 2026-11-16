//! A task's reference images in a model request: one leading user turn that
//! is rebuilt from the task every request, so compaction cannot drop it.

use aether_core::model::{ContentBlock, Message};
use aether_core::task::Task;

/// Opens the leading turn; the assignment lists every reference by path.
pub const REFERENCE_INTRO: &str = "Reference images attached to your task (listed with their paths in your assignment; \
     re-attached to every request, so they survive compaction):";

/// Insert the task's reference images as their own user turn: first, or
/// right after the latest signed compaction block, since a provider replays
/// nothing before that block. Returns the turn's index; a model without
/// vision gets none and reads the paths its assignment lists instead.
pub fn attach_reference_images(
    messages: &mut Vec<Message>,
    task: Option<&Task>,
    vision: bool,
) -> Option<usize> {
    if !vision {
        return None;
    }
    let images: Vec<ContentBlock> = task
        .into_iter()
        .flat_map(|task| &task.references)
        .filter_map(|reference| reference.image.clone())
        .map(ContentBlock::ImageRef)
        .collect();
    if images.is_empty() {
        return None;
    }
    debug_assert!(images.len() <= aether_core::reference::MAX_REFERENCE_IMAGES);
    let at = messages
        .iter()
        .rposition(|message| {
            message
                .content
                .iter()
                .any(ContentBlock::is_signed_compaction)
        })
        .map_or(0, |compacted| compacted + 1);
    // The tail ends with a user turn and a compaction block is the model's,
    // so the reference turn never becomes the newest message.
    debug_assert!(at == 0 || at < messages.len());
    let before = messages.len();
    let mut content = Vec::with_capacity(1 + images.len());
    content.push(ContentBlock::text(REFERENCE_INTRO));
    content.extend(images);
    // Its own turn, never merged into another: the newest user turn is
    // recorded in the transcript, and these must never be.
    messages.insert(at, Message::user_blocks(content));
    debug_assert_eq!(messages.len(), before + 1);
    Some(at)
}
