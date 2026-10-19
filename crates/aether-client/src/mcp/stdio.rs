//! Serve MCP over a pair of streams: newline-delimited JSON-RPC, one message
//! per line. On the daemon's socket this is stdin and stdout, and nothing
//! else may be written to stdout.
//!
//! The peer here is whoever started this process, so it holds the operator's
//! surface. It is still read under a cap: every other read from a peer in
//! Aether is bounded, and a peer that never sends a newline would otherwise
//! grow the process without limit.

use tokio::io::{
    AsyncBufRead, AsyncBufReadExt as _, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _,
};

use serde_json::{Value, json};

use super::{Answer, CommandSink, Surface};

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
/// MCP's code for a request that arrives before `initialize`.
const NOT_INITIALIZED: i64 = -32002;

/// Longest request line accepted. A JSON-RPC call is small; a longer one is
/// a mistake or an attack, and reading it would be the vulnerability.
pub const MAX_LINE_BYTES: usize = 1 << 20;

/// Answer requests until the peer closes the stream.
pub async fn serve(
    sink: &dyn CommandSink,
    reader: impl AsyncBufRead + Unpin,
    mut writer: impl AsyncWrite + Unpin,
) -> anyhow::Result<()> {
    let mut initialized = false;
    // Capped rather than `lines()`, which reads to the next newline however
    // far away that is.
    let mut lines = reader.take(MAX_LINE_BYTES as u64 + 1).lines();
    while let Some(line) = lines.next_line().await? {
        if line.len() > MAX_LINE_BYTES {
            anyhow::bail!("a request line passed the {MAX_LINE_BYTES}-byte cap");
        }
        // `take` bounds the whole stream, so the budget is restored per line.
        lines.get_mut().set_limit(MAX_LINE_BYTES as u64 + 1);
        if line.trim().is_empty() {
            continue;
        }
        let request: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "mcp server: malformed request");
                write_line(&mut writer, &error_reply(PARSE_ERROR, "parse error")).await?;
                continue;
            }
        };
        if let Some(refusal) = refuse(&request, initialized) {
            write_line(&mut writer, &refusal).await?;
            continue;
        }
        let response = match super::handle(sink, &Surface::Fleet, &request).await {
            Answer::Reply(response) => response,
            Answer::None => continue, // a notification takes no reply
            // The operator's surface offers every tool it accepts, so this
            // is unreachable; refusing loudly beats answering as if it ran.
            Answer::Denied(why) => anyhow::bail!("the operator surface refused a call: {why}"),
        };
        initialized |= request["method"] == "initialize";
        write_line(&mut writer, &response).await?;
    }
    Ok(())
}

/// The JSON-RPC error that answers `request` instead of running it, if any:
/// a batch or a non-object is not a request, and nothing but `initialize`
/// and `ping` is served before the handshake. A notification is never refused.
fn refuse(request: &Value, initialized: bool) -> Option<Value> {
    let Some(object) = request.as_object() else {
        let what = if request.is_array() {
            "batches are not supported; send one request per line"
        } else {
            "a request must be a JSON object"
        };
        return Some(error_reply(INVALID_REQUEST, what));
    };
    let method = object.get("method").and_then(Value::as_str).unwrap_or("");
    let id = object.get("id")?;
    if initialized || matches!(method, "initialize" | "ping") {
        return None;
    }
    let mut reply = error_reply(
        NOT_INITIALIZED,
        "send `initialize` before any other request",
    );
    reply["id"] = id.clone();
    Some(reply)
}

fn error_reply(code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": null, "error": {"code": code, "message": message}})
}

async fn write_line(writer: &mut (impl AsyncWrite + Unpin), value: &Value) -> anyhow::Result<()> {
    let mut text = serde_json::to_string(value)?;
    text.push('\n');
    writer.write_all(text.as_bytes()).await?;
    writer.flush().await?;
    Ok(())
}
