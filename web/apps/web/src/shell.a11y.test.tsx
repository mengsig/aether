// @vitest-environment happy-dom
// axe on the shell as the gallery renders it. happy-dom does no layout, so colour
// contrast is checked in Chromium by scripts/capture.ts instead.

import { AppShell, ThemeProvider } from "@aether-ui/ui";
import axe from "axe-core";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, test } from "vitest";
import { sampleActions, sampleConnections, sampleRoutes } from "./gallery/fixtures.tsx";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

afterEach(() => {
  document.body.innerHTML = "";
});

async function mountShell(hash: string): Promise<HTMLElement> {
  document.documentElement.lang = "en"; // as index.html sets it
  const container = document.createElement("div");
  document.body.append(container);
  const route = sampleRoutes.find((candidate) => candidate.path === hash);
  await act(async () => createRoot(container).render(
    <ThemeProvider initialPreference="dark">
      <AppShell routes={sampleRoutes} actions={sampleActions} capabilities={{ "can.operate.local": true }} hash={hash}
        match={route ? { route, params: {} } : null} onNavigate={() => undefined} onActionError={() => undefined}
        connections={sampleConnections} notices={[{ id: 1, title: "Reconnecting to Archon", tone: "warning" }]} />
    </ThemeProvider>,
  ));
  for (let round = 0; round < 3; round += 1) await act(async () => new Promise((resolve) => setTimeout(resolve, 0)));
  return container;
}

async function violations(): Promise<string[]> {
  const result = await axe.run(document, { rules: { "color-contrast": { enabled: false } }, resultTypes: ["violations"] });
  return result.violations.map((violation) => `${violation.id}: ${violation.nodes.map((node) => node.target.join(" ")).join(", ")}`);
}

describe("axe on the shell", () => {
  test("the shell with a screen of every primitive has no violations", async () => {
    const container = await mountShell("#/fleet");
    expect(container.querySelectorAll("[role=listitem]").length).toBeGreaterThan(0);
    expect(container.querySelector(".ui-navrail")).not.toBeNull();
    expect(await violations()).toEqual([]);
  });

  test("the overview, a failed screen and the open palette have no violations", async () => {
    await mountShell("#/");
    expect(await violations()).toEqual([]);
    document.body.innerHTML = "";
    const failed = await mountShell("#/archon/queue");
    expect(failed.querySelector("[role=alert]")).not.toBeNull();
    await act(async () => document.dispatchEvent(new KeyboardEvent("keydown", { key: "k", ctrlKey: true, bubbles: true })));
    expect(failed.querySelector("[role=combobox]")).not.toBeNull();
    expect(await violations()).toEqual([]);
  });
});
