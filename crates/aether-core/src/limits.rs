//! Size limits on free text that ends up in an engine's input line.

/// The line limit of the engine's JSONL protocol (Claude Code refuses a
/// record above 16 MiB and the engine dies with SIGKILL).
pub const ENGINE_LINE_BYTES: usize = 16 * 1024 * 1024;

/// JSON escaping can multiply a byte up to six times (`\u00XX`), and the
/// record wraps the text in more JSON, so a message may use an eighth of a line.
pub const MAX_MESSAGE_BYTES: usize = ENGINE_LINE_BYTES / 8;

const _: () = assert!(MAX_MESSAGE_BYTES * 6 < ENGINE_LINE_BYTES);

/// Refuses `bytes` of `what` (for example "message" or "task objective")
/// above [`MAX_MESSAGE_BYTES`], saying how large it is and what to do.
pub fn check_text_size(what: &str, bytes: usize) -> Result<(), String> {
    if bytes <= MAX_MESSAGE_BYTES {
        return Ok(());
    }
    Err(format!(
        "this {what} is {} KiB, above the {} KiB limit the engine can take in one message: \
         publish the content as an artifact (or write it to a file in the workspace) and send \
         its name instead",
        bytes.div_ceil(1024),
        MAX_MESSAGE_BYTES / 1024
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_limit_is_inclusive_and_the_refusal_is_actionable() {
        assert!(check_text_size("message", MAX_MESSAGE_BYTES).is_ok());
        let refusal = check_text_size("message", MAX_MESSAGE_BYTES + 1).unwrap_err();
        assert!(refusal.contains("2049 KiB") && refusal.contains("2048 KiB"));
        assert!(refusal.contains("artifact"));
    }
}
