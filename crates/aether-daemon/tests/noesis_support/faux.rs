//! A scripted model on loopback: an OpenAI chat-completions server that
//! streams the next scripted reply (text or one tool call) and keeps every
//! request body, so a real daemon runs Noesis with no model or credentials.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

/// What the faux model answers once no scripted reply is left.
pub const FALLBACK_TEXT: &str = "Faux model reply.";
const MAX_BODY: usize = 16 * 1024 * 1024;

/// One scripted reply.
#[derive(Debug, Clone)]
pub enum Reply {
    Text(String),
    /// A visible text, then one call of `name` with `arguments`.
    Tool {
        text: String,
        name: String,
        arguments: Value,
    },
}

#[derive(Default)]
struct State {
    script: VecDeque<Reply>,
    requests: Vec<Value>,
}

pub struct FauxModel {
    pub addr: SocketAddr,
    state: Arc<Mutex<State>>,
    serving: tokio::task::JoinHandle<()>,
}

impl Drop for FauxModel {
    fn drop(&mut self) {
        self.serving.abort();
    }
}

impl FauxModel {
    pub async fn start() -> FauxModel {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        assert!(
            addr.ip().is_loopback(),
            "the faux model listens on loopback only"
        );
        let state = Arc::new(Mutex::new(State::default()));
        let shared = state.clone();
        let serving = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let state = shared.clone();
                tokio::spawn(async move { serve(stream, state).await });
            }
        });
        FauxModel {
            addr,
            state,
            serving,
        }
    }

    pub fn push(&self, reply: Reply) {
        self.state.lock().script.push_back(reply);
    }

    pub fn text(&self, text: &str) {
        self.push(Reply::Text(text.to_owned()));
    }

    pub fn tool(&self, text: &str, name: &str, arguments: Value) {
        assert!(arguments.is_object(), "tool arguments are an object");
        self.push(Reply::Tool {
            text: text.to_owned(),
            name: name.to_owned(),
            arguments,
        });
    }

    /// Every request body received so far, oldest first.
    pub fn requests(&self) -> Vec<Value> {
        self.state.lock().requests.clone()
    }

    /// `providers.toml` naming this server as provider `mock`, model `mock/mock-1`.
    pub fn providers_toml(&self) -> String {
        format!(
            "[providers.mock]\nname = \"Faux model (tests)\"\nprotocol = \"openai-chat-completions\"\n\
             base_url = \"http://{}/v1\"\n[providers.mock.auth]\nnone = true\n\
             [[providers.mock.models]]\nid = \"mock-1\"\nname = \"Faux model\"\n\
             [providers.mock.models.capabilities]\ncontext_window = 32768\n\
             max_output_tokens = 4096\ntools = true\n",
            self.addr
        )
    }
}

/// The tool names a request offered the model.
pub fn offered_tools(request: &Value) -> Vec<String> {
    let tools = request["tools"].as_array().cloned().unwrap_or_default();
    tools
        .iter()
        .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_owned))
        .collect()
}

async fn serve(stream: TcpStream, state: Arc<Mutex<State>>) {
    let mut stream = BufReader::new(stream);
    let Some((method, path, body)) = read_request(&mut stream).await else {
        return;
    };
    let response = match (method.as_str(), path.as_str()) {
        ("GET", "/v1/models") => {
            json_response(&json!({"object": "list", "data": [{"id": "mock-1", "object": "model"}]}))
        }
        ("POST", "/v1/chat/completions") => {
            let request: Value = serde_json::from_slice(&body).expect("a JSON request body");
            let reply = {
                let mut state = state.lock();
                state.requests.push(request.clone());
                state.script.pop_front()
            };
            let reply = reply.unwrap_or_else(|| Reply::Text(FALLBACK_TEXT.into()));
            sse_response(request["model"].as_str().unwrap_or("mock-1"), &reply)
        }
        _ => "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_owned(),
    };
    let stream = stream.get_mut();
    // A client that hung up first is not this server's failure to report.
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

/// The method, path and body of one HTTP/1.1 request; `None` on a closed or malformed one.
async fn read_request(stream: &mut BufReader<TcpStream>) -> Option<(String, String, Vec<u8>)> {
    let mut line = String::new();
    stream.read_line(&mut line).await.ok()?;
    let mut parts = line.split_whitespace();
    let (method, path) = (parts.next()?.to_owned(), parts.next()?.to_owned());
    let mut length = 0usize;
    loop {
        let mut header = String::new();
        if stream.read_line(&mut header).await.ok()? == 0 {
            return None;
        }
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().ok()?;
        }
    }
    assert!(length <= MAX_BODY, "a request body over {MAX_BODY} bytes");
    let mut body = vec![0; length];
    stream.read_exact(&mut body).await.ok()?;
    Some((method, path, body))
}

fn json_response(body: &Value) -> String {
    let body = body.to_string();
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn sse_response(model: &str, reply: &Reply) -> String {
    let chunk = |delta: Value, finish: Option<&str>, usage: Option<Value>| {
        let mut frame = json!({"id": "chatcmpl-faux", "object": "chat.completion.chunk", "created": 0, "model": model,
            "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]});
        if let Some(usage) = usage {
            frame["usage"] = usage;
        }
        format!("data: {frame}\n\n")
    };
    let usage = Some(json!({"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}));
    let mut events = vec![chunk(
        json!({"role": "assistant", "content": ""}),
        None,
        None,
    )];
    let finish = match reply {
        Reply::Text(text) => {
            events.push(chunk(json!({"content": text}), None, None));
            "stop"
        }
        Reply::Tool {
            text,
            name,
            arguments,
        } => {
            if !text.is_empty() {
                events.push(chunk(json!({"content": text}), None, None));
            }
            let call = json!({"index": 0, "id": format!("call_{}", ulid::Ulid::generate()), "type": "function",
                "function": {"name": name, "arguments": arguments.to_string()}});
            events.push(chunk(json!({"tool_calls": [call]}), None, None));
            "tool_calls"
        }
    };
    events.push(chunk(json!({}), Some(finish), usage));
    events.push("data: [DONE]\n\n".to_owned());
    let body = events.concat();
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}
