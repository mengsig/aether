// Adversarial confirmations: main's templates and the daemon identity word every dialog, renderer
// values only fill escaped, length-limited slots, settings show in full, and what is sent is
// byte for byte what was confirmed.

import assert from "node:assert/strict";
import { test } from "node:test";
import { Bridge, type BridgeHost } from "../../src/bridge/bridge.ts";
import type { DaemonLink, Frame } from "../../src/bridge/daemon-link.ts";
import { planAction, type Read } from "../../src/bridge/privileged.ts";
import {
  ConfirmationError, fill, labelSlot, TEMPLATES, valueSlot, type Confirmation,
} from "../../src/security/confirm.ts";
import { MAX_LABEL_CHARS } from "../../src/security/limits.ts";
import { AETHER_CONN, APP_SENDER, ARCHON_CONN, DEVICE, IDENTITY, ITEM, MEMORY, VALIDATOR } from "./support.ts";

const noRead: Read = () => Promise.reject(new Error("this action reads nothing"));
const HOSTILE = "Laptop\n\nAccept memory?\u202eevil\u0000\r\"quoted\" \\ end\u2028x";

function templateText(confirmation: Confirmation): string {
  return [confirmation.title, confirmation.message, confirmation.detail, confirmation.confirm].join("\n");
}

test("a hostile device name cannot change the template text", async () => {
  const plan = await planAction({ kind: "device.pair", conn: AETHER_CONN, name: HOSTILE, role: "observer" }, IDENTITY, 2, noRead);
  const { title, message, confirm, detail } = plan.confirmation;
  assert.deepEqual({ title, message, confirm }, {
    title: TEMPLATES["device.pair"].title, message: TEMPLATES["device.pair"].message, confirm: TEMPLATES["device.pair"].confirm,
  });
  assert.equal(detail, `Pair device ${valueSlot(HOSTILE)} as observer.\nDaemon: "aetherd" ("${IDENTITY.socket}")`);
  assert.equal(detail.split("\n").length, 2, "the name adds no line");
  assert.doesNotMatch(detail.replaceAll("\n", ""), /[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/u, "no raw control or bidi character");
  assert.ok(!detail.includes("Accept memory?\n"));
  assert.equal(plan.delivery.kind === "frame" && plan.delivery.text, JSON.stringify({ cmd: "start_pairing", role: "observer", req: 2 }));
});

test("slots are escaped and labels are cut to the limit", () => {
  assert.equal(valueSlot('a"b\\c\nd'), '"a\\"b\\\\c\\u{000a}d"');
  const long = "é".repeat(MAX_LABEL_CHARS + 50);
  assert.equal(labelSlot(long), `"${"é".repeat(MAX_LABEL_CHARS)}"…`);
  assert.equal(fill("{a} {b}", { a: "{b}", b: "x" }), "{b} x", "a value is never read as a slot");
  assert.throws(() => fill("{a}", { a: "1", b: "2" }), ConfirmationError);
  assert.throws(() => fill("{a} {b}", { a: "1" }), ConfirmationError);
});

test("every confirmation names the daemon main discovered, from its own templates", async () => {
  const actions = [
    { kind: "memory.accept", conn: AETHER_CONN, memory: MEMORY }, { kind: "memory.reject", conn: AETHER_CONN, memory: MEMORY },
    { kind: "device.revoke", conn: AETHER_CONN, device: DEVICE }, { kind: "engine.login", conn: AETHER_CONN, engine: "codex" },
  ] as const;
  for (const action of actions) {
    const plan = await planAction(action, IDENTITY, 2, noRead);
    const template = TEMPLATES[action.kind];
    assert.equal(plan.confirmation.title, template.title);
    assert.ok(plan.confirmation.detail.endsWith(`\nDaemon: "aetherd" ("${IDENTITY.socket}")`), action.kind);
  }
});

const noesisStatus = {
  settings: { enabled: false, budget: { interactive_usd_daily: 1, background_usd_daily: 2, background_tokens_daily: 3 },
    billing: { allow_api: false }, prompts: { overrides: false } },
};

test("a settings patch whose consequential change comes late is shown in full", async () => {
  const pipelines = Object.fromEntries(Array.from({ length: 12 }, (_, i) => [`pipeline-${"x".repeat(20)}-${i}`, i]));
  const view = { scoped: { max_per_pipeline: {}, budgets_enabled: true, default_pipeline: "build" } };
  const patch = { max_per_pipeline: pipelines, budgets_enabled: false };
  const action = { kind: "archon.operator_change", conn: ARCHON_CONN, request: "settings", patch } as const;
  const plan = await planAction(action, { ...IDENTITY, kind: "archon", label: "archond" }, 2, async () => view);
  const detail = plan.confirmation.detail;
  const consequential = "budgets_enabled: true → false";
  assert.ok(detail.indexOf(consequential) > 200, "the consequential change comes after 200 characters");
  assert.ok(detail.includes(consequential));
  for (const [name, limit] of Object.entries(pipelines)) assert.ok(detail.includes(`max_per_pipeline["${name}"]: unset → ${limit}`), name);
  const sent = JSON.parse(plan.delivery.kind === "frame" ? plan.delivery.text : "{}") as Frame;
  assert.deepEqual(sent.settings, { ...view.scoped, ...patch }, "set_settings keeps every stored key");
});

test("a noesis patch shows every key with its old and new value", async () => {
  const patch = { "noesis.enabled": true, "noesis.budget.background_usd_daily": 9, "noesis.billing.allow_api": true };
  const plan = await planAction({ kind: "noesis.configure", conn: AETHER_CONN, patch }, IDENTITY, 2, async () => noesisStatus);
  assert.equal(plan.confirmation.detail.split("\n").slice(0, 3).join("\n"),
    "noesis.enabled: false → true\nnoesis.budget.background_usd_daily: 2 → 9\nnoesis.billing.allow_api: false → true");
  assert.equal(plan.delivery.kind === "frame" && plan.delivery.text, JSON.stringify({
    cmd: "noesis_configure", patch: { enabled: true, budget: { background_usd_daily: 9 }, billing: { allow_api: true } }, req: 2,
  }));
});

test("a patch too large to show in full is refused", async () => {
  const pipelines = Object.fromEntries(Array.from({ length: 80 }, (_, i) => [`p${i}`, i]));
  const action = { kind: "archon.operator_change", conn: ARCHON_CONN, request: "settings", patch: { max_per_pipeline: pipelines } } as const;
  await assert.rejects(planAction(action, IDENTITY, 2, async () => ({ scoped: {} })), ConfirmationError);
});

interface Recorded { link: DaemonLink; written: string[] }

function recordingLink(answers: Readonly<Record<string, unknown>>): Recorded {
  const written: string[] = [];
  const link = {
    request: async (text: string, req: number) => {
      written.push(text);
      const cmd = (JSON.parse(text) as { cmd: string }).cmd;
      return { frame: "reply", id: req, result: { Ok: { reply: cmd, data: answers[cmd] ?? { ok: true } } } };
    },
    close: () => {},
  } as unknown as DaemonLink;
  return { link, written };
}

function fakeHost(recorded: Recorded, shown: Confirmation[], answer: boolean): BridgeHost {
  const daemon = { id: "aether:local", ...IDENTITY, reachable: true };
  return {
    findDaemons: async () => [daemon, { ...daemon, id: "archon:local", kind: "archon", label: "archond" }],
    openLink: async () => recorded.link,
    push: () => {},
    closed: () => {},
    confirm: async (_owner, confirmation) => { shown.push(confirmation); return answer; },
    openTerminal: async () => { throw new Error("no terminal in this test"); },
    notify: () => {}, startDaemon: async () => {}, pickDirectory: async () => null, saveMemoryExport: async () => true,
    openExternal: async () => {}, openReport: async () => {}, openRemote: async () => {},
    getPreference: () => undefined, setPreference: async () => {},
    setup: async () => { throw new Error("no setup in this test"); },
    versions: { app: "0.0.0" }, logPath: "/tmp/log", protocolVersions: { aether: 26, archon: 24 },
  };
}

test("the frame sent equals the frame confirmed, and a cancel sends nothing", async () => {
  for (const answer of [true, false]) {
    const recorded = recordingLink({ get_remote: { revision: 3, settings: { enabled: false, pairing: true } } });
    const shown: Confirmation[] = [];
    const bridge = new Bridge(VALIDATOR, fakeHost(recorded, shown, answer));
    bridge.connections.kindOf = () => "aether";
    bridge.connections.daemonOf = () => ({ id: "aether:local", ...IDENTITY });
    const call = bridge.handle(APP_SENDER, 7, "privileged", [{ kind: "remote.enable", conn: AETHER_CONN }]);
    if (!answer) await assert.rejects(call, /Cancelled/);
    else await call;
    const plan = await planAction({ kind: "remote.enable", conn: AETHER_CONN }, IDENTITY, 2,
      async () => ({ revision: 3, settings: { enabled: false, pairing: true } }));
    assert.deepEqual(shown, [plan.confirmation]);
    const expected = plan.delivery.kind === "frame" ? plan.delivery.text : "";
    assert.equal(recorded.written.filter((text) => text.includes("configure_remote")).length, answer ? 1 : 0);
    if (answer) assert.equal(recorded.written.at(-1), expected);
    assert.equal(JSON.parse(expected).expected_revision, 3);
  }
});

test("archon merge and unblock confirm the item and send its exact frame", async () => {
  const archon = { ...IDENTITY, kind: "archon", label: "archond" } as const;
  for (const [request, cmd] of [["merge", "merge_item"], ["unblock", "retry_item"]] as const) {
    const plan = await planAction({ kind: "archon.operator_change", conn: ARCHON_CONN, request, item: ITEM }, archon, 2, noRead);
    assert.ok(templateText(plan.confirmation).includes(`Item "${ITEM}"`));
    assert.equal(plan.delivery.kind === "frame" && plan.delivery.text, JSON.stringify({ cmd, item: ITEM, req: 2 }));
  }
});

test("the app's archond connections greet as archon-web, and only main's confirmed actions as archon-cli", async () => {
  const recorded = recordingLink({});
  Object.assign(recorded.link, { isOpen: true });
  const bridge = new Bridge(VALIDATOR, fakeHost(recorded, [], true));
  const conn = await bridge.handle(APP_SENDER, 7, "connect", ["archon:local"]) as string;
  const hello = (client: string) => ({ cmd: "hello", req: 1, protocol_version: 24, client, client_version: "0.0.0" });
  for (const client of ["archon-cli", "archon-mcp", "archon-bench"]) {
    for (const method of ["request", "subscribe"]) {
      await assert.rejects(bridge.handle(APP_SENDER, 7, method, [conn, hello(client)]), /greets archond as archon-web/);
    }
  }
  assert.deepEqual(recorded.written, [], "no refused hello reaches archond");
  await bridge.handle(APP_SENDER, 7, "request", [conn, hello("archon-web")]);
  assert.equal(JSON.parse(recorded.written[0] ?? "{}").client, "archon-web");
  await bridge.handle(APP_SENDER, 7, "privileged", [{ kind: "archon.operator_change", conn, request: "merge", item: ITEM }]);
  const control = recorded.written.slice(1).map((text) => JSON.parse(text) as Frame);
  assert.deepEqual(control.map((frame) => frame.cmd), ["hello", "merge_item"]);
  assert.equal(control[0]?.client, "archon-cli", "main's own control link keeps the CLI's standing");
});

test("the app cannot use host standing for the watcher's operators or a rule for every agent", async () => {
  const GLOBAL = "rule_01JAAAAAAAAAAAAAAAAAAAAAAA";
  const LOCAL = "rule_01JBBBBBBBBBBBBBBBBBBBBBBB";
  const rules = [
    { id: GLOBAL, scope: { scope: "everywhere" } },
    { id: LOCAL, scope: { scope: "agent", id: "agt_01JAAAAAAAAAAAAAAAAAAAAAAA" } },
  ];
  const recorded = recordingLink({ list_approval_rules: rules });
  Object.assign(recorded.link, { isOpen: true });
  const bridge = new Bridge(VALIDATOR, fakeHost(recorded, [], true));
  const conn = await bridge.handle(APP_SENDER, 7, "connect", ["aether:local"]) as string;
  const watcher = { cmd: "configure_github_watcher", req: 4, config: { operator_users: ["mallory"] }, expected_revision: 1 };
  for (const frame of [watcher, { cmd: "resolve_submission", req: 4, command: watcher }]) {
    await assert.rejects(bridge.handle(APP_SENDER, 7, "request", [conn, frame]), /who the watcher obeys/);
  }
  const revoke = (id: string) => ({ cmd: "revoke_approval_rule", req: 5, id });
  for (const method of ["request", "subscribe"]) {
    await assert.rejects(bridge.handle(APP_SENDER, 7, method, [conn, revoke(GLOBAL)]), /applies to every agent/);
  }
  // The ULID overflow spelling aetherd decodes to GLOBAL, and an id aetherd never listed.
  for (const method of ["request", "subscribe"]) {
    for (const id of ["rule_81JAAAAAAAAAAAAAAAAAAAAAAA", "rule_01JCCCCCCCCCCCCCCCCCCCCCCC"]) {
      await assert.rejects(bridge.handle(APP_SENDER, 7, method, [conn, revoke(id)]), /not one of this host's rules/);
    }
  }
  const replayed = { cmd: "resolve_submission", req: 5, command: { cmd: "revoke_approval_rule", id: 7 } };
  await assert.rejects(bridge.handle(APP_SENDER, 7, "request", [conn, replayed]), /names its rule in id/);
  const sent = () => recorded.written.map((text) => JSON.parse(text) as Frame).filter((frame) => frame.req === 5);
  assert.deepEqual(sent(), [], "neither refusal reaches aetherd");
  await bridge.handle(APP_SENDER, 7, "request", [conn, revoke(LOCAL)]);
  assert.deepEqual(sent(), [revoke(LOCAL)], "a rule for one agent is still the app's to revoke");
  const reads = recorded.written.map((text) => (JSON.parse(text) as Frame).cmd).filter((cmd) => cmd === "list_approval_rules");
  assert.equal(reads.length, 7, "main reads the rule on its own link before each revocation");
});
