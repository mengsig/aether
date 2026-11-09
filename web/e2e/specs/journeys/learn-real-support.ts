import assert from "node:assert/strict";
import { access, mkdir, realpath, readFile, unlink, writeFile } from "node:fs/promises";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";
import { expect, test, type Page } from "@playwright/test";
import type { Stack } from "../../harness/stack.ts";
import { until } from "../../harness/process.ts";
import { agent } from "./daemon.ts";
import { choose, need, open, textField } from "./ui.ts";

const ROOT = path.resolve(import.meta.dirname, "../../../..");
export const WAIT_MS = 240_000;

export async function checkBinaries(): Promise<void> {
  const tmp = process.env.E2E_TMP;
  assert.ok(tmp && path.isAbsolute(tmp), "run through scripts/e2e_journeys.sh with its private E2E_TMP");
  const privateTmp = await realpath(tmp);
  assert.ok(privateTmp.startsWith("/var/tmp/"), `the stack must stay under /var/tmp: ${privateTmp}`);
  const runRoot = path.dirname(privateTmp);
  for (const name of ["HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME", "XDG_STATE_HOME"]) {
    const value = process.env[name];
    assert.ok(value && value.startsWith(`${runRoot}/`), `${name} must be private to ${runRoot}`);
  }
  const target = await realpath(process.env.CARGO_TARGET_DIR ?? path.join(ROOT, "target"));
  for (const name of ["AETHER_BIN", "ARCHOND_BIN"]) {
    const value = process.env[name];
    assert.ok(value && path.isAbsolute(value), `${name} must name this checkout's build`);
    const binary = await realpath(value);
    assert.ok(binary.startsWith(`${target}/`), `${name} must resolve inside ${target}: ${binary}`);
  }
}

export async function configure(stack: Stack, model: string, budget: number): Promise<void> {
  assert.match(model, /^(claude-code\/claude-haiku-5-5|codex\/gpt-6-luna)$/);
  assert.ok(Number.isFinite(budget) && budget >= 0);
  await until(async () => {
    try {
      const reply = await stack.aether.request({ cmd: "noesis_configure", patch: {
        enabled: true, billing: { allow_api: true }, models: { strong: model, cheap: model },
        budget: { interactive_usd_daily: budget },
      } });
      assert.equal(reply.reply, "noesis_status");
      return true;
    } catch (error) {
      if (error instanceof Error && /Noesis turn\(s\) are in flight/.test(error.message)) return false;
      throw error;
    }
  }, Boolean, "Noesis settings to change after the turn settles", WAIT_MS);
}

async function interactiveSpend(stack: Stack): Promise<number> {
  const reply = await stack.aether.request({ cmd: "noesis_status" });
  assert.equal(reply.reply, "noesis_status");
  const usage = (reply.data as unknown as { spend_today: { interactive: { cost_usd: number; estimated_cost_usd?: number } } }).spend_today.interactive;
  const spent = usage.cost_usd + (usage.estimated_cost_usd ?? 0);
  assert.ok(Number.isFinite(spent) && spent >= 0, "interactive spend is nonnegative");
  return spent;
}

/** Ensure a one-cent limit will actually be spent before testing recovery. */
export async function warmBudget(stack: Stack): Promise<void> {
  for (let attempt = 0; attempt < 8; attempt += 1) {
    if (await interactiveSpend(stack) >= 0.01) return;
    const reply = await stack.aether.request({ cmd: "noesis_create_session", mode: "practice",
      goal: "Practise a simple linear equation.", idempotency_key: `budget-warmup-${crypto.randomUUID()}` });
    assert.equal(reply.reply, "noesis_session");
    const id = String(reply.data.id);
    assert.match(id, /^ns_/);
    await until(async () => turnFinished(stack, id), Boolean, "the budget warm-up turn to finish", WAIT_MS);
  }
  assert.ok(await interactiveSpend(stack) >= 0.01, "warm-up spent at least one cent");
}

interface Session {
  id: string;
  agent: string;
  mode: string;
  active_problem?: unknown;
  stalled?: { reason: string };
  questions?: Array<{ answer?: { thinking?: string }; result?: { correct?: boolean | null } }>;
}

export async function session(stack: Stack, id: string): Promise<Session> {
  assert.ok(id.length > 0);
  const reply = await stack.aether.request({ cmd: "noesis_get_session", session: id });
  assert.equal(reply.reply, "noesis_session");
  assert.equal(reply.data.id, id);
  return reply.data as unknown as Session;
}

export function sampleReads(stack: Stack, id: string): () => Promise<void> {
  assert.ok(id.startsWith("ns_"));
  const samples = { noesis_get_session: [] as number[], noesis_learner: [] as number[] };
  let running = true;
  let failure: unknown;
  const work = (async () => {
    while (running) {
      for (const command of [{ cmd: "noesis_get_session", session: id }, { cmd: "noesis_learner" }] as const) {
        const start = performance.now();
        await stack.aether.request(command);
        samples[command.cmd].push(performance.now() - start);
      }
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
  })().catch((error: unknown) => { failure = error; });
  return async () => {
    running = false;
    await work;
    if (failure !== undefined) throw failure;
    await test.info().attach(`${id}-read-latency.json`, { body: JSON.stringify(samples), contentType: "application/json" });
    for (const [command, durations] of Object.entries(samples)) {
      assert.ok(durations.length > 0, `${command} has samples`);
      durations.sort((a, b) => a - b);
      const p99 = durations[Math.ceil(durations.length * 0.99) - 1];
      assert.ok(p99 !== undefined, `${command} has a p99 sample`);
      assert.ok(p99 < 200, `${command} p99 ${p99.toFixed(1)}ms exceeds 200ms`);
    }
  };
}

export async function startSession(page: Page, mode: "Learn something new" | "Practise what I know", goal: string, probe = false): Promise<string> {
  assert.ok(goal.trim().length > 0);
  await open(page, "Sessions", "#/learn");
  await (await textField(await need(page, "learn-goal"))).fill(goal);
  await choose(page, await need(page, "learn-mode"), mode);
  if (probe) await beginStreamProbe(page);
  await (await need(page, "learn-start")).click();
  await expect(page).toHaveURL(/#\/learn\/sessions\/[^/]+$/, { timeout: 30_000 });
  const id = decodeURIComponent(page.url().split("/").at(-1) ?? "");
  assert.ok(id.length > 0);
  return id;
}

export async function assertModel(stack: Stack, id: string, model: string): Promise<void> {
  const current = await session(stack, id);
  assert.equal(typeof current.agent, "string");
  const actual = await agent(stack, current.agent);
  assert.equal(`${actual.model.provider}/${actual.model.model}`, model);
}

/** Hold the real CLI before it starts the next model call, while letting probes run. */
export async function holdEngine(stack: Stack, cli: "claude" | "codex"): Promise<{ ready: string; release: () => Promise<void> }> {
  const dir = path.join(stack.layout.root, `engine-gate-${cli}`);
  const block = path.join(dir, "hold");
  const ready = path.join(dir, "waiting");
  const executable = await realpath(path.join(stack.layout.root, "engines", cli));
  const quote = (value: string) => `'${value.replaceAll("'", "'\\''")}'`;
  await mkdir(dir, { recursive: true, mode: 0o700 });
  await writeFile(path.join(dir, cli), `#!/bin/sh\ncase "$*" in *--version*|*--help*|"auth status"|"login status") exec ${quote(executable)} "$@";; esac\nif [ -e ${quote(block)} ]; then\n  echo $$ > ${quote(ready)}\n  while [ -e ${quote(block)} ]; do sleep 0.1; done\nfi\nexec ${quote(executable)} "$@"\n`, { mode: 0o700 });
  const paths = String(stack.layout.env.PATH).split(":").filter((part) => part !== dir);
  stack.layout.env.PATH = [dir, ...paths].join(":");
  await stack.restart("aether");
  await writeFile(block, "hold");
  return { ready, release: () => unlink(block) };
}

function pendingTurn(stack: Stack, id: string): boolean {
  const db = new DatabaseSync(path.join(stack.layout.aether.data, "noesis.db"), { readOnly: true });
  try {
    const row = db.prepare("SELECT turn_pending FROM sessions WHERE id = ?").get(id);
    assert.ok(row !== undefined, "the session exists in the private store");
    return row.turn_pending === 1;
  } finally {
    db.close();
  }
}

export async function assertQueuedTurn(stack: Stack, id: string, ready: string): Promise<number> {
  await access(ready);
  const pid = Number((await readFile(ready, "utf8")).trim());
  assert.ok(Number.isSafeInteger(pid) && pid > 0, "the held engine process has a PID");
  process.kill(pid, 0);
  assert.ok(pendingTurn(stack, id), "the turn is durably queued before the real CLI starts");
  const view = await agent(stack, (await session(stack, id)).agent);
  assert.equal(view.usage?.input_tokens ?? 0, 0, "the queued turn has used no input tokens");
  assert.equal(view.usage?.output_tokens ?? 0, 0, "the queued turn has used no output tokens");
  return pid;
}

export function turnFinished(stack: Stack, id: string): boolean {
  return !pendingTurn(stack, id);
}

export async function capture(page: Page, label: string): Promise<void> {
  assert.match(label, /^[a-z0-9-]+$/);
  for (const scheme of ["light", "dark"] as const) {
    await page.emulateMedia({ colorScheme: scheme, reducedMotion: "reduce" });
    const file = test.info().outputPath(`${label}-${scheme}.png`);
    await page.screenshot({ path: file, fullPage: true });
    await test.info().attach(`${label}-${scheme}`, { path: file, contentType: "image/png" });
  }
  await page.emulateMedia({ colorScheme: null, reducedMotion: null });
}

export async function beginStreamProbe(page: Page): Promise<void> {
  await page.evaluate(() => {
    const state = { started: performance.now(), changes: [] as Array<{ ms: number; text: string }>, writingMs: null as number | null };
    (window as unknown as { learnProbe: typeof state }).learnProbe = state;
    new MutationObserver(() => {
      if (state.writingMs === null && document.querySelector(".learn-working")?.textContent?.includes("writing")) state.writingMs = performance.now() - state.started;
      const text = Array.from(document.querySelectorAll('.learn-bubble[data-live="true"]')).map((node) => node.textContent ?? "").join("").trim();
      if (text && text !== state.changes.at(-1)?.text) state.changes.push({ ms: performance.now() - state.started, text });
    }).observe(document.body, { childList: true, characterData: true, subtree: true });
  });
}

export async function endStreamProbe(page: Page, label: string): Promise<void> {
  const probe = await page.evaluate(() => (window as unknown as { learnProbe: { changes: Array<{ ms: number; text: string }>; writingMs: number | null } }).learnProbe);
  const reason = probe.changes.length === 0 ? "Prose and sealed tool fields share an undivided model reply, so it is buffered until sealed." : null;
  await test.info().attach(`${label}-stream.json`, { body: JSON.stringify({ firstTokenMs: probe.changes[0]?.ms ?? null, writingMs: probe.writingMs, reason, changes: probe.changes }, null, 2), contentType: "application/json" });
  if (reason !== null) assert.notEqual(probe.writingMs, null, `${label}: buffered reply must show writing activity`);
  else assert.ok(probe.changes.length >= 2, `${label}: streamed text must update incrementally`);
}

export async function attachSlowRequests(stack: Stack, label: string): Promise<void> {
  assert.match(label, /^[a-z0-9-]+$/);
  const log = await readFile(stack.layout.aether.log, "utf8");
  const lines = log.split("\n").filter((line) => /slow (request|engine tool call|bridge call)/.test(line));
  await test.info().attach(`${label}-slow-requests.txt`, { body: lines.join("\n") || "No slow request lines recorded.\n", contentType: "text/plain" });
  assert.equal(lines.filter((line) => /slow request/.test(line) && /noesis_(get_session|learner)/.test(line)).length, 0,
    "session and learner reads do not emit slow request warnings");
}
