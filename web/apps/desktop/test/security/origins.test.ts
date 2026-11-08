// openRemote and openExternal take only what they should: an exact https: origin that
// remote.origins lists; GitHub or a listed origin at once, another https: site once the user
// agrees, a loopback report page in its window, and nothing else.

import assert from "node:assert/strict";
import { test } from "node:test";
import { Bridge, type BridgeHost } from "../../src/bridge/bridge.ts";
import { externalTarget } from "../../src/native/external.ts";
import type { Confirmation } from "../../src/security/confirm.ts";
import { normalizeOrigin, normalizeOriginList, OriginError } from "../../src/security/origins.ts";
import { APP_SENDER, VALIDATOR } from "./support.ts";

test("an origin is https:, with no path, query, fragment or userinfo", () => {
  assert.equal(normalizeOrigin("https://Box.Example.ts.net/"), "https://box.example.ts.net");
  assert.equal(normalizeOrigin("https://box.example:443"), "https://box.example");
  assert.equal(normalizeOrigin("https://bücher.example"), "https://xn--bcher-kva.example");
  for (const bad of ["http://box.example", "https://box.example/path", "https://user@box.example",
    "https://user:pw@box.example", "https://box.example?x", "https://box.example#x", "https://box.example/.",
    "https://box.example\\@evil.example", " https://box.example", "wss://box.example", "box.example", ""]) {
    assert.throws(() => normalizeOrigin(bad), OriginError, bad);
  }
  assert.throws(() => normalizeOriginList(["https://a.example", "https://A.example"]), OriginError);
});

function hostWith(origins: unknown, opened: string[], asked: Confirmation[] = [], answer = false): BridgeHost {
  return {
    findDaemons: async () => [], openLink: async () => { throw new Error("no daemon"); }, push: () => {}, closed: () => {},
    confirm: async (_owner, confirmation) => { asked.push(confirmation); return answer; },
    openTerminal: async () => {}, notify: () => {},
    startDaemon: async () => {}, pickDirectory: async () => null,
    openExternal: async (url) => { opened.push(`external ${url}`); },
    openReport: async (url) => { opened.push(`report ${url}`); },
    openRemote: async (origin) => { opened.push(origin); },
    getPreference: (key) => (key === "remote.origins" ? origins : undefined), setPreference: async () => {},
    setup: async () => { throw new Error("no setup in this test"); },
    versions: {}, logPath: "/tmp/log", protocolVersions: { aether: 26, archon: 24 },
  };
}

test("openRemote opens only an origin listed in remote.origins", async () => {
  const opened: string[] = [];
  const bridge = new Bridge(VALIDATOR, hostWith(["https://box.example.ts.net"], opened));
  for (const bad of ["http://box.example.ts.net", "https://box.example.ts.net/admin", "https://me@box.example.ts.net",
    "https://other.example.ts.net"]) {
    await assert.rejects(bridge.handle(APP_SENDER, 1, "openRemote", [bad]), Error, bad);
  }
  assert.deepEqual(opened, []);
  await bridge.handle(APP_SENDER, 1, "openRemote", ["https://BOX.example.ts.net/"]);
  assert.deepEqual(opened, ["https://box.example.ts.net"]);
  const unset = new Bridge(VALIDATOR, hostWith(undefined, opened));
  await assert.rejects(unset.handle(APP_SENDER, 1, "openRemote", ["https://box.example.ts.net"]), /remote.origins/);
});

test("a link's target: https: to the browser, trusted only for GitHub's exact origins; loopback http: to a report", () => {
  assert.deepEqual(externalTarget("https://github.com/octocat/hello/pull/1"),
    { kind: "browser", url: "https://github.com/octocat/hello/pull/1", origin: "https://github.com", trusted: true });
  assert.ok((externalTarget("https://GIST.github.com:443/x") as { trusted: boolean }).trusted, "the default port is the same origin");
  const other = externalTarget("https://docs.example/page?q=1");
  assert.ok(other.kind === "browser" && !other.trusted && other.url === "https://docs.example/page?q=1");
  for (const untrusted of ["https://github.com.evil.example/", "https://github.com:8443/x", "https://api.github.com/x", "https://box.example.ts.net/x"]) {
    assert.ok(!(externalTarget(untrusted) as { trusted: boolean }).trusted, untrusted);
  }
  assert.deepEqual(externalTarget("http://127.0.0.1:4387/session/abc"),
    { kind: "report", url: "http://127.0.0.1:4387/session/abc", origin: "http://127.0.0.1:4387" });
  assert.equal(externalTarget("http://[::1]:9000/r").kind, "report");
  for (const bad of ["http://docs.example", "http://127.0.0.1/no-port", "file:///etc/passwd", "javascript:alert(1)",
    "https://u:p@docs.example", "http://u@127.0.0.1:4387/", "smb://share", "mailto:octocat@example.com", "not a url", "",
    `https://github.com/${"a".repeat(2100)}`]) {
    assert.throws(() => externalTarget(bad), Error, bad);
  }
});

test("openExternal opens GitHub at once, asks for another site, and opens a local report in its window", async () => {
  const opened: string[] = [];
  const asked: Confirmation[] = [];
  const bridge = new Bridge(VALIDATOR, hostWith([], opened, asked, false));
  await bridge.handle(APP_SENDER, 1, "openExternal", ["https://github.com/octocat/hello"]);
  await bridge.handle(APP_SENDER, 1, "openExternal", ["http://127.0.0.1:4387/session/abc"]);
  assert.deepEqual(asked, []);
  await assert.rejects(bridge.handle(APP_SENDER, 1, "openExternal", ["https://docs.example/page"]), /Cancelled/);
  assert.equal(asked.length, 1);
  assert.match((asked[0] as Confirmation | undefined)?.detail ?? "", /"https:\/\/docs\.example"/);
  assert.deepEqual(opened, ["external https://github.com/octocat/hello", "report http://127.0.0.1:4387/session/abc"]);
  const agreeing = new Bridge(VALIDATOR, hostWith([], opened, [], true));
  await agreeing.handle(APP_SENDER, 1, "openExternal", ["https://docs.example/page"]);
  assert.equal(opened.at(-1), "external https://docs.example/page");
  await assert.rejects(agreeing.handle(APP_SENDER, 1, "openExternal", ["file:///etc/passwd"]), /https: links only/);
  assert.equal(opened.length, 3);
});

test("a renderer-written remote.origins entry never skips the question for a link", async () => {
  const opened: string[] = [];
  const asked: Confirmation[] = [];
  const bridge = new Bridge(VALIDATOR, hostWith(["https://box.example.ts.net"], opened, asked, false));
  await assert.rejects(bridge.handle(APP_SENDER, 1, "openExternal", ["https://box.example.ts.net/x"]), /Cancelled/);
  assert.equal(asked.length, 1, "the listed origin was asked about like any other site");
  assert.deepEqual(opened, []);
});
