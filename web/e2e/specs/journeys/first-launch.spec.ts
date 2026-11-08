// Journey 1, first launch: a fresh root, the packaged app paired, then an agent started from
// Activity that answers, runs a shell call, takes a message, and is stopped and resumed.

import assert from "node:assert/strict";
import { writeFile } from "node:fs/promises";
import path from "node:path";
import { expect, test, type Locator, type Page } from "@playwright/test";
import { until } from "../../harness/process.ts";
import { AGENT_LIMITS, isRealModel } from "../../harness/real-engine.ts";
import { agent, agentIds, cancelLive, capAgent, toolCallInputs } from "./daemon.ts";
import { connectionsReady, pairIfAsked, journeyHooks, type Journey } from "./journey.ts";
import { FAUX_COMMANDS, LISTED_REPLY, OBJECTIVES, REAL_COMMANDS, REAL_OBJECTIVES, RELEASE_FILE } from "./scripts.ts";
import { shootWindow } from "./capture.ts";
import { agentPage, chooseWorkspace, chooseWorkType, expectDefaultModel, landOnAgent, newAgentId, objectiveField, openConversation, openStart, stopAndResume, submitStart } from "./steps.ts";
import { need, note, open, textField } from "./ui.ts";

const journey = journeyHooks();
const REPLY_MS = { faux: 30_000, real: 180_000 } as const;
const MESSAGE = "Thanks; one more check, please.";
/** The app's minimum window, the size the smoke `min-window` holds the shell to. */
const SMALLEST_WINDOW = { width: 720, height: 451 } as const;

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
  await chooseWorkType(window, "Task agent");
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
  await expectFactsAboveTranscript(page);
  await shootWindow(window, "agent", "tool-unfolded");
  await expectStackAtSmallestWindow(window);
  await until(() => toolCallInputs(stack, id), (inputs) => inputs.some((input) => input.includes(RELEASE_FILE)), "the agent to hold in its shell call", REPLY_MS[variant]);
  const held = await agent(stack, id);
  assert.ok(!["completed", "failed", "cancelled"].includes(held.state), `the held agent still works: ${held.state}`);
}

/** The agent page stacks in order with nothing drawn over its neighbour: every fact, then the transcript
 * toolbar, the transcript, the composer. The facts are measured one by one: the card's own box ends above
 * content that overflows it. */
async function expectFactsAboveTranscript(page: Locator): Promise<void> {
  const facts = await page.locator(".agent-fact").all();
  assert.ok(facts.length > 0, "the agent page shows its facts");
  let factsEnd = 0;
  for (const fact of facts) {
    const box = await fact.boundingBox();
    assert.ok(box !== null, "every fact is laid out");
    factsEnd = Math.max(factsEnd, box.y + box.height);
  }
  const toolbar = await page.locator(".conv-toolbar").boundingBox();
  const transcript = await page.locator(".conv-list").boundingBox();
  const composer = await page.locator(".cmp").boundingBox();
  assert.ok(toolbar !== null && transcript !== null && composer !== null, "the transcript toolbar, the transcript and the composer are laid out");
  assert.ok(factsEnd <= toolbar.y, `the transcript toolbar starts below the facts (facts end at ${factsEnd}, the toolbar at ${toolbar.y})`);
  assert.ok(toolbar.y + toolbar.height <= transcript.y, `the transcript starts below its toolbar (toolbar ends at ${toolbar.y + toolbar.height}, the transcript at ${transcript.y})`);
  assert.ok(transcript.y + transcript.height <= composer.y, `the composer starts below the transcript (transcript ends at ${transcript.y + transcript.height}, the composer at ${composer.y})`);
}

/** The order holds at the smallest window the app allows, where the page is shorter than the chat's own minimum
 * and scrolls instead of letting the transcript or the composer draw over a neighbour. */
async function expectStackAtSmallestWindow(window: Page): Promise<void> {
  const full = await window.evaluate(() => ({ width: innerWidth, height: innerHeight }));
  assert.ok(full.width > 0 && full.height > SMALLEST_WINDOW.height, "the journey runs in a window taller than the smallest allowed");
  await window.setViewportSize(SMALLEST_WINDOW);
  try {
    await expectFactsAboveTranscript(agentPage(window));
  } finally {
    await window.setViewportSize(full);
  }
}

/** Sends while the agent holds in its shell call: the daemon keeps the message for its next turn. */
async function sendFromComposer(window: Page): Promise<void> {
  const page = agentPage(window);
  const input = await textField(await need(page, "composer-input"));
  await input.fill(MESSAGE);
  await (await need(page, "composer-send")).click();
  await expect(input, "sending empties the composer").toHaveValue("");
  await expect(page.locator('.cmp .b3-note[data-state="done"]'), "the composer says the message was sent").toBeVisible();
  await shootWindow(window, "agent", "message-sent");
}

/** Back on the agent's page after the controls: the released agent's turn carried the message as the operator's row. */
async function expectMessageDelivered(run: Journey, id: string): Promise<void> {
  await openConversation(run.window, run.stack, id);
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
    await expectMessageDelivered(run, id);
    if (run.variant === "real") await recordCost(run, id);
  } finally {
    await run.window.screencast.stop();
    await test.info().attach("first-launch video", { path: video, contentType: "video/webm" });
    if (id !== undefined) await cancelLive(run.stack, [id]);
  }
});

test("the disconnected conversation stays ordered in a narrow, short window", async () => {
  const { window, stack } = journey();
  const full = await window.evaluate(() => ({ width: innerWidth, height: innerHeight }));
  assert.ok(full.width > 320 && full.height > SMALLEST_WINDOW.height, "the initial window is larger than the constrained viewport");
  await stack.aether.stop();
  try {
    // Follow-up: the desktop bridge detects an idle link's closure only on the next request.
    await open(window, "Activity", "#/activity");
    await window.goBack();
    await window.setViewportSize({ width: 320, height: SMALLEST_WINDOW.height });
    const page = agentPage(window);
    const banner = page.getByTestId("conv-connection");
    await expect(banner).toBeVisible();
    const lines = await banner.evaluate((element) => {
      const range = document.createRange();
      range.selectNodeContents(element);
      return range.getClientRects().length;
    });
    assert.ok(lines > 1, "the disconnected banner wraps");
    const bannerBox = await banner.boundingBox();
    const transcript = await page.locator(".conv-list").boundingBox();
    const composer = await page.locator(".cmp").boundingBox();
    assert.ok(bannerBox !== null && transcript !== null && composer !== null, "the conversation is laid out");
    const floor = await window.evaluate(() => 8 * Number.parseFloat(getComputedStyle(document.documentElement).fontSize));
    assert.ok(bannerBox.y + bannerBox.height <= transcript.y, "the banner ends above the transcript");
    assert.ok(transcript.height >= floor, "the transcript retains its 8rem minimum");
    assert.ok(transcript.y + transcript.height <= composer.y, "the composer starts below the transcript");
  } finally {
    await window.setViewportSize(full);
  }
});
