# MCP servers

Aether connects to the servers configured as `[[mcp.servers]]` in
`config.toml` (see the [configuration template](../crates/aether-model/src/config/templates.rs)).
A server's tools reach agents as `mcp__<server>__<tool>` and go through the
same approvals, idempotency keys and events as built-in tools. Aether-driven
turns call them directly. Native engines (Claude Code, Codex) call them through
Aether's bridge. Both paths end in the same connection.

Each server is a single connection shared by every agent: one process for a
stdio server, one endpoint for an HTTP one.

## Tokens

An HTTP server can name an environment variable that holds its bearer token:

```toml
[[mcp.servers]]
name = "issues"
url = "https://issues.example/mcp"
token_env = "ISSUES_MCP_TOKEN"
```

The daemon reads the variable once, when it starts, and stores no
credential. If the variable is unset or empty, the server fails auth: it is
reported as `auth_failed` and is never contacted, so a typo in the variable's
name cannot quietly turn into anonymous access. Set the variable in the
daemon's environment and restart the daemon; `mcp refresh` cannot help,
because the token is not read again.

A server that also serves anonymous callers can say so:

```toml
auth = "optional"
```

It is then reached without a token when the variable is unset. `auth`
needs a `token_env`.

`aether mcp list` prints an `auth:` line for every server with a token
variable, and `aether doctor` lists the servers that failed auth or connected
without a token. Both name the variable, never its value. Over the protocol,
`McpServerHealth.auth` carries the variable's name and a state: `token`,
`auth_failed` or `anonymous`. A server that failed auth also has
`state: "failed"` and an `error` that starts with `auth_failed`.

The token variable is hidden from every tool child, engine and stdio MCP
server, so an agent's shell cannot read it. A stdio server's `env` may not set
a name the daemon hides (this variable, a provider key, a `[listen]` or
`[serve]` secret): the server then fails to start with an error naming the key.

## Caller identity

A server configured with `send_caller_meta = true` is told which agent each
call is for. Every `tools/call` that Aether sends to it on behalf of an agent
names that agent in the request's `params._meta`, the field the MCP
specification reserves for metadata on every request:

```json
{
  "name": "read_note",
  "arguments": { "key": "k" },
  "_meta": {
    "aether/agent_id": "agt_01…",
    "aether/task_id": "tsk_01…"
  }
}
```

| Key | Present | Value |
| --- | --- | --- |
| `aether/agent_id` | always | the id of the agent whose turn made the call |
| `aether/task_id` | when the agent is working a task | that task's id |

A server can use these keys to tell agents apart without a credential in the
conversation. Passing a token as a tool argument would put it in the model's
context and in Aether's `tool.requested` events.

### How the keys are set

- **Aether sets them, not the model.** They come from the context the tool
  call runs in, which is the same agent and task the call is recorded against.
  This holds whether the call came from an Aether-driven turn or from a native
  engine through the bridge.
- **Arguments are never merged into `_meta`.** The model's arguments are sent
  unchanged under `arguments`. An `_meta` object the model writes inside them
  stays there.
- **A model cannot claim an identity.** Aether refuses a call to an opted-in
  server before sending it if its arguments have a key starting with
  `aether/` (case-insensitive) at the top level or inside an `_meta` object
  of their own. The call fails as invalid input, the server never sees it,
  and the refusal is recorded like any other failed call. A value such as
  `"aether/agent_id"` is still an ordinary argument.
- **Every call carries its own caller.** The keys are built per request, so
  two agents that use the same server at the same time each send their own
  id. A server must read the identity from each request and must not remember
  a "current" agent between requests.

### Trust model

The keys are Aether's statement about who made the call. They are only as
trustworthy as the channel that carries them:

- **stdio:** the server's only peer is the aetherd that spawned it, so on that
  channel the keys carry aetherd's authority. aetherd is the trust anchor.
  Accept them only on that stdin/stdout. Never accept them from another
  channel the same server listens on, such as a socket, an HTTP port, or a
  copy started by some other parent.
- **HTTP:** any client that can reach the endpoint and present its token can
  send any `_meta` it likes. Trust the keys only to the extent that the
  endpoint's token belongs to aetherd alone.

The keys identify a caller. They are not a capability. A server that limits
what each agent may do keeps its own mapping from agent id to permissions.
Agent and task ids are not secrets: they appear in Aether's events, logs and
clients.

Some calls carry no caller keys, so a server that relies on them should
refuse a call without them rather than guess:

- every call to a server without `send_caller_meta = true`;
- calls a native engine makes to MCP servers configured in its own settings,
  because those never pass through Aether;
- calls made with `McpClientHandle`, which drives one server directly on
  behalf of no agent.

### Turning it on

The keys are off by default, per server:

```toml
[[mcp.servers]]
name = "personas"
command = "persona-mcp-server"
send_caller_meta = true
```

Off is the default because some servers reject params they do not know, even
though the specification reserves `_meta` on every request. A server
configured without the flag is sent exactly the requests it was sent before:
no `_meta`, and its arguments untouched, including any `aether/` keys, since
Aether claims no identity to it.

If an opted-in server does reject `_meta`, each call fails with that server's
error. The error is visible to the agent, and the server stays connected.
Remove the flag to restore it.
