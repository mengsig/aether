// Pairing against a real aetherd: the claim endpoint's refusals and guess limit over HTTP, then a
// browser that opens the printed pairing URL in the new app and gets an authenticated socket.

import assert from "node:assert/strict";
import { createECDH } from "node:crypto";
import { setTimeout as delay } from "node:timers/promises";
import type { Browser } from "@playwright/test";
import { until, type Daemon } from "./daemon.ts";

interface Pairing { readonly id: string; readonly code: string; readonly url: string }
interface Claim { readonly status: number; readonly retryAfter: string | null; readonly body: { pairing?: string; error?: { code: string } } }

const CODE = /^[0-9A-HJKMNP-TV-Z]{4}-[0-9A-HJKMNP-TV-Z]{4}$/;

async function startPairing(daemon: Daemon): Promise<Pairing> {
  const { reply, data } = await daemon.request({ cmd: "start_pairing", role: "observer" });
  assert.equal(reply, "pairing");
  assert.equal(data.state, "open");
  assert.match(String(data.code), CODE);
  return { id: String(data.id), code: String(data.code), url: String(data.url) };
}

async function state(daemon: Daemon, id: string): Promise<Record<string, unknown>> {
  return (await daemon.request({ cmd: "get_pairing", id })).data;
}

async function claim(daemon: Daemon, publicKey: string, code: string, name: string): Promise<Claim> {
  const response = await fetch(`${daemon.url}/api/pair/claim`, {
    method: "POST", headers: { "Content-Type": "application/json", Origin: daemon.url },
    body: JSON.stringify({ code, public_key: publicKey, device_name: name }), signal: AbortSignal.timeout(10_000),
  });
  return { status: response.status, retryAfter: response.headers.get("retry-after"), body: await response.json() as Claim["body"] };
}

function publicKey(): string {
  const key = createECDH("prime256v1");
  key.generateKeys();
  const encoded = key.getPublicKey().toString("base64url");
  assert.match(encoded, /^[A-Za-z0-9_-]{87}$/);
  return encoded;
}

/** Names the daemon must refuse, the code untouched; then wrong codes until the rate limit answers. */
export async function checkClaims(daemon: Daemon): Promise<void> {
  const key = publicKey();
  const pairing = await startPairing(daemon);
  try {
    for (const name of ["", "   ", "x".repeat(65), "Phone​name", "Phone‮name", "Phone﻿name"]) {
      const refused = await claim(daemon, key, pairing.code, name);
      assert.equal(refused.status, 400, `name ${JSON.stringify(name)}`);
      assert.equal(refused.body.error?.code, "bad_request");
    }
    const wrong = pairing.code === "0000-0000" ? "1111-1111" : "0000-0000";
    const burst = await Promise.all(Array.from({ length: 16 }, () => claim(daemon, key, wrong, "Security phone")));
    assert.ok(burst.some((reply) => reply.status === 429), "a burst of wrong codes is rate limited");
    for (const reply of burst) {
      assert.equal(reply.body.error?.code, reply.status === 429 ? "rate_limited" : "invalid_code");
      if (reply.status === 429) assert.equal(reply.retryAfter, "1");
    }
    assert.equal((await state(daemon, pairing.id)).state, "open", "no guess claimed the pairing");
    await delay(1100);
    const valid = await claim(daemon, key, pairing.code, "After wrong guesses");
    assert.equal(valid.status, 200);
    assert.equal(valid.body.pairing, pairing.id);
  } finally {
    await daemon.request({ cmd: "cancel_pairing", id: pairing.id });
  }
  console.log("Live pairing: name refusals and the wrong-code rate limit hold.");
}

/** Opens the pairing URL at `origin` in a fresh context, pairs, and checks the authenticated welcome. */
export async function checkBrowserPairing(browser: Browser, daemon: Daemon, origin: string): Promise<void> {
  const context = await browser.newContext();
  const pairing = await startPairing(daemon);
  let device: string | undefined;
  try {
    assert.equal(pairing.url, `${daemon.url}/pair#code=${pairing.code}`, "the daemon prints the pairing URL the app reads");
    const page = await context.newPage();
    const errors: string[] = [];
    const welcomes: string[] = [];
    page.on("pageerror", (error) => errors.push(error.message));
    page.on("console", (message) => { if (/content.security.policy|violat/i.test(message.text())) errors.push(message.text()); });
    page.on("websocket", (socket) => {
      if (new URL(socket.url()).pathname !== "/ws") return;
      socket.on("framereceived", (frame) => { if (String(frame.payload).includes('"reply":"welcome"')) welcomes.push(String(frame.payload)); });
    });
    const response = await page.goto(`${origin}/pair#code=${pairing.code}`);
    assert.equal(response?.status(), 200);
    const code = page.getByLabel("Pairing code");
    await code.waitFor();
    assert.equal(await code.inputValue(), pairing.code, "the URL's code is filled in");
    assert.equal(new URL(page.url()).pathname + new URL(page.url()).hash, "/#/pair/aether", "the code left the address bar");
    await page.getByLabel("Name for this device").fill(`Live ${new URL(origin).hostname}`);
    await page.getByRole("button", { name: "Pair" }).click();
    const claimed = await until(() => state(daemon, pairing.id), (value) => value.state === "claimed", "the browser's claim");
    const phrase = String((claimed.candidate as { phrase?: unknown } | undefined)?.phrase);
    await page.getByText(phrase, { exact: true }).waitFor();
    const enrolled = (await daemon.request({ cmd: "confirm_pairing", id: pairing.id, accept: true })).data;
    device = String((enrolled.device as { id?: unknown }).id);
    assert.equal(enrolled.state, "enrolled");
    await page.getByText("Paired with Aether").waitFor();
    await until(async () => welcomes.length, (count) => count > 0, "an authenticated welcome on /ws", 20_000);
    assert.ok(welcomes[0]?.includes(device), "the welcome names the paired device");
    assert.deepEqual(errors, [], `${origin}: browser or CSP errors`);
    console.log(`Live pairing: ${origin} paired through /pair#code= and was welcomed on /ws.`);
  } finally {
    try {
      if (device === undefined) await daemon.request({ cmd: "cancel_pairing", id: pairing.id });
      else await daemon.request({ cmd: "revoke_device", id: device, reason: "Live check complete" });
    } finally {
      await context.close();
    }
  }
}
