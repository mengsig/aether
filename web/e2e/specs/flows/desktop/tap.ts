// A test-only tap on a page's WebSockets, installed before its scripts run: it keeps every socket
// core opens, so a spec can send frames over the page's own authenticated connection. Replies to
// those frames (ids from PROBE_BASE up) are kept for the spec and never reach core.

import assert from "node:assert/strict";
import type { Page } from "@playwright/test";

const PROBE_BASE = 1_000_000_000;
const PROBE_MS = 20_000;

/** A daemon's reply to one probe: `Ok` data, or the refusal's code and message. */
export interface ProbeReply { readonly cmd: string; readonly ok: boolean; readonly code?: string; readonly message?: string }

interface TapState { sockets: WebSocket[]; replies: Record<number, { result?: { Ok?: unknown; Err?: { code?: string; message?: string } } }> }

/** Runs in the page, so it is self-contained. */
function install(base: number): void {
  const Native = window.WebSocket;
  const tap: TapState = { sockets: [], replies: {} };
  (window as unknown as { e2eTap: TapState }).e2eTap = tap;
  const probeReply = (data: unknown): { id: number } | null => {
    if (typeof data !== "string" || !data.includes("\"reply\"")) return null;
    const frame = JSON.parse(data) as { frame?: string; id?: number };
    return frame.frame === "reply" && typeof frame.id === "number" && frame.id >= base ? { id: frame.id } : null;
  };
  class Tapped extends Native {
    constructor(url: string | URL, protocols?: string | string[]) {
      super(url, protocols);
      tap.sockets.push(this);
      this.addEventListener("message", (event) => {
        const probe = probeReply(event.data);
        if (probe !== null) tap.replies[probe.id] = JSON.parse(event.data as string) as TapState["replies"][number];
      });
    }

    override set onmessage(handler: ((this: WebSocket, event: MessageEvent) => unknown) | null) {
      super.onmessage = handler === null ? null : function filtered(this: WebSocket, event: MessageEvent) {
        if (probeReply(event.data) === null) handler.call(this, event);
      };
    }

    override get onmessage(): ((this: WebSocket, event: MessageEvent) => unknown) | null {
      return super.onmessage;
    }
  }
  window.WebSocket = Tapped;
}

export async function installTap(page: Page): Promise<void> {
  await page.addInitScript(install, PROBE_BASE);
}

/**
 * Sends each frame, in order, on the page's newest open socket of `protocol` ("aether" or
 * "archon"), and returns each reply. Fails if any reply is missing.
 */
export async function probe(page: Page, protocol: "aether" | "archon", frames: ReadonlyArray<Readonly<Record<string, unknown>>>, batch: number): Promise<ProbeReply[]> {
  assert.ok(frames.length > 0 && frames.every((frame) => typeof frame.cmd === "string"), "every probe names its command");
  assert.ok(Number.isInteger(batch) && batch >= 0 && batch < 1000, "a batch number keeps probe ids apart");
  const first = PROBE_BASE + batch * 1000;
  const answers = await page.evaluate(async ({ protocol: wanted, frames: sent, first: id, timeout }) => {
    const tap = (window as unknown as { e2eTap?: TapState }).e2eTap;
    if (tap === undefined) throw new Error("the WebSocket tap is not installed");
    const socket = tap.sockets.filter((candidate) => candidate.protocol === wanted && candidate.readyState === WebSocket.OPEN).at(-1);
    if (socket === undefined) throw new Error(`no open ${wanted} socket among ${tap.sockets.length}`);
    sent.forEach((frame, index) => socket.send(JSON.stringify({ ...frame, req: id + index })));
    const end = Date.now() + timeout;
    while (Date.now() < end && sent.some((_frame, index) => tap.replies[id + index] === undefined)) await new Promise((resolve) => setTimeout(resolve, 50));
    return sent.map((_frame, index) => tap.replies[id + index] ?? null);
  }, { protocol, frames, first, timeout: PROBE_MS });
  return answers.map((answer, index) => {
    const cmd = String((frames[index] as { cmd: string }).cmd);
    assert.ok(answer !== null, `${protocol} answered the ${cmd} probe within ${PROBE_MS} ms`);
    const err = answer.result?.Err;
    if (err === undefined) return { cmd, ok: true };
    return { cmd, ok: false, ...(err.code === undefined ? {} : { code: err.code }), ...(err.message === undefined ? {} : { message: err.message }) };
  });
}
