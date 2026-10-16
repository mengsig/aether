//! Account RPCs have no thread or turn and own a short-lived, cancellable server.

use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::EngineError;
use crate::output::BoundedLines;

mod process;
#[cfg(test)]
mod tests;

const LIFECYCLE_TIMEOUT: Duration = Duration::from_secs(10);

pub(super) async fn read(
    program: &str,
    hidden_env: &[String],
    cancel: CancellationToken,
) -> Result<Value, EngineError> {
    read_with_timeout(program, hidden_env, cancel, LIFECYCLE_TIMEOUT).await
}

async fn read_with_timeout(
    program: &str,
    hidden_env: &[String],
    cancel: CancellationToken,
    timeout: Duration,
) -> Result<Value, EngineError> {
    assert!(
        !timeout.is_zero(),
        "a usage read needs a finite lifecycle budget"
    );
    let deadline = Instant::now() + timeout;
    let request_deadline = deadline - timeout / 5;
    assert!(request_deadline < deadline, "reserve time for reaping");
    let stop = cancel.child_token();
    let _cancel_on_drop = stop.clone().drop_guard();
    let program = program.to_owned();
    let hidden_env = hidden_env.to_vec();
    // This owner survives a dropped read future until it has killed and reaped the server.
    tokio::spawn(
        async move { supervise(&program, &hidden_env, stop, request_deadline, deadline).await },
    )
    .await
    .map_err(|error| failure(format!("lifecycle task failed: {error}")))?
}

async fn supervise(
    program: &str,
    hidden_env: &[String],
    cancel: CancellationToken,
    request_deadline: Instant,
    deadline: Instant,
) -> Result<Value, EngineError> {
    if cancel.is_cancelled() {
        return Err(failure("cancelled"));
    }
    if Instant::now() >= request_deadline {
        return Err(failure("timed out before launch"));
    }
    let mut process = process::UsageProcess::spawn(program, hidden_env)?;
    let result = tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(failure("cancelled")),
        _ = tokio::time::sleep_until(request_deadline) => Err(failure("timed out")),
        result = exchange(&mut process.child) => result,
    };
    let cleanup_deadline = deadline.min(Instant::now() + (deadline - request_deadline));
    let cleanup = process.finish(cleanup_deadline).await;
    match (result, cleanup) {
        (result, Ok(())) => result,
        (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(cleanup)) => Err(failure(format!("{error}; {cleanup}"))),
    }
}

async fn exchange(child: &mut Child) -> Result<Value, EngineError> {
    let mut stdin = child.stdin.take().ok_or_else(|| failure("missing stdin"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| failure("missing stdout"))?;
    let mut lines = BoundedLines::new(stdout);
    request(&mut stdin, 1, "initialize", super::driver::init_params()).await?;
    response(&mut lines, 1).await?;
    write(
        &mut stdin,
        json!({"jsonrpc": "2.0", "method": "initialized"}),
    )
    .await?;
    request(&mut stdin, 2, "account/rateLimits/read", json!({})).await?;
    let result = response(&mut lines, 2).await?;
    let payload = result
        .get("rateLimits")
        .filter(|value| value.is_object())
        .ok_or_else(|| failure("account/rateLimits/read returned no rateLimits object"))?;
    if aether_core::provider::ProviderUsage::from_engine_payload(payload, aether_core::time::now())
        .is_none()
    {
        return Err(failure(
            "account/rateLimits/read returned no usable quota windows",
        ));
    }
    Ok(payload.clone())
}

async fn request(
    stdin: &mut ChildStdin,
    id: u64,
    method: &str,
    params: Value,
) -> Result<(), EngineError> {
    assert!(id > 0, "usage RPC ids start at one");
    assert!(!method.is_empty(), "usage RPCs require a method");
    write(
        stdin,
        json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}),
    )
    .await
}

async fn write(stdin: &mut ChildStdin, message: Value) -> Result<(), EngineError> {
    let mut line = message.to_string();
    line.push('\n');
    stdin
        .write_all(line.as_bytes())
        .await
        .map_err(|error| failure(format!("writing RPC: {error}")))?;
    stdin
        .flush()
        .await
        .map_err(|error| failure(format!("flushing RPC: {error}")))
}

async fn response(lines: &mut BoundedLines<ChildStdout>, id: u64) -> Result<Value, EngineError> {
    loop {
        let line = lines
            .next_line()
            .await
            .map_err(|error| failure(format!("reading RPC {id}: {error}")))?
            .ok_or_else(|| failure(format!("server closed stdout before RPC {id} replied")))?;
        let message: Value = serde_json::from_str(&line)
            .map_err(|error| failure(format!("invalid RPC JSON: {error}")))?;
        if !message.is_object() {
            return Err(failure("RPC response is not an object"));
        }
        if message.get("id").is_none() && message.get("method").is_some_and(Value::is_string) {
            continue;
        }
        if message.get("method").is_some() || message.get("id") != Some(&json!(id)) {
            return Err(failure(format!(
                "unexpected RPC while waiting for response {id}"
            )));
        }
        if let Some(error) = message.get("error") {
            return Err(failure(format!("RPC {id} failed: {error}")));
        }
        return message
            .get("result")
            .filter(|result| result.is_object())
            .cloned()
            .ok_or_else(|| failure(format!("RPC {id} returned no result object")));
    }
}

fn failure(detail: impl std::fmt::Display) -> EngineError {
    EngineError::Exited(format!("codex usage read: {detail}"))
}
