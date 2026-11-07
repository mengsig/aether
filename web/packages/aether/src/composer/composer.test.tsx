// @vitest-environment happy-dom
// The chat composer: Enter sends through core and Shift+Enter does not, Stop interrupts a running
// turn, an ended agent says why it takes nothing, and the `/` menu works by keyboard and pointer.

import type { aether } from "@aether-ui/protocol";
import { act } from "react";
import { afterEach, describe, expect, test, vi } from "vitest";
import { controlsScript } from "../controls/fixture/demo.ts";
import type { Answer, ScriptOptions } from "../controls/fixture/daemon.ts";
import { buttonNamed, click, connected, type Harness } from "../controls/fixture/harness.test-util.tsx";
import { fauxAgent } from "../conversation/fixture/transcript.ts";
import { invariant } from "../fleet/invariant.ts";
import { Composer } from "./Composer.tsx";
import { clientCommands, hostCommands, mergeCommands, modeCommands, type SlashCommand } from "./slash.ts";
import { forgetDrafts } from "./drafts.ts";

let current: Harness | null = null;
afterEach(() => {
  current?.unmount();
  current = null;
  sessionStorage.clear();
  forgetDrafts();
  window.location.hash = "";
});

const running = fauxAgent();

async function open(agent: aether.Agent = running, options: ScriptOptions = {}, extra: readonly SlashCommand[] = [], unknownHeld: string | null = null) {
  const h = current = await connected({ agents: [agent], ...options, script: { ...controlsScript(), ...options.script } });
  const commands = mergeCommands(clientCommands(h.conn, agent), hostCommands(agent), extra);
  const root = await h.mount(<Composer conn={h.conn} agent={agent} commands={commands} unknownHeld={unknownHeld} />);
  return { h, root, input: textarea(root) };
}

function textarea(root: ParentNode): HTMLTextAreaElement {
  const input = root.querySelector("textarea");
  invariant(input instanceof HTMLTextAreaElement, "the composer has a textarea");
  return input;
}

async function write(h: Harness, input: HTMLTextAreaElement, value: string): Promise<void> {
  const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")?.set;
  invariant(setter !== undefined, "a textarea has a value setter");
  await act(async () => {
    setter.call(input, value);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await h.settle();
}

/** Presses `key` in the textarea; returns whether the composer took it (prevented its default). */
async function press(h: Harness, input: HTMLTextAreaElement, key: string, init: KeyboardEventInit = {}): Promise<boolean> {
  let taken = false;
  await act(async () => { taken = !input.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...init })); });
  await h.settle();
  return taken;
}

const sentMessages = (h: Harness) => h.daemon.commands("send_message").map((c) => ({ to: c.to, content: c.content }));
const options = (root: ParentNode) => [...root.querySelectorAll('[role="option"]')].map((o) => o.querySelector(".cmp-option-name")?.textContent);
const active = (root: ParentNode, input: HTMLTextAreaElement) => root.querySelector(`[id="${input.getAttribute("aria-activedescendant") ?? ""}"] .cmp-option-name`)?.textContent;

describe("Composer sending", () => {
  test("Enter sends the draft and clears it once delivered; Shift+Enter is left to the textarea", async () => {
    const { h, root, input } = await open();
    await write(h, input, "Check the parser");
    expect(await press(h, input, "Enter", { shiftKey: true })).toBe(false);
    expect(sentMessages(h)).toEqual([]);
    expect(await press(h, input, "Enter")).toBe(true);
    expect(sentMessages(h)).toEqual([{ to: running.id, content: "Check the parser" }]);
    expect(input.value).toBe("");
    expect(root.querySelector(".b3-note")?.textContent).toBe("Sent");
  });

  test("a failed send keeps the draft and shows why", async () => {
    const refuse = (): Answer => ({ code: "invalid_state", message: "agent agt_1 is completed" });
    const { h, root, input } = await open(running, { script: { send_message: refuse } });
    await write(h, input, "Still there?");
    await click(h, root, "Send");
    expect(input.value).toBe("Still there?");
    expect(root.querySelector('[role="alert"]')?.textContent).toContain("is completed");
  });

  test("Stop interrupts a running agent and is absent while it waits", async () => {
    const { h, root } = await open();
    await click(h, root, "Stop");
    expect(h.daemon.commands("interrupt")).toEqual([expect.objectContaining({ cmd: "interrupt", agent: running.id })]);
    current?.unmount();
    const waiting = await open(fauxAgent(undefined, { status: { state: "waiting", condition: { on: "inbox" } } as aether.AgentStatus }));
    expect(() => buttonNamed(waiting.root, "Stop")).toThrow();
  });

  test("an observer sees Send and Stop off with the reason", async () => {
    const { h, root, input } = await open(running, { role: "observer" });
    await write(h, input, "hello");
    expect(buttonNamed(root, "Send").disabled).toBe(true);
    expect(buttonNamed(root, "Stop").disabled).toBe(true);
    expect(root.querySelector(".cmp-hint")?.textContent).toBe("Observer devices cannot change daemon state");
  });
});

describe("Composer for an ended agent", () => {
  test("a completed agent says why it takes no messages and offers a new chat; app commands still run", async () => {
    const done = fauxAgent(undefined, { status: { state: "completed", summary: "Shipped it" } });
    const { h, root, input } = await open(done);
    expect(root.querySelector(".cmp-ended")?.textContent).toContain("builder finished its task: Shipped it");
    expect(root.querySelector('.cmp-ended a[href="#/activity"]')?.textContent).toBe("Start a new chat");
    await write(h, input, "one more thing");
    expect(buttonNamed(root, "Send").disabled).toBe(true);
    expect(await press(h, input, "Enter")).toBe(true);
    expect(sentMessages(h)).toEqual([]);
    await write(h, input, "/settings");
    await press(h, input, "Enter");
    expect(window.location.hash).toBe("#/settings");
  });

  test("a failed agent can be woken from the notice", async () => {
    const failed = fauxAgent(undefined, { status: { state: "failed", error: "provider quota" } });
    const { h, root } = await open(failed);
    expect(root.textContent).toContain("builder failed: provider quota");
    await click(h, root, "Wake it to continue");
    expect(h.daemon.commands("wake")).toEqual([expect.objectContaining({ agent: failed.id })]);
  });
});

describe("Composer drafts", () => {
  test("a draft survives leaving and coming back, per agent, and a lost reply keeps it", async () => {
    const first = await open(running, { script: { send_message: () => "hold" } });
    await write(first.h, first.input, "half a thought");
    first.h.unmount();
    current = null;
    const other = await open(fauxAgent("agt_01JFAUX0000000000000000009"));
    expect(other.input.value).toBe("");
    other.h.unmount();
    const back = await open(running, { script: { send_message: () => "hold" } });
    expect(back.input.value).toBe("half a thought");
    await press(back.h, back.input, "Enter");
    back.h.daemon.drop();
    await back.h.settle();
    expect(back.root.querySelector('[data-state="unconfirmed"]')).not.toBeNull();
    expect(back.input.value).toBe("half a thought");
  });

  test("a stored value that is not a draft is not shown", async () => {
    const h = current = await connected({ agents: [running] });
    const key = `aether.draft.v1:${JSON.stringify([h.conn, running.id, "message"])}`;
    sessionStorage.setItem(key, "{not json");
    const root = await h.mount(<Composer conn={h.conn} agent={running} commands={[]} unknownHeld={null} />);
    expect(textarea(root).value).toBe("");
  });
});

describe("the slash menu", () => {
  test("opens on /, filters by prefix and moves with the arrow keys, as a combobox", async () => {
    const { h, root, input } = await open();
    expect(input.getAttribute("aria-expanded")).toBe("false");
    await write(h, input, "/");
    expect(input.getAttribute("aria-expanded")).toBe("true");
    expect(root.querySelector('[role="listbox"]')?.id).toBe(input.getAttribute("aria-controls"));
    expect(options(root)).toContain("/memory");
    expect(root.querySelector('[role="group"] .cmp-group-name')?.textContent).toBe("App");
    await write(h, input, "/s");
    expect(options(root)).toEqual(["/settings", "/schedules"]);
    expect(active(root, input)).toBe("/settings");
    expect(await press(h, input, "ArrowDown")).toBe(true);
    expect(active(root, input)).toBe("/schedules");
    await press(h, input, "ArrowDown");
    expect(active(root, input)).toBe("/settings");
    await press(h, input, "ArrowUp");
    expect(active(root, input)).toBe("/schedules");
    expect(root.querySelector('[aria-selected="true"] .cmp-option-about')?.textContent).toContain("recurring timers");
    expect(await press(h, input, "Escape")).toBe(true);
    expect(root.querySelector('[role="listbox"]')).toBeNull();
  });

  test("Tab completes the name, then its choices are offered and Enter runs the one picked", async () => {
    const { h, root, input } = await open();
    await write(h, input, "/set");
    expect(await press(h, input, "Tab")).toBe(true);
    expect(input.value).toBe("/settings ");
    expect(options(root)).toEqual(["connections", "devices", "remote", "providers", "notifications"]);
    await write(h, input, "/settings d");
    expect(options(root)).toEqual(["devices"]);
    await press(h, input, "Enter");
    expect(window.location.hash).toBe("#/settings/devices");
    expect(input.value).toBe("");
  });

  test("pointer hover highlights an option and a click runs it", async () => {
    const { h, root, input } = await open();
    await write(h, input, "/");
    const memory = [...root.querySelectorAll('[role="option"]')].find((o) => o.querySelector(".cmp-option-name")?.textContent === "/memory");
    invariant(memory instanceof HTMLElement, "the menu offers /memory");
    await act(async () => { memory.dispatchEvent(new MouseEvent("mousemove", { bubbles: true })); });
    expect(memory.getAttribute("aria-selected")).toBe("true");
    await act(async () => memory.click());
    await h.settle();
    expect(window.location.hash).toBe(`#/memory/${encodeURIComponent(h.conn)}`);
  });

  test("/help opens the shell's command palette with its own shortcut", async () => {
    const { h, input } = await open();
    const seen: KeyboardEvent[] = [];
    const listen = (event: KeyboardEvent) => seen.push(event);
    document.addEventListener("keydown", listen);
    await write(h, input, "/help");
    await press(h, input, "Enter");
    document.removeEventListener("keydown", listen);
    expect(seen.filter((e) => e.ctrlKey && e.key === "k")).toHaveLength(1);
    expect(input.value).toBe("");
  });

  test("an unknown command and a host command are sent to the agent exactly as typed", async () => {
    const archon = fauxAgent(undefined, { role: "archon-orchestrator" });
    const { h, root, input } = await open(archon);
    await write(h, input, "/");
    expect([...root.querySelectorAll(".cmp-group-name")].map((g) => g.textContent)).toEqual(["App", "Archon"]);
    await write(h, input, "/frobnicate the widget");
    await press(h, input, "Enter");
    await write(h, input, "/stat");
    await press(h, input, "Enter");
    expect(input.value).toBe("");
    await write(h, input, "/answer");
    await press(h, input, "Enter");
    expect(input.value).toBe("/answer ");
    await write(h, input, "/answer 2 yes, ship it");
    await press(h, input, "Enter");
    expect(sentMessages(h).map((m) => m.content)).toEqual(["/frobnicate the widget", "/status", "/answer 2 yes, ship it"]);
  });

  test("Archon's commands are offered only to its orchestrator", () => {
    expect(hostCommands(running)).toEqual([]);
    const host = hostCommands(fauxAgent(undefined, { role: "archon-orchestrator" }));
    expect(host.map((c) => c.name)).toEqual(["answer", "questions", "approve", "reject", "merge", "archon", "status", "queue", "item", "repos", "lessons", "update"]);
    expect(host.every((c) => c.run === null && c.group === "Archon")).toBe(true);
    expect(host.find((c) => c.name === "merge")).toMatchObject({ args: "<id> [--ignore-checks]", about: "merge the item's pull request now" });
  });

  test("view modes: /mode sets one from its choices, /<name> toggles a non-standard one, a bad name is explained", async () => {
    const choose = vi.fn();
    const toggle = vi.fn();
    const modes = [
      { name: "standard", label: "Standard", description: "the default" },
      { name: "verbose", label: "Verbose", description: "every tool call" },
      { name: "status", label: "Status", description: "shadowed by a command" },
      { name: "quiet_work", label: "Quiet", description: "a name the daemon allows" },
    ];
    const extra = modeCommands(modes, choose, toggle, "unused");
    expect(extra.map((c) => c.name)).toEqual(["mode", "verbose", "quiet_work"]);
    const { h, root, input } = await open(running, {}, extra);
    await write(h, input, "/mode v");
    expect(options(root)).toEqual(["verbose"]);
    await press(h, input, "Enter");
    expect(choose).toHaveBeenCalledWith("verbose");
    await write(h, input, "/verbose");
    await press(h, input, "Enter");
    expect(toggle).toHaveBeenCalledWith("verbose");
    await write(h, input, "/mode loud");
    await press(h, input, "Enter");
    expect(root.querySelector('[role="alert"]')?.textContent).toBe("Choose a view mode: standard, verbose, status, quiet_work");
    expect(input.value).toBe("/mode loud");
    await write(h, input, "/quiet_work");
    await press(h, input, "Escape");
    await press(h, input, "Enter");
    expect(toggle).toHaveBeenCalledWith("quiet_work");
    expect(sentMessages(h), "view commands never reach the agent").toEqual([]);
  });

  test.each(["View modes are still loading", "View modes unavailable: connection lost"])(
    "without the daemon's catalog /mode, /focus and /calm refuse here instead of reaching the agent (%s)", async (reason) => {
      const choose = vi.fn();
      const extra = modeCommands([], choose, choose, reason);
      const { h, root, input } = await open(running, {}, extra, reason);
      for (const line of ["/mode focus", "/focus", "/calm"]) {
        await write(h, input, line);
        await press(h, input, "Escape");
        await press(h, input, "Enter");
        expect(root.querySelector('[role="alert"]')?.textContent).toBe(reason);
        expect(input.value, "the line stays to run again").toBe(line);
      }
      for (const line of ["/brief", "/brief now"]) {
        await write(h, input, line);
        await press(h, input, "Enter");
        expect(root.querySelector('[role="alert"]')?.textContent, "a user's own mode may be named so").toBe(`${reason}, so /brief is not sent: it may be a view mode`);
        expect(input.value).toBe(line);
      }
      expect(choose).not.toHaveBeenCalled();
      expect(sentMessages(h)).toEqual([]);
      await write(h, input, "/home/me/notes.md is ready");
      await press(h, input, "Enter");
      expect(sentMessages(h), "a path is not a command").toEqual([{ to: running.id, content: "/home/me/notes.md is ready" }]);
    });

  test("a / line reaches the agent byte for byte, while a message is trimmed", async () => {
    const { h, input } = await open(running, {}, modeCommands([{ name: "standard", label: "Standard", description: "" }], vi.fn(), vi.fn(), "unused"), null);
    await write(h, input, "/custom  two  spaces \n  ");
    await press(h, input, "Escape");
    await press(h, input, "Enter");
    await write(h, input, "  plain words \n");
    await press(h, input, "Enter");
    expect(sentMessages(h)).toEqual([{ to: running.id, content: "/custom  two  spaces \n  " }, { to: running.id, content: "plain words" }]);
  });

  test("with the catalog read, an unknown /name goes to the agent as typed", async () => {
    const { h, input } = await open(running, {}, modeCommands([{ name: "standard", label: "Standard", description: "" }], vi.fn(), vi.fn(), "unused"), null);
    await write(h, input, "/brief");
    await press(h, input, "Escape");
    await press(h, input, "Enter");
    expect(sentMessages(h)).toEqual([{ to: running.id, content: "/brief" }]);
  });

  test.each([{ method: "getItem", store: "an unreadable" }, { method: "setItem", store: "a full" }] as const)(
    "with $store store, text typed while a send is pending outlives its confirmation", async ({ method }) => {
      const broken = vi.spyOn(sessionStorage, method).mockImplementation(() => { throw new DOMException("denied", "SecurityError"); });
      let reply: Answer = "hold";
      try {
        const { h, root, input } = await open(running, { script: { send_message: () => reply } });
        await write(h, input, "first");
        expect(root.textContent, "the draft says it is not kept across a reload").toContain("not across a reload");
        await press(h, input, "Enter");
        await write(h, input, "second, typed while the first was in flight");
        reply = { tag: "message_sent", data: { seq: 1 } };
        h.daemon.release();
        await h.settle();
        expect(input.value).toBe("second, typed while the first was in flight");
        await press(h, input, "Enter");
        await h.settle();
        expect(input.value, "a confirmed draft still clears").toBe("");
        expect(sentMessages(h).map((m) => m.content)).toEqual(["first", "second, typed while the first was in flight"]);
      } finally {
        broken.mockRestore();
      }
    });

  test("a menu pick waits like Send: never twice while a send is pending, never to an ended agent", async () => {
    const orchestrator = fauxAgent(undefined, { role: "archon-orchestrator" });
    const { h, input } = await open(orchestrator, { script: { send_message: () => "hold" } });
    await write(h, input, "/status");
    await press(h, input, "Enter");
    await write(h, input, "/status");
    await press(h, input, "Enter");
    expect(sentMessages(h)).toEqual([{ to: orchestrator.id, content: "/status" }]);
    current?.unmount();
    const ended = fauxAgent(undefined, { role: "archon-orchestrator", status: { state: "completed", summary: "done" } });
    const again = await open(ended);
    await write(again.h, again.input, "/queue");
    await press(again.h, again.input, "Enter");
    expect(sentMessages(again.h)).toEqual([]);
  });
});
