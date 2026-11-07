// Journey 1, first launch: a fresh root, the packaged app paired, then an agent started from
// Activity that answers, runs a shell call, takes a message, and is stopped and resumed.

import assert from "node:assert/strict";
import { writeFile } from "node:fs/promises";
import path from "node:path";
import { expect, test, type Page } from "@playwright/test";
import { until } from "../../harness/process.ts";
import { AGENT_LIMITS, isRealModel } from "../../harness/real-engine.ts";
import { agent, agentIds, cancelLive, capAgent, toolCallInputs } from "./daemon.ts";
import { connectionsReady, pairIfAsked, journeyHooks, type Journey } from "./journey.ts";
import { FAUX_COMMANDS, LISTED_REPLY, OBJECTIVES, REAL_COMMANDS, REAL_OBJECTIVES, RELEASE_FILE } from "./scripts.ts";
import { shootWindow } from "./capture.ts";
import { chooseWorkspace, expectDefaultModel, landOnAgent, newAgentId, objectiveField, openStart, stopAndResume, submitStart, agentPage } from "./steps.ts";
import { need, note, open, textField } from "./ui.ts";

const journey = journeyHooks();
const REPLY_MS = { faux: 30_000, real: 180_000 } as const;
const MESSAGE = "Thanks; one more check, please.";

test("first-launch smoke: paired, the fleet renders, Start-agent shows", async () => {
  const run = journey();
  if (!(await pairIfAsked(run))) note("pairing", "the packaged app on local sockets connects without asking");
  await connectionsReady(run);
  await open(run.window, "Fleet", "#/fleet");
  await expect(run.window.getByRole("region", { name: "Fleet" }), "the fleet renders").toBeVisible();
  await shootWindow(run.window, "fleet", "paired");
  await open(run.window, "Activity", "#/activity");
  await expect(run.window.getByRole("button", { name: "Start an agent", exact: true }).first(), "Activity offers Start an agent").toBeVisible();
  await shootWindow(run.window, "activity", "start-offered");
});

/** Starts the journey's agent from Activity's sheet and lands on its page; returns its id. */
async function startFromActivity(run: Journey): Promise<string> {
  const { window, stack, variant } = run;
  const before = await agentIds(stack);
  await openStart(window);
  await (await objectiveField(window)).fill(variant === "faux" ? OBJECTIVES.firstLaunch : REAL_OBJECTIVES.firstLaunch);
  await chooseWorkspace(window, stack.layout.aether.work);
  await expectDefaultModel(window, variant);
  await shootWindow(window, "start-agent", "filled");
  await submitStart(window, "Start agent");
  const id = await newAgentId(stack, before);
  if (variant === "real") await capAgent(stack, id, AGENT_LIMITS);
  await landOnAgent(window, id);
  return id;
}

/** The reply and the shell call show in the transcript; the faux engine saw the call's result. */
async function expectReplyAndToolCall(run: Journey, id: string): Promise<void> {
  const { window, stack, variant } = run;
  const page = agentPage(window);
  const reply = variant === "faux" ? LISTED_REPLY : /pong/i;
  // The agent's own words: the page also prints the objective, which names the reply.
  await expect(page.locator('.conv-msg[data-role="assistant"]').filter({ hasText: reply }).first(), "the agent's reply shows").toBeVisible({ timeout: REPLY_MS[variant] });
  if (variant === "faux") {
    const answered = stack.engine.requests.filter((request) => request.toolResults.length > 0);
    assert.ok(answered.length > 0, "the daemon ran the shell call and sent its result back");
  }
  const command = variant === "faux" ? FAUX_COMMANDS.firstLaunch : REAL_COMMANDS.firstLaunch;
  const toggle = page.getByTestId("conv-tool-toggle").filter({ hasText: /bash|shell/i }).first();
  await expect(toggle, "the shell call shows in the conversation").toBeVisible();
  await toggle.click();
  await expect(toggle, "the shell call unfolds").toHaveAttribute("aria-expanded", "true");
  const output = page.getByTestId("conv-tool-output").filter({ hasText: command }).first();
  await expect(output, "the unfolded call shows its command").toBeVisible();
  await shootWindow(window, "agent", "tool-unfolded");
  await until(() => toolCallInputs(stack, id), (inputs) => inputs.some((input) => input.includes(RELEASE_FILE)), "the agent to hold in its shell call", REPLY_MS[variant]);
  const held = await agent(stack, id);
  assert.ok(!["completed", "failed", "cancelled"].includes(held.state), `the held agent still works: ${held.state}`);
}

/** Sends while the agent holds in its shell call: the daemon keeps the message for its next turn. */
async function sendFromComposer(window: Page): Promise<void> {
  const page = agentPage(window);
  const input = await textField(await need(page, "composer-input"));
  await input.fill(MESSAGE);
  await (await need(page, "composer-send")).click();
  await expect(input, "sending empties the composer").toHaveValue("");
  await expect(page.locator('.conv-composer-note[data-state="done"]'), "the composer says the message reaches the agent at its next turn").toBeVisible();
  await shootWindow(window, "agent", "message-sent");
}

/** The released agent's next turn carried the message: the transcript shows it as the operator's. */
async function expectMessageDelivered(run: Journey): Promise<void> {
  const page = agentPage(run.window);
  const delivered = page.locator('.conv-msg[data-speaker="user"]').filter({ hasText: MESSAGE }).first();
  await expect(delivered, "the sent message shows in the conversation once the agent read it").toBeVisible({ timeout: REPLY_MS[run.variant] });
}

/** Releases the agent's held shell call; it then finishes its task. */
async function releaseAndFinish(run: Journey, id: string): Promise<void> {
  const work = run.stack.layout.aether.work;
  assert.ok(path.isAbsolute(work), `the agent's workspace is absolute: ${work}`);
  await writeFile(path.join(work, RELEASE_FILE), "go\n");
  const done = await until(() => agent(run.stack, id), (now) => ["completed", "failed", "cancelled"].includes(now.state), "the released agent to finish", REPLY_MS[run.variant]);
  assert.equal(done.state, "completed", "the agent completes its task once released");
}

/** The real run's spend, from the agent's cost fields, on the test's report. */
async function recordCost(run: Journey, id: string): Promise<void> {
  const view = await until(() => agent(run.stack, id), (now) => now.usage !== null, "the agent's usage", 30_000);
  assert.ok(isRealModel(view.model), `the agent ran a capped real model: ${view.model.provider}/${view.model.model}`);
  const cost = view.usage?.cost_usd ?? view.usage?.estimated_cost_usd ?? null;
  test.info().annotations.push({ type: "cost", description: `${view.model.provider}/${view.model.model}: ${cost === null ? "unreported" : `$${cost.toFixed(4)}`} (cost_usd ${String(view.usage?.cost_usd)}, estimated ${String(view.usage?.estimated_cost_usd)})` });
}

test("first-launch journey: start an agent, see it work, message, stop and resume", async () => {
  const run = journey();
  const video = test.info().outputPath("first-launch.webm");
  await run.window.screencast.start({ path: video, size: { width: 1280, height: 800 } });
  let id: string | undefined;
  try {
    id = await startFromActivity(run);
    await expectReplyAndToolCall(run, id);
    await sendFromComposer(run.window);
    await stopAndResume(run.window, run.stack, id);
    await releaseAndFinish(run, id);
    await expectMessageDelivered(run);
    if (run.variant === "real") await recordCost(run, id);
  } finally {
    await run.window.screencast.stop();
    await test.info().attach("first-launch video", { path: video, contentType: "video/webm" });
    if (id !== undefined) await cancelLive(run.stack, [id]);
  }
});
