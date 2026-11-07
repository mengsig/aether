// @vitest-environment happy-dom
// Desktop alerts in Settings: shown on the desktop only, each switch saves this computer's
// setting and says so, the kinds follow the main switch, and the test sends a real notification.

import { ALERT_SETTINGS_KEY, DEFAULT_ALERT_SETTINGS } from "@aether-ui/core";
import type { DesktopBridge } from "@aether-ui/protocol";
import { act } from "react";
import { afterEach, describe, expect, test, vi } from "vitest";
import { fakeDesktop } from "../kit/fixture/desktop.test-util.ts";
import { click, connected, type Harness } from "../kit/fixture/harness.test-util.tsx";
import { TEST_ALERT } from "./DesktopAlerts.tsx";
import { NotificationsPage } from "./NotificationsPage.tsx";

let current: Harness | null = null;

afterEach(() => {
  current?.unmount();
  current = null;
});

function savedStorage(failing = false) {
  const saved = new Map<string, unknown>();
  return {
    saved,
    storage: {
      get: async (key: string) => saved.get(key),
      set: async (key: string, value: unknown) => {
        if (failing) throw new Error("disk full");
        saved.set(key, value);
      },
    },
  };
}

async function mountOnDesktop(storage: ReturnType<typeof savedStorage>["storage"], refusal?: string) {
  const notify = vi.fn(async () => { if (refusal !== undefined) throw new Error(refusal); });
  const bridge = { ...fakeDesktop().bridge, notify } as DesktopBridge;
  current = await connected({ desktop: bridge, storage });
  const root = await current.mount(<NotificationsPage conn={current.conn} />);
  return { root, notify, h: current };
}

const switchNamed = (root: ParentNode, label: string): HTMLInputElement => {
  const toggle = [...root.querySelectorAll("label.st-toggle")].find((entry) => entry.querySelector(".st-toggle-label")?.textContent === label);
  const input = toggle?.querySelector("input[role=switch]");
  if (!(input instanceof HTMLInputElement)) throw new Error(`a switch named ${label}`);
  return input;
};

async function flip(h: Harness, input: HTMLInputElement): Promise<void> {
  await act(async () => input.click());
  await h.settle();
}

describe("DesktopAlerts", () => {
  test("the browser has no desktop alerts section", async () => {
    current = await connected();
    const root = await current.mount(<NotificationsPage conn={current.conn} />);
    expect(root.textContent).not.toContain("Desktop alerts");
  });

  test("the switches start from the defaults and each one saves and says so", async () => {
    const { saved, storage } = savedStorage();
    const { root, h } = await mountOnDesktop(storage);
    expect(switchNamed(root, "Desktop alerts").checked).toBe(true);
    expect(switchNamed(root, "Also while Aether is in front").checked).toBe(false);
    await flip(h, switchNamed(root, "Approvals"));
    expect(saved.get(ALERT_SETTINGS_KEY)).toEqual({ ...DEFAULT_ALERT_SETTINGS, approvals: false });
    expect(root.textContent).toContain("Approvals off");
    await flip(h, switchNamed(root, "Desktop alerts"));
    for (const label of ["Questions", "Approvals", "Also while Aether is in front"]) expect(switchNamed(root, label).disabled).toBe(true);
    expect(saved.get(ALERT_SETTINGS_KEY)).toEqual({ ...DEFAULT_ALERT_SETTINGS, enabled: false, approvals: false });
  });

  test("a setting that could not be saved goes back and says why", async () => {
    const { storage } = savedStorage(true);
    const { root, h } = await mountOnDesktop(storage);
    await flip(h, switchNamed(root, "Questions"));
    expect(switchNamed(root, "Questions").checked).toBe(true);
    expect(root.textContent).toContain("Not saved: Error: disk full");
  });

  test("Send test notification shows a native notification that opens the inbox", async () => {
    const { storage } = savedStorage();
    const { root, notify, h } = await mountOnDesktop(storage);
    await click(h, root, "Send test notification");
    expect(notify).toHaveBeenCalledExactlyOnceWith(TEST_ALERT);
    expect(TEST_ALERT.route).toBe("/inbox");
    expect(root.textContent).toContain("Test notification sent.");
  });

  test("a test notification main refuses says why instead of claiming it was sent", async () => {
    const { storage } = savedStorage();
    const { root, notify, h } = await mountOnDesktop(storage, "Too many notifications; try again in a minute");
    await click(h, root, "Send test notification");
    expect(notify).toHaveBeenCalledOnce();
    expect(root.textContent).toContain("No test notification: Too many notifications; try again in a minute");
    expect(root.textContent).not.toContain("Test notification sent.");
  });
});
