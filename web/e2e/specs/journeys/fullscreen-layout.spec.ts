import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { expect, test } from "@playwright/test";
import { createAgent } from "../../harness/agents.ts";
import { aetherConn, route } from "../flows/desktop/state.ts";
import { connectionsReady, journeyHooks, pairIfAsked } from "./journey.ts";

const journey = journeyHooks({ fauxOnly: "Layout uses a held faux agent", x11: true });
const run = promisify(execFile);

test("packaged chat fills a maximized and fullscreen window", async () => {
  const { app, stack, sway, window } = journey();
  await pairIfAsked(journey());
  await connectionsReady(journey());
  await run("/usr/bin/swaymsg", ["-s", sway.ipc, "output", "HEADLESS-1", "resolution", "1920x1080"], { timeout: 10_000 });
  stack.engine.setScript(() => ({ toolCall: { name: "bash", arguments: { command: "sleep 150" }, text: "Checking the layout." } }));
  const id = await createAgent(stack, "Fullscreen worker", "Check the window layout.");
  await route(window, `#/agents/${encodeURIComponent(await aetherConn(stack))}/${id}`);
  await expect(window.locator(".conv-toolbar")).toBeVisible();
  assert.ok(sway.x11Display, "the packaged app has an X11 display");
  const env = { ...process.env, DISPLAY: sway.x11Display };
  const { stdout } = await run("/usr/bin/xdotool", ["search", "--onlyvisible", "--pid", String(app.pid)], { env, timeout: 10_000 });
  const windowId = stdout.trim().split("\n")[0] ?? "";
  assert.match(windowId, /^\d+$/, "the app has a visible native window");
  for (const [state, property] of [["maximized", "maximized_vert,maximized_horz"], ["fullscreen", "fullscreen"]] as const) {
    await run("/usr/bin/wmctrl", ["-ir", windowId, "-b", `add,${property}`], { env, timeout: 10_000 });
    const windowState = async () => (await run("/usr/bin/xprop", ["-id", windowId, "_NET_WM_STATE"], { env, timeout: 10_000 })).stdout;
    await expect.poll(windowState)
      .toContain(state === "fullscreen" ? "_NET_WM_STATE_FULLSCREEN" : "_NET_WM_STATE_MAXIMIZED_VERT");
    if (state === "maximized") await expect.poll(windowState).toContain("_NET_WM_STATE_MAXIMIZED_HORZ");
    await expect.poll(() => window.evaluate(() => innerWidth)).toBeGreaterThanOrEqual(1850);
    const screen = await window.locator(".agent").boundingBox();
    const thread = await window.locator(".conv-list").boundingBox();
    const composer = await window.locator(".cmp").boundingBox();
    assert.ok(screen && thread && composer, `${state} chat is laid out`);
    expect(thread.width).toBeCloseTo(screen.width, 0);
    expect(composer.width).toBeCloseTo(screen.width, 0);
    expect(thread.y + thread.height).toBeLessThanOrEqual(composer.y + 1);
    expect(await window.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    for (const theme of ["light", "dark"] as const) {
      await window.emulateMedia({ colorScheme: theme, reducedMotion: "reduce" });
      await expect(window.locator("html")).toHaveAttribute("data-theme", theme);
      await window.screenshot({ path: test.info().outputPath(`chat-${state}-${theme}.png`) });
    }
    await run("/usr/bin/wmctrl", ["-ir", windowId, "-b", `remove,${property}`], { env, timeout: 10_000 });
  }
});
