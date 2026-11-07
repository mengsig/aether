// openRemote and openExternal take only what they should: an exact https: origin that
// remote.origins lists, and an https: link with no credentials in it.

import assert from "node:assert/strict";
import { test } from "node:test";
import { Bridge, externalUrl, type BridgeHost } from "../../src/bridge/bridge.ts";
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

function hostWith(origins: unknown, opened: string[]): BridgeHost {
  return {
    findDaemons: async () => [], openLink: async () => { throw new Error("no daemon"); }, push: () => {},
    confirm: async () => false, openTerminal: async () => {}, notify: () => {},
    startDaemon: async () => {}, pickDirectory: async () => null,
    openExternal: async (url) => { opened.push(`external ${url}`); },
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

test("openExternal opens https: links without credentials only", () => {
  assert.equal(externalUrl("https://docs.example/page?q=1"), "https://docs.example/page?q=1");
  for (const bad of ["http://docs.example", "file:///etc/passwd", "javascript:alert(1)", "https://u:p@docs.example",
    "smb://share", "not a url"]) {
    assert.throws(() => externalUrl(bad), Error, bad);
  }
});
