import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { expect, test } from "@playwright/test";
import { createAgent } from "../../harness/agents.ts";
import { aetherConn, route } from "../flows/desktop/state.ts";
import { connectionsReady, journeyHooks, pairIfAsked } from "./journey.ts";

const journey = journeyHooks({ fauxOnly: "Layout uses a held faux agent", x11: true });
const run = promisify(execFile);

test("agent chat fits short and wide windows", async () => {
  const { app, stack, sway, window } = journey();
  await pairIfAsked(journey());
  await connectionsReady(journey());
  stack.engine.setScript(() => ({ toolCall: { name: "bash", arguments: { command: "sleep 150" }, text: "Checking the layout." } }));
  const id = await createAgent(stack, "Layout worker", "Check the chat layout.");
  await route(window, `#/agents/${encodeURIComponent(await aetherConn(stack))}/${id}`);
  await expect(window.locator(".conv-toolbar")).toBeVisible();
  assert.ok(sway.x11Display, "the packaged app has an X11 display");
  const env = { ...process.env, DISPLAY: sway.x11Display };
  const { stdout } = await run("/usr/bin/xdotool", ["search", "--onlyvisible", "--pid", String(app.pid)], { env, timeout: 10_000 });
  const windowId = stdout.trim().split("\n")[0] ?? "";
  assert.match(windowId, /^\d+$/, "the app has a visible native window");
  await run("/usr/bin/wmctrl", ["-ir", windowId, "-b", "add,maximized_vert,maximized_horz"], { env, timeout: 10_000 });
  for (const [width, height] of [[1024, 768], [1280, 800], [3440, 1440]] as const) {
    await run("/usr/bin/swaymsg", ["-s", sway.ipc, "output", "HEADLESS-1", "resolution", `${width}x${height}`], { timeout: 10_000 });
    await expect.poll(() => window.evaluate(() => [innerWidth, innerHeight])).toEqual([width, height]);
    const screen = await window.locator(".agent").boundingBox();
    const thread = await window.locator(".conv-list").boundingBox();
    const composer = await window.locator(".cmp").boundingBox();
    assert.ok(screen && thread && composer, `${width}x${height} chat is laid out`);
    expect(thread.width).toBeCloseTo(screen.width, 0);
    expect(thread.height).toBeGreaterThan(screen.height * 0.4);
    expect(thread.y + thread.height).toBeLessThanOrEqual(composer.y + 1);
    expect(composer.y + composer.height).toBeLessThanOrEqual(height);
    expect(await window.evaluate(() => ({ x: document.documentElement.scrollWidth <= innerWidth, y: document.documentElement.scrollHeight <= innerHeight }))).toEqual({ x: true, y: true });
    if (width === 1024) {
      const details = window.locator(".agent-compact-details");
      await expect(details.locator("summary")).toBeVisible();
      await details.locator("summary").focus();
      await window.keyboard.press("Enter");
      await expect(details.locator(".agent-id")).toHaveText(id);
      await expect(details.locator(".agent-fact").filter({ hasText: "Model" })).toBeVisible();
      await window.keyboard.press("Enter");
    } else {
      await expect(window.locator(".agent-wide-facts")).toBeVisible();
    }
  }
});
