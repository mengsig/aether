// Fixtures for the headless Electron checks: a fake aetherd on a Unix socket that records every
// line, and an HTTPS listener that admits only its own Origin and a SameSite=Strict cookie.

import { execFileSync } from "node:child_process";
import { randomBytes, X509Certificate } from "node:crypto";
import { readFileSync } from "node:fs";
import https from "node:https";
import type { IncomingMessage, ServerResponse } from "node:http";
import net from "node:net";
import path from "node:path";

export interface FakeDaemon {
  readonly socket: string;
  readonly lines: string[];
  close(): Promise<void>;
}

const REMOTE_VIEW = { revision: 7, settings: { enabled: false, pairing: true, public_origins: [], push: false } };

function answer(frame: Record<string, unknown>): unknown {
  switch (frame.cmd) {
    case "hello": return { reply: "welcome", data: { protocol_version: 26 } };
    case "get_remote": return { reply: "remote", data: REMOTE_VIEW };
    default: return { reply: String(frame.cmd), data: { ok: true } };
  }
}

/** A daemon that answers every frame with Ok and keeps the raw lines it received. */
export function fakeDaemon(dir: string): Promise<FakeDaemon> {
  const socket = path.join(dir, "aetherd.sock");
  const lines: string[] = [];
  const open = new Set<net.Socket>();
  const server = net.createServer((connection) => {
    open.add(connection);
    connection.on("close", () => open.delete(connection));
    let buffered = "";
    connection.on("data", (chunk) => {
      buffered += chunk.toString("utf8");
      let newline = buffered.indexOf("\n");
      while (newline !== -1) {
        const line = buffered.slice(0, newline);
        buffered = buffered.slice(newline + 1);
        lines.push(line);
        const frame = JSON.parse(line) as Record<string, unknown>;
        connection.write(`${JSON.stringify({ frame: "reply", id: frame.req, result: { Ok: answer(frame) } })}\n`);
        newline = buffered.indexOf("\n");
      }
    });
  });
  return new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(socket, () => resolve({
      socket, lines, close: () => new Promise((done) => {
        for (const connection of open) connection.destroy();
        server.close(() => done());
      }),
    }));
  });
}

export interface TlsFixture {
  readonly key: Buffer;
  readonly cert: Buffer;
  readonly fingerprint: string;
}

/** A throwaway self-signed certificate for localhost, made with openssl for this run only. */
export function makeCertificate(dir: string): TlsFixture {
  const key = path.join(dir, "key.pem");
  const cert = path.join(dir, "cert.pem");
  execFileSync("openssl", ["req", "-x509", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:prime256v1", "-nodes",
    "-keyout", key, "-out", cert, "-days", "1", "-subj", "/CN=localhost",
    "-addext", "subjectAltName=DNS:localhost,IP:127.0.0.1"], { stdio: "ignore", timeout: 10_000 });
  const pem = readFileSync(cert);
  const fingerprint = `sha256/${Buffer.from(new X509Certificate(pem).fingerprint256.replaceAll(":", ""), "hex").toString("base64")}`;
  return { key: readFileSync(key), cert: pem, fingerprint };
}

export interface RemoteServer {
  readonly origin: string;
  readonly port: number;
  /** Every request seen: method, Host header and path. */
  readonly seen: Array<{ method: string; host: string; path: string }>;
  readonly reports: unknown[];
  authenticated: boolean;
  close(): Promise<void>;
}

const PAGE = `<!doctype html><title>remote</title><script src="/app.js"></script>`;
const SCRIPT = `
(async () => {
  const report = (body) => fetch("/api/report", { method: "POST", body: JSON.stringify(body) });
  await report({ desktop: typeof window.desktop, require: typeof require, process: typeof process });
  const login = await fetch("/api/login", { method: "POST" });
  const me = await fetch("/api/me", { method: "POST" });
  await report({ login: login.status, me: me.status });
})();`;

function cookie(request: IncomingMessage): string | undefined {
  return /(?:^|; )session=([0-9a-f]+)/.exec(request.headers.cookie ?? "")?.[1];
}

/** The listener's rules: its own Origin on every POST, a same-origin fetch, the session cookie. */
function route(server: RemoteServer, session: string, request: IncomingMessage, response: ServerResponse, body: string): void {
  const sameOrigin = request.headers.origin === server.origin && request.headers["sec-fetch-site"] === "same-origin";
  const send = (status: number, type: string, text: string, headers: Record<string, string> = {}) => {
    response.writeHead(status, { "content-type": type, ...headers });
    response.end(text);
  };
  if (request.method === "GET" && request.url === "/") return send(200, "text/html", PAGE);
  if (request.method === "GET" && request.url === "/app.js") return send(200, "text/javascript", SCRIPT);
  if (request.url === "/redirect") return send(302, "text/plain", "", { location: `https://127.0.0.1:${server.port}/landing` });
  if (request.method !== "POST" || !sameOrigin) return send(403, "text/plain", "forbidden");
  if (request.url === "/api/report") {
    server.reports.push(JSON.parse(body));
    return send(204, "text/plain", "");
  }
  if (request.url === "/api/login") {
    return send(204, "text/plain", "", { "set-cookie": `session=${session}; Path=/; Secure; HttpOnly; SameSite=Strict` });
  }
  if (request.url === "/api/me" && cookie(request) === session) {
    server.authenticated = true;
    return send(200, "application/json", "{}");
  }
  return send(401, "text/plain", "unauthenticated");
}

export function remoteServer(tls: TlsFixture): Promise<RemoteServer> {
  const session = randomBytes(16).toString("hex");
  const state: RemoteServer = {
    origin: "", port: 0, seen: [], reports: [], authenticated: false, close: async () => {},
  };
  const server = https.createServer({ key: tls.key, cert: tls.cert }, (request, response) => {
    state.seen.push({ method: request.method ?? "", host: request.headers.host ?? "", path: request.url ?? "" });
    let body = "";
    request.on("data", (chunk: Buffer) => { body += chunk.toString("utf8"); });
    request.on("end", () => route(state, session, request, response, body));
  });
  return new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      if (address === null || typeof address === "string") throw new Error("the fixture listens on a TCP port");
      Object.assign(state, {
        port: address.port,
        origin: `https://localhost:${address.port}`,
        close: () => new Promise<void>((done) => { server.closeAllConnections(); server.close(() => done()); }),
      });
      resolve(state);
    });
  });
}
