// The faux engine: an OpenAI chat-completions server on loopback that streams a scripted reply,
// so agents run with no model and no credentials. Every request is kept for assertions.

import assert from "node:assert/strict";
import http from "node:http";
import type { AddressInfo } from "node:net";

export interface EngineRequest {
  readonly model: string;
  readonly lastUserText: string;
  /** Every user message's text, oldest first. */
  readonly userTexts: readonly string[];
  /** The tools the model already called in this conversation, oldest first. */
  readonly toolsCalled: readonly string[];
}

export interface ToolReply {
  readonly name: string;
  readonly arguments: Record<string, unknown>;
  readonly text?: string;
}

/** A text reply or one tool call; the default names the faux engine. */
export type Script = (request: EngineRequest) => string | ToolReply;

export interface FauxEngine {
  /** `http://127.0.0.1:<port>/v1`, the provider's `base_url`. */
  readonly baseUrl: string;
  readonly requests: readonly EngineRequest[];
  setScript(script: Script): void;
  close(): Promise<void>;
}

export const DEFAULT_REPLY = "Faux engine reply.";
const MAX_BODY_BYTES = 16 * 1024 * 1024;

function userTexts(messages: unknown): string[] {
  if (!Array.isArray(messages)) return [];
  return (messages as { role?: unknown }[]).filter((message) => message.role === "user").map((message) => lastUserText([message]));
}

function toolsCalled(messages: unknown): string[] {
  if (!Array.isArray(messages)) return [];
  return (messages as { role?: unknown; tool_calls?: unknown }[])
    .filter((message) => message.role === "assistant" && Array.isArray(message.tool_calls))
    .flatMap((message) => (message.tool_calls as { function?: { name?: unknown } }[]).map((call) => String(call.function?.name ?? "")));
}

function lastUserText(messages: unknown): string {
  if (!Array.isArray(messages)) return "";
  for (const message of [...messages].reverse() as { role?: unknown; content?: unknown }[]) {
    if (message.role !== "user") continue;
    if (typeof message.content === "string") return message.content;
    if (Array.isArray(message.content)) {
      return (message.content as { type?: unknown; text?: unknown }[])
        .filter((part) => part.type === "text" && typeof part.text === "string").map((part) => String(part.text)).join(" ");
    }
  }
  return "";
}

async function readJson(request: http.IncomingMessage): Promise<Record<string, unknown>> {
  const chunks: Buffer[] = [];
  let size = 0;
  for await (const chunk of request) {
    size += (chunk as Buffer).length;
    if (size > MAX_BODY_BYTES) throw new Error(`a request body over ${MAX_BODY_BYTES} bytes`);
    chunks.push(chunk as Buffer);
  }
  const parsed: unknown = JSON.parse(Buffer.concat(chunks).toString("utf8") || "{}");
  assert.ok(parsed !== null && typeof parsed === "object" && !Array.isArray(parsed), "a request body is a JSON object");
  return parsed as Record<string, unknown>;
}

function streamReply(response: http.ServerResponse, model: string, reply: string | ToolReply): void {
  const text = typeof reply === "string" ? reply : reply.text ?? "";
  assert.ok(model.length > 0, "a streamed reply names its model");
  assert.ok(typeof reply === "string" || reply.name.trim().length > 0, "a tool call has a name");
  const id = `chatcmpl-faux-${Date.now()}`;
  const chunk = (delta: Record<string, unknown>, finish: string | null, usage?: Record<string, number>) =>
    `data: ${JSON.stringify({ id, object: "chat.completion.chunk", created: Math.floor(Date.now() / 1000), model,
      choices: [{ index: 0, delta, finish_reason: finish }], ...(usage ? { usage } : {}) })}\n\n`;
  response.writeHead(200, { "Content-Type": "text/event-stream", "Cache-Control": "no-cache" });
  response.write(chunk({ role: "assistant", content: "" }, null));
  for (let at = 0; at < text.length; at += 8) response.write(chunk({ content: text.slice(at, at + 8) }, null));
  if (typeof reply !== "string") {
    response.write(chunk({ tool_calls: [{ index: 0, id: `call_${crypto.randomUUID()}`, type: "function",
      function: { name: reply.name, arguments: JSON.stringify(reply.arguments) } }] }, null));
  }
  const tokens = Math.max(1, Math.ceil(text.length / 4));
  response.write(chunk({}, typeof reply === "string" ? "stop" : "tool_calls", { prompt_tokens: 10, completion_tokens: tokens, total_tokens: 10 + tokens }));
  response.end("data: [DONE]\n\n");
}

async function handle(request: http.IncomingMessage, response: http.ServerResponse, state: { script: Script; requests: EngineRequest[] }) {
  const url = new URL(request.url ?? "/", "http://127.0.0.1");
  if (request.method === "GET" && url.pathname === "/v1/models") {
    response.writeHead(200, { "Content-Type": "application/json" }).end(JSON.stringify({ object: "list", data: [{ id: "mock-1", object: "model" }] }));
    return;
  }
  if (request.method !== "POST" || url.pathname !== "/v1/chat/completions") {
    response.writeHead(404).end();
    return;
  }
  const body = await readJson(request);
  const seen: EngineRequest = {
    model: String(body.model ?? ""), lastUserText: lastUserText(body.messages),
    userTexts: userTexts(body.messages), toolsCalled: toolsCalled(body.messages),
  };
  state.requests.push(seen);
  streamReply(response, seen.model, state.script(seen));
}

/** Listens on a free loopback port; only loopback, so a request from anywhere else cannot reach it. */
export async function startEngine(): Promise<FauxEngine> {
  const state = { script: (() => DEFAULT_REPLY) as Script, requests: [] as EngineRequest[] };
  const server = http.createServer((request, response) => {
    handle(request, response, state).catch((error: unknown) => {
      if (!response.headersSent) response.writeHead(500, { "Content-Type": "text/plain" });
      response.end(`faux engine: ${error instanceof Error ? error.message : String(error)}`);
    });
  });
  await new Promise<void>((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
  const address = server.address() as AddressInfo;
  assert.equal(address.address, "127.0.0.1", "the faux engine listens on loopback only");
  return {
    baseUrl: `http://127.0.0.1:${address.port}/v1`,
    requests: state.requests,
    setScript(script) { state.script = script; },
    close: () => new Promise<void>((resolve, reject) => {
      server.closeAllConnections();
      server.close((error) => (error ? reject(error) : resolve()));
    }),
  };
}

/** `providers.toml` naming the faux engine as provider `mock`, model `mock/mock-1`. */
export function providersToml(engine: FauxEngine): string {
  assert.match(engine.baseUrl, /^http:\/\/127\.0\.0\.1:\d+\/v1$/, "the provider points at loopback");
  return [
    "[providers.mock]", 'name = "Faux engine (e2e)"', 'protocol = "openai-chat-completions"', `base_url = "${engine.baseUrl}"`,
    "[providers.mock.auth]", "none = true",
    "[[providers.mock.models]]", 'id = "mock-1"', 'name = "Faux model"',
    "[providers.mock.models.capabilities]", "context_window = 32768", "max_output_tokens = 4096", "tools = true", "",
  ].join("\n");
}
