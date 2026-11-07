// A loopback HTTPS front for aetherd's listener, standing in for `tailscale serve`: it ends TLS
// with a throwaway certificate and pipes every byte, HTTP and WebSocket alike, to the listener,
// so Host, Origin and cookies arrive exactly as the window sent them.

import assert from "node:assert/strict";
import net from "node:net";
import tls from "node:tls";
import { makeCertificate } from "../../../../apps/desktop/test/security/electron/fixtures.ts";

export interface TlsFront {
  /** `https://127.0.0.1:<port>`, as a browser serializes it. */
  readonly origin: string;
  /** The certificate's `sha256/<base64>` fingerprint, as Electron reports it. */
  readonly fingerprint: string;
  /** Client connections the front accepted so far. */
  readonly accepted: () => number;
  close(): Promise<void>;
}

function pipeTo(upstreamPort: number, client: tls.TLSSocket, open: Set<net.Socket>): void {
  const upstream = net.connect(upstreamPort, "127.0.0.1");
  for (const socket of [client, upstream]) {
    open.add(socket);
    socket.once("close", () => open.delete(socket));
  }
  // A daemon restart ends the upstream; the client then sees its connection close, as behind a proxy.
  client.once("error", () => upstream.destroy());
  upstream.once("error", () => client.destroy());
  client.once("close", () => upstream.destroy());
  upstream.once("close", () => client.destroy());
  client.pipe(upstream);
  upstream.pipe(client);
}

/** Listens on 127.0.0.1 only; `dir` receives the key and certificate. */
export async function startFront(dir: string, upstreamPort: number): Promise<TlsFront> {
  assert.ok(Number.isInteger(upstreamPort) && upstreamPort > 0, "the front forwards to a TCP port");
  const certificate = makeCertificate(dir);
  assert.match(certificate.fingerprint, /^sha256\/[A-Za-z0-9+/=]+$/, "the certificate has a sha256 fingerprint");
  const open = new Set<net.Socket>();
  let accepted = 0;
  const server = tls.createServer({ key: certificate.key, cert: certificate.cert }, (client) => {
    accepted += 1;
    pipeTo(upstreamPort, client, open);
  });
  await new Promise<void>((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => resolve());
  });
  const address = server.address();
  assert.ok(address !== null && typeof address === "object" && address.address === "127.0.0.1", "the front listens on loopback");
  return {
    origin: `https://127.0.0.1:${address.port}`,
    fingerprint: certificate.fingerprint,
    accepted: () => accepted,
    close: () => new Promise<void>((resolve) => {
      for (const socket of open) socket.destroy();
      server.close(() => resolve());
    }),
  };
}
