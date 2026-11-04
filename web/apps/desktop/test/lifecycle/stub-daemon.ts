// A stub daemon for the process test, run as `aether daemon` from a temp bin directory. It stays
// in the foreground, so the process managed start spawned is the daemon itself. It answers
// hello, records every command it gets, and stops only on SIGTERM from the test that owns it.

import { appendFileSync, rmSync, writeFileSync } from "node:fs";
import net from "node:net";

const socket = process.env.AETHER_SOCKET ?? "";
if (process.argv[2] !== "daemon" || !socket.startsWith("/") || !socket.endsWith(".sock")) {
  process.stderr.write(`stub daemon: run as \`aether daemon\` with AETHER_SOCKET set, not ${process.argv.slice(2).join(" ")}\n`);
  process.exit(2);
}
const commands = socket.replace(/\.sock$/, ".commands");

const server = net.createServer((connection) => {
  let buffered = "";
  connection.on("data", (chunk) => {
    buffered += chunk.toString("utf8");
    for (let newline = buffered.indexOf("\n"); newline !== -1; newline = buffered.indexOf("\n")) {
      const frame = JSON.parse(buffered.slice(0, newline)) as { cmd?: unknown; req?: unknown };
      buffered = buffered.slice(newline + 1);
      appendFileSync(commands, `${String(frame.cmd)}\n`);
      const result = frame.cmd === "hello"
        ? { Ok: { data: { daemon_version: "stub-daemon", protocol_version: 26, pid: process.pid } } }
        : { Err: { code: "unsupported", message: "the stub answers hello only" } };
      connection.write(`${JSON.stringify({ frame: "reply", id: frame.req, result })}\n`);
    }
  });
  connection.on("error", () => connection.destroy());
});

process.on("SIGTERM", () => {
  server.close();
  rmSync(socket, { force: true });
  process.exit(0);
});

server.listen(socket, () => {
  writeFileSync(socket.replace(/\.sock$/, ".pid"), `${process.pid}\n`);
  process.stdout.write(`stub daemon ${process.pid} listening\n`);
});
