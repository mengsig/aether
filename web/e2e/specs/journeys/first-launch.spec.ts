// Journey 1, first launch: a fresh root, the packaged app paired, then an agent started from
// Activity that answers, runs a shell call, takes a message, and is stopped and resumed.

import assert from "node:assert/strict";
import { expect, test, type Page } from "@playwright/test";
import { until } from "../../harness/process.ts";
import { AGENT_LIMITS, isRealModel } from "../../harness/real-engine.ts";
import { agent, agentIds, cancelLive, capAgent } from "./daemon.ts";
import { connectionsReady, pairIfAsked, journeyHooks, type Journey } from "./journey.ts";
import { FAUX_COMMANDS, LISTED_REPLY, OBJECTIVES, REAL_COMMANDS, REAL_OBJECTIVES } from "./scripts.ts";
import { chooseWorkspace, expectDefaultModel, landOnAgent, newAgentId, objectiveField, openStart, stopAndResume, submitStart, agentPage } from "./steps.ts";
import { finish, need, note, open, optional, textField } from "./ui.ts";

const journey = journeyHooks();
const REPLY_MS = { faux: 30_000, real: 180_000 } as const;
const MESSAGE = "Thanks; one more check, please.";

test("first-launch smoke: paired, the fleet renders, Start-agent shows", async () => {
  const run = journey();
  if (!(await pairIfAsked(run))) note("fallback", "pairing: the packaged app on local sockets connects without asking");
  await connectionsReady(run);
  await open(run.window, "Fleet", "#/fleet");
  await expect(run.window.getByRole("region", { name: "Fleet" }), "the fleet renders").toBeVisible();
  await open(run.window, "Activity", "#/activity");
  await need(run.window, "start-agent", (page) => page.getByRole("button", { name: "Start an agent" }));
});

/** Starts the journey's agent from Activity's sheet and lands on its page; returns its id. */
async function startFromActivity(run: Journey): Promise<string> {
  const { window, stack, variant } = run;
  const before = await agentIds(stack);
  await openStart(window);
  await (await objectiveField(window)).fill(variant === "faux" ? OBJECTIVES.firstLaunch : REAL_OBJECTIVES.firstLaunch);
  await chooseWorkspace(window, stack.layout.aether.work);
  await expectDefaultModel(window, variant);
  const byTestId = await submitStart(window, "Start agent");
  const id = await newAgentId(stack, before);
  if (variant === "real") await capAgent(stack, id, AGENT_LIMITS);
  await landOnAgent(window, stack, id, byTestId);
  return id;
}

/** The reply and the shell call show in the transcript; the faux engine saw the call's result. */
async function expectReplyAndToolCall(run: Journey, id: string): Promise<void> {
  const { window, stack, variant } = run;
  const page = agentPage(window);
  const reply = variant === "faux" ? LISTED_REPLY : /pong/i;
  await expect(page.getByText(reply).first(), "the agent's reply shows").toBeVisible({ timeout: REPLY_MS[variant] });
  if (variant === "faux") {
    const answered = stack.engine.requests.filter((request) => request.toolResults.length > 0);
    assert.ok(answered.length > 0, "the daemon ran the shell call and sent its result back");
  }
  const command = variant === "faux" ? FAUX_COMMANDS.firstLaunch : REAL_COMMANDS.firstLaunch;
  await optional(window, "conv-tool-toggle", (p) => agentPage(p).locator("summary").filter({ hasText: /bash|shell/i }), async (toggle) => {
    await toggle.click();
    await expect(page.getByText(command).first(), "the tool call shows its command").toBeVisible();
  });
  assert.ok((await agent(stack, id)).state !== "failed", "the agent did not fail");
}

async function sendFromComposer(window: Page): Promise<void> {
  await optional(window, "composer-input", undefined, async (found) => {
    const input = await textField(found);
    await input.fill(MESSAGE);
    await input.press("Enter");
    await expect(agentPage(window).getByText(MESSAGE).first(), "the message shows in the conversation").toBeVisible();
  });
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
    if (run.variant === "real") await recordCost(run, id);
  } finally {
    await run.window.screencast.stop();
    await test.info().attach("first-launch video", { path: video, contentType: "video/webm" });
    if (id !== undefined) await cancelLive(run.stack, [id]);
  }
  finish();
});
