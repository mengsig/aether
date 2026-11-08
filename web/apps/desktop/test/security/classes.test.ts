// The class allowlist mirrors `Command::access` (crates/aether-protocol/src/access.rs): every
// command of the generated schema has a class, and each static class matches the Rust source.

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { REPO_ROOT } from "../../src/main/paths.ts";
import { AETHER_BY_FIELD, ARCHON_PASS, aetherClass, passThroughRefusal } from "../../src/security/classes.ts";
import { SCHEMAS } from "./support.ts";

const RUST_CLASSES: Readonly<Record<string, string>> = {
  Observe: "observe", Operate: "operate", HostOnly: "host_only", RemoteAdmin: "remote_admin", DeviceSelf: "device_self",
};

function snake(name: string): string {
  return name.replace(/[A-Z]/g, (letter, index: number) => `${index === 0 ? "" : "_"}${letter.toLowerCase()}`);
}

/** The static arms of `Command::access`: command name to class. */
function rustClasses(): Map<string, string> {
  const source = readFileSync(path.join(REPO_ROOT, "crates/aether-protocol/src/access.rs"), "utf8");
  const body = source.slice(source.indexOf("pub fn access(&self) -> Access"));
  const arm = /((?:\|?\s*Self::\w+(?:\s*\{\s*\.\.\s*\}|\(_\))?\s*)+)=>\s*\{?\s*Access::(\w+)/g;
  const classes = new Map<string, string>();
  for (const [, names, access] of body.matchAll(arm)) {
    const wanted = RUST_CLASSES[access ?? ""];
    assert.ok(wanted !== undefined, `a known class: ${access}`);
    for (const [, name] of (names ?? "").matchAll(/Self::(\w+)/g)) classes.set(snake(name ?? ""), wanted);
  }
  assert.ok(classes.size > 100, `the parse found the arms (${classes.size})`);
  return classes;
}

test("every static class matches access.rs", () => {
  for (const [cmd, wanted] of rustClasses()) {
    assert.equal(aetherClass({ cmd }), wanted, cmd);
  }
});

test("every Aether command in the generated schema has a class", () => {
  const rust = rustClasses();
  const commands = SCHEMAS.frames.aether.commands();
  assert.ok(commands.length > 100);
  for (const cmd of commands) {
    assert.ok(rust.has(cmd) || AETHER_BY_FIELD.has(cmd), `${cmd} is classified in access.rs`);
    if (cmd !== "resolve_submission") assert.notEqual(aetherClass({ cmd }), undefined, `${cmd} has a desktop class`);
  }
});

test("field rules widen the class as access.rs does", () => {
  assert.equal(aetherClass({ cmd: "create_memory", spec: { proposed: true } }), "host_only");
  assert.equal(aetherClass({ cmd: "memories", query: { proposed: true } }), "host_only");
  assert.equal(aetherClass({ cmd: "list_devices", include_revoked: true }), "remote_admin");
  assert.equal(aetherClass({ cmd: "update_agent", patch: { tools: [] } }), "host_only");
  assert.equal(aetherClass({ cmd: "add_approval_rule", scope: { scope: "everywhere" } }), "host_only");
  assert.equal(aetherClass({ cmd: "resolve_approval", remember: { scope: { scope: "everywhere" } } }), "host_only");
  assert.equal(aetherClass({ cmd: "noesis_configure", patch: { billing: { allow_api: true } } }), "host_only");
  assert.equal(aetherClass({ cmd: "resolve_submission", command: { cmd: "shutdown" } }), "host_only");
  assert.equal(aetherClass({ cmd: "resolve_submission", command: { cmd: "status" } }), "operate");
});

test("the Archon pass list names only commands Archon's schema has, and never shutdown", () => {
  const commands = new Set(SCHEMAS.frames.archon.commands());
  for (const cmd of ARCHON_PASS) assert.ok(commands.has(cmd), cmd);
  assert.ok(!ARCHON_PASS.has("shutdown") && !ARCHON_PASS.has("operator_command") && !ARCHON_PASS.has("for_agent"));
  assert.match(passThroughRefusal("archon", { cmd: "shutdown" }) ?? "", /shutdown/);
  assert.equal(passThroughRefusal("archon", { cmd: "pause", scope: "global" }), undefined);
});

test("the app says, answers and reads an initiative as the web app, never with the CLI's knobs", () => {
  assert.equal(passThroughRefusal("archon", { cmd: "say", text: "hi" }, 26), undefined);
  assert.equal(passThroughRefusal("archon", { cmd: "answer", question: "q_1", text: "yes" }, 26), undefined);
  assert.equal(passThroughRefusal("archon", { cmd: "get_initiative", initiative: "in_1" }), undefined);
  assert.match(passThroughRefusal("archon", { cmd: "answer", question: "q_1", text: "yes", chat: true }, 26) ?? "", /archon chat/);
  assert.match(passThroughRefusal("archon", { cmd: "resume", scope: "global", hard: true }) ?? "", /archon CLI/);
});

test("steering commands require web standing and model policy remains readable", () => {
  assert.equal(passThroughRefusal("archon", { cmd: "model_policy" }, 24), undefined);
  for (const cmd of ["steer_item", "update_item"]) {
    assert.match(passThroughRefusal("archon", { cmd }, 23) ?? "", /Archon 1\.5\.0 or newer/);
    assert.equal(passThroughRefusal("archon", { cmd }, 24), undefined);
  }
});

test("no frame but the hello declares a client, and none names an agent caller", () => {
  assert.match(passThroughRefusal("archon", { cmd: "say", text: "hi", client: "archon-cli" }, 26) ?? "", /names no client/);
  assert.match(passThroughRefusal("archon", { cmd: "answer", question: "q_1", text: "y", caller: "agt_1" }, 26) ?? "", /names no caller/);
  assert.match(passThroughRefusal("archon", { cmd: "hello", client: "archon-cli" }) ?? "", /greets archond as archon-web/);
  assert.equal(passThroughRefusal("archon", { cmd: "hello", client: "archon-web" }), undefined);
});

test("say and answer pass only on a welcome of protocol 24 or newer; the rest keeps protocol 23", () => {
  const say = { cmd: "say", text: "hi" };
  const answer = { cmd: "answer", question: "q_1", text: "yes" };
  for (const frame of [say, answer]) {
    assert.match(passThroughRefusal("archon", frame, 23) ?? "", /Archon 1\.5\.0 or newer/);
    assert.match(passThroughRefusal("archon", frame) ?? "", /Archon 1\.5\.0 or newer/);
    assert.equal(passThroughRefusal("archon", frame, 24), undefined);
  }
  assert.equal(passThroughRefusal("archon", { cmd: "get_initiative", initiative: "in_1" }, 23), undefined);
  assert.equal(passThroughRefusal("archon", { cmd: "pause", scope: "global" }, 23), undefined);
  assert.equal(passThroughRefusal("archon", { cmd: "hello", client: "archon-web" }), undefined);
});
