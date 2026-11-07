// One request over a daemon's Unix socket, as its CLI makes it: a hello, then the command.
// Aether and Archon frame alike: `{req, cmd, ...}` out, `{frame: "reply", id, result}` back.

import net from "node:net";
import { createInterface } from "node:readline";

export type Reply = { readonly reply: string; readonly data: Record<string, unknown> };
export type Command = Readonly<Record<string, unknown>> & { readonly cmd: string };

const REQUEST_MS = 15_000;

interface ReplyFrame { frame?: string; id?: number; result?: { Ok?: Reply; Err?: unknown } }

/** Sends `hello`, then `command`; returns both replies. A refused hello or command throws. */
export async function socketRequest(socketPath: string, hello: Command, command: Command): Promise<{ welcome: Reply; reply: Reply }> {
  if (hello.cmd !== "hello") throw new Error(`a connection opens with hello, not ${hello.cmd}`);
  if (Buffer.byteLength(socketPath) > 107) throw new Error(`socket path over the Unix limit: ${socketPath}`);
  const socket = net.createConnection(socketPath);
  const lines = createInterface({ input: socket });
  const timer = setTimeout(() => socket.destroy(new Error(`${command.cmd} on ${socketPath} timed out`)), REQUEST_MS);
  let welcome: Reply | undefined;
  try {
    await new Promise((resolve, reject) => { socket.once("connect", resolve); socket.once("error", reject); });
    socket.write(`${JSON.stringify({ req: 1, ...hello })}\n`);
    for await (const line of lines) {
      const frame = JSON.parse(line) as ReplyFrame;
      if (frame.frame !== "reply") continue;
      if (frame.result?.Err !== undefined) throw new Error(`${frame.id === 1 ? "hello" : command.cmd} refused: ${JSON.stringify(frame.result.Err)}`);
      const ok = frame.result?.Ok;
      if (ok === undefined) throw new Error(`a reply frame without a result: ${line}`);
      if (frame.id === 1) {
        welcome = ok;
        socket.write(`${JSON.stringify({ req: 2, ...command })}\n`);
      } else if (frame.id === 2 && welcome !== undefined) {
        return { welcome, reply: ok };
      }
    }
    throw new Error(`${socketPath} closed before answering ${command.cmd}`);
  } finally {
    clearTimeout(timer);
    lines.close();
    socket.destroy();
  }
}
