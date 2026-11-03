// A scripted daemon behind a Transport, for tests: it answers each command
// through a handler, replays recorded frames, and drops links on demand.

import type { Transport } from "@aether-ui/protocol";
import type { Json } from "../guards.ts";

export type Answer = { ok: Json; then?: readonly Json[] } | { err: Json } | { frames: readonly Json[]; close?: number };
export type Handler = (command: Json, daemon: FakeDaemon) => Answer | undefined;

interface LiveLink { open: boolean; onFrame(frame: unknown): void; onClose(code: number, reason: string): void }

export class FakeDaemon {
  readonly sent: Json[] = [];
  opens = 0;
  handler: Handler;
  private link: LiveLink | null = null;
  readonly transport: Transport;

  constructor(handler: Handler, id: Transport["id"] = "ws-aether") {
    this.handler = handler;
    this.transport = {
      id,
      open: async (_target, onFrame, onClose) => {
        this.opens += 1;
        const link: LiveLink = { open: true, onFrame, onClose };
        this.link = link;
        return { send: (frame) => this.receive(link, frame as Json), close: () => { link.open = false; } };
      },
    };
  }

  commands(cmd: string): Json[] {
    return this.sent.filter((command) => command.cmd === cmd);
  }

  push(frame: Json): void {
    if (this.link?.open) this.link.onFrame(structuredClone(frame));
  }

  drop(code = 1006, reason = "connection lost"): void {
    const link = this.link;
    this.link = null;
    if (!link?.open) return;
    link.open = false;
    link.onClose(code, reason);
  }

  private receive(link: LiveLink, command: Json): void {
    this.sent.push(command);
    const answer = this.handler(command, this);
    if (!answer) return;
    // Replies arrive on a later turn, as they would from a socket.
    void Promise.resolve().then(() => {
      if (!link.open) return;
      if ("frames" in answer) {
        for (const frame of answer.frames) link.onFrame(structuredClone(frame));
        if (answer.close !== undefined) this.drop(answer.close, "closed by the daemon");
        return;
      }
      const result = "err" in answer ? { Err: answer.err } : { Ok: answer.ok };
      link.onFrame({ frame: "reply", id: command.req, result: structuredClone(result) });
      for (const frame of "then" in answer ? answer.then ?? [] : []) link.onFrame(structuredClone(frame));
    });
  }
}
