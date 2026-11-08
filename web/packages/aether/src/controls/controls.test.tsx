// @vitest-environment happy-dom
import { drafts } from "@aether-ui/core";
// The agent controls: each control sends its command through core, destructive ones only after a
// confirm step; an observer sees every control off with the reason; reads land in core's store.

import { act } from "react";
import { beforeEach, afterEach, describe, expect, test, vi } from "vitest";
import { invariant } from "../fleet/invariant.ts";
import { ControlsScreen } from "./ControlsScreen.tsx";
import { previewText } from "./records.tsx";
import { AGENT_ID, CHILD_ID, controlsScript, demoAgents } from "./fixture/demo.ts";
import { buttonNamed, click, connected, pick, type, type Harness } from "./fixture/harness.test-util.tsx";
import type { ScriptOptions } from "./fixture/daemon.ts";
import { forgetDrafts } from "../composer/drafts.ts";

let current: Harness | null = null;
afterEach(() => { current?.unmount(); current = null; sessionStorage.clear(); forgetDrafts(); });

async function open(options: ScriptOptions = {}, tab?: string) {
  const h = current = await connected({ agents: demoAgents(), ...options, script: { ...controlsScript(), ...options.script } });
  const root = await h.mount(<ControlsScreen conn={h.conn} id={AGENT_ID} />);
  if (tab) {
    const button = [...root.querySelectorAll('[role="tab"]')].find((t) => t.textContent === tab) as HTMLButtonElement | undefined;
    expect(button).toBeDefined();
    await act(async () => button?.click());
    await h.settle();
    await h.settle();
  }
  return { h, root };
}

function openDialog(): HTMLElement {
  const dialog = document.querySelector<HTMLElement>("dialog[open]");
  expect(dialog).not.toBeNull();
  return dialog ?? document.body;
}

function required<T>(value: T | null | undefined): T {
  invariant(value !== null && value !== undefined, "test fixture value exists");
  return value;
}

/** The commands sent, without the request id and idempotency key core adds. */
const sent = (h: Harness, cmd: string) => h.daemon.commands(cmd).map((c) => Object.fromEntries(Object.entries(c).filter(([k]) => k !== "req" && k !== "idempotency_key")));

describe("ControlsScreen", () => {
  test("lifecycle controls send their commands; pause can include the delegates", async () => {
    const { h, root } = await open();
    await click(h, root, "Interrupt");
    await click(h, root, "Include the agents it delegated to");
    await click(h, root, "Pause");
    await click(h, root, "Compact context");
    expect(sent(h, "interrupt")).toEqual([{ cmd: "interrupt", agent: AGENT_ID }]);
    expect(sent(h, "pause")).toEqual([{ cmd: "pause", agent: AGENT_ID, subtree: true }]);
    expect(sent(h, "compact")).toEqual([{ cmd: "compact", agent: AGENT_ID }]);
    expect(root.querySelector(".b3-note")?.textContent).toBe("Compacting");
  });

  test("restart, archive and cancel ask first; keeping it sends nothing, confirming sends once", async () => {
    const { h, root } = await open();
    await click(h, root, "Cancel agent");
    await click(h, document.body, "Keep it");
    expect(h.daemon.commands("cancel")).toHaveLength(0);
    await click(h, root, "Cancel agent");
    await type(h, document.body, "Reason (optional)", "superseded by 1.4");
    await click(h, openDialog(), "Cancel agent");
    expect(sent(h, "cancel")).toEqual([{ cmd: "cancel", agent: AGENT_ID, reason: "superseded by 1.4" }]);
  });

  test.each(["completed", "failed", "cancelled", "imported"] as const)("%s agent exposes only available lifecycle actions and cannot be messaged", async (state) => {
    const agents = demoAgents();
    const agent = required(agents[0]);
    const status = state === "failed" ? { state, error: "failed" } as const
      : state === "imported" ? { state, was: "completed" } as const : { state };
    agents[0] = { ...agent, status };
    const { h, root } = await open({ agents });
    for (const label of ["Interrupt", "Pause", "Compact context"]) {
      const button = buttonNamed(root, label);
      expect(button.disabled).toBe(true);
      expect(document.getElementById(required(button.getAttribute("aria-describedby")))?.textContent).toContain(state);
    }
    expect(buttonNamed(root, "Wake").disabled).toBe(state !== "failed");
    expect(buttonNamed(root, "Restart").disabled).toBe(state !== "failed" && state !== "cancelled");
    expect(buttonNamed(root, "Archive").disabled).toBe(false);
    if (state === "failed") {
      await click(h, root, "Wake");
      expect(sent(h, "wake")).toEqual([{ cmd: "wake", agent: AGENT_ID }]);
    }
    await click(h, root, "Steer");
    const send = buttonNamed(root, "Send");
    expect(send.disabled).toBe(true);
    expect(document.getElementById(required(send.getAttribute("aria-describedby")))?.textContent).toContain(state);
    expect(sent(h, "send_message")).toEqual([]);
  });

  test("restart and archive require confirmation for a failed agent", async () => {
    const agents = demoAgents();
    agents[0] = { ...required(agents[0]), status: { state: "failed", error: "failed" } };
    const { h, root } = await open({ agents });
    await click(h, root, "Restart");
    expect(sent(h, "restart")).toEqual([]);
    await click(h, openDialog(), "Restart");
    await click(h, root, "Archive");
    await click(h, openDialog(), "Archive");
    expect(sent(h, "restart")).toEqual([{ cmd: "restart", agent: AGENT_ID }]);
    expect(sent(h, "archive")).toEqual([{ cmd: "archive", target: { agent: AGENT_ID }, archived: true }]);
  });

  test("a restarted failed agent cannot be woken or restarted again", async () => {
    const agents = demoAgents();
    agents[0] = { ...required(agents[0]), status: { state: "failed", error: "failed" }, restarted_as: CHILD_ID };
    const { root } = await open({ agents });
    for (const label of ["Wake", "Restart"]) {
      const button = buttonNamed(root, label);
      expect(button.disabled).toBe(true);
      expect(document.getElementById(required(button.getAttribute("aria-describedby")))?.textContent).toContain(CHILD_ID);
    }
  });

  test("waiting agents can resume without waking or pausing", async () => {
    const agents = demoAgents();
    agents[0] = { ...required(agents[0]), status: required(agents[1]).status };
    const { h, root } = await open({ agents });
    expect(buttonNamed(root, "Wake").disabled).toBe(true);
    await click(h, root, "Resume");
    expect(sent(h, "resume")).toEqual([{ cmd: "resume", agent: AGENT_ID }]);
  });

  test("a paused ended agent can resume its subtree and an archived agent can be restored", async () => {
    const agents = demoAgents();
    agents[0] = { ...required(agents[0]), status: { state: "completed" }, paused: true, archived: true };
    const { h, root } = await open({ agents });
    expect(buttonNamed(root, "Resume").disabled).toBe(true);
    await click(h, root, "Include the agents it delegated to");
    await click(h, root, "Resume");
    await click(h, root, "Restore");
    expect(sent(h, "resume")).toEqual([{ cmd: "resume", agent: AGENT_ID, subtree: true }]);
    expect(sent(h, "archive")).toEqual([{ cmd: "archive", target: { agent: AGENT_ID }, archived: false }]);
  });

  test("an observer sees every control disabled with the reason, and the standing notice once", async () => {
    const { root } = await open({ role: "observer" });
    expect(root.querySelector(".b3-standing")?.textContent).toContain("Observer devices cannot change daemon state");
    const actions = [...root.querySelectorAll(".b3-action")];
    expect(actions.length).toBeGreaterThanOrEqual(7);
    for (const action of actions) {
      expect(action.querySelector("button")?.disabled).toBe(true);
      expect(action.querySelector(".b3-reason")?.textContent).toBe("Observer devices cannot change daemon state");
    }
  });

  test("a lost cancel reply is reported unconfirmed with the way to the recovery review, and not resent", async () => {
    const { h, root } = await open({ script: { cancel: () => "hold" } });
    await click(h, root, "Cancel agent");
    await click(h, openDialog(), "Cancel agent");
    h.daemon.drop();
    await h.settle();
    const note = root.querySelector(".b3-note");
    expect(note?.getAttribute("data-state")).toBe("unconfirmed");
    expect(note?.querySelector("a")?.getAttribute("href")).toBe("#/inbox");
    expect(h.daemon.commands("cancel")).toHaveLength(1);
    expect(h.app.store.get().aether[h.conn]?.outbox).toEqual([expect.objectContaining({ state: "unknown" })]);
  });

  test("steering sends a trimmed message, to the delegates when asked, and refuses an empty one", async () => {
    const { h, root } = await open({}, "Steer");
    expect(buttonNamed(root, "Send").disabled).toBe(true);
    expect(root.textContent).toContain("Write a message");
    await type(h, root, "Message", "  Hold the release until CI is green  ");
    await click(h, root, "Also send to the agents it delegated to");
    await click(h, root, "Send");
    expect(sent(h, "send_message")).toEqual([{ cmd: "send_message", to: AGENT_ID, content: "Hold the release until CI is green", subtree: true }]);
  });

  test("an unsent Steer message survives leaving the screen and is cleared only once delivered", async () => {
    const first = await open({ script: { send_message: () => ({ code: "unavailable", message: "daemon busy" }) } }, "Steer");
    await type(first.h, first.root, "Message", "Hold the release");
    await click(first.h, first.root, "Send");
    expect(first.root.textContent).toContain("daemon busy");
    first.h.unmount();
    const back = await open({}, "Steer");
    const field = back.root.querySelector("textarea");
    expect(field?.value).toBe("Hold the release");
    await click(back.h, back.root, "Send");
    expect(field?.value).toBe("");
    back.h.unmount();
    current = (await open({}, "Steer")).h;
    expect(document.querySelector("textarea")?.value).toBe("");
  });

  test("a Steer message the session store cannot keep says so", async () => {
    const setItem = vi.spyOn(sessionStorage, "setItem").mockImplementation(() => { throw new DOMException("full", "QuotaExceededError"); });
    try {
      const { h, root } = await open({}, "Steer");
      await type(h, root, "Message", "Hold the release");
      expect(root.textContent).toContain("Drafts are kept while the app is open, but not across a reload (full).");
    } finally {
      setItem.mockRestore();
    }
  });

  test("model and limits read the model list into the store and patch only what changed", async () => {
    const { h, root } = await open({}, "Model & limits");
    expect(h.app.store.get().reads[h.conn]?.["configure:models"]?.value).toHaveLength(3);
    await pick(h, root, "Model", "Sonnet 5.5");
    await click(h, root, "max");
    await click(h, root, "Apply model");
    await type(h, root, "Turn limit", "2.5");
    expect(buttonNamed(root, "Apply limits").disabled).toBe(true);
    expect(root.textContent).toContain("The turn limit must be a whole number");
    await type(h, root, "Turn limit", "50");
    await type(h, root, "Cost budget (USD)", "12.5");
    await click(h, root, "Apply limits");
    await type(h, root, "New name", "release-lead");
    await click(h, root, "Rename");
    expect(sent(h, "update_agent").map((c) => c.patch)).toEqual([
      { model: { provider: "anthropic", model: "claude-sonnet-5-5" }, effort: "max" },
      { cost_budget: 12.5, max_turns: 50 },
      { name: "release-lead" },
    ]);
  });

  test("approvals: policy, standing rules from the store, add and confirmed revoke reread the rules", async () => {
    const { h, root } = await open({}, "Approvals");
    expect(buttonNamed(root, "Apply approval policy").disabled).toBe(true);
    await click(h, root, "Changes");
    await click(h, root, "Apply approval policy");
    expect(sent(h, "set_approvals")).toEqual([{ cmd: "set_approvals", agent: AGENT_ID, policy: { require_for: ["mutation", "external"] } }]);
    const rules = root.querySelector('[aria-label="Standing rules"]');
    expect(rules?.querySelectorAll("li")).toHaveLength(2);
    expect(rules?.textContent).toContain("Everywhere");
    await type(h, root, "Tool", "shell");
    await type(h, root, "Argument (optional)", "npm test");
    await click(h, root, "Add rule");
    expect(sent(h, "add_approval_rule")).toEqual([]);
    expect(openDialog().textContent).toContain("approves calls to tool shell, argument \"npm test\" for this agent");
    await click(h, openDialog(), "Add rule");
    expect(sent(h, "add_approval_rule")).toEqual([{ cmd: "add_approval_rule", decision: "approved", scope: { scope: "agent", id: AGENT_ID }, match: { tool: "shell", argument: "npm test" } }]);
    await click(h, root, "Revoke");
    await click(h, openDialog(), "Revoke rule");
    expect(sent(h, "revoke_approval_rule")).toEqual([{ cmd: "revoke_approval_rule", id: "rul_1" }]);
    expect(h.daemon.commands("list_approval_rules")).toHaveLength(3);
  });

  test.each([
    ["operator", true],
    ["local", false],
  ] as const)("an everywhere rule's revoke is host-only, refused with its reason for a %s device: %s", async (role, refused) => {
    const { root } = await open({ role }, "Approvals");
    const rows = [...root.querySelectorAll<HTMLElement>('[aria-label="Standing rules"] li')];
    const everywhere = rows.find((row) => row.textContent?.includes("Everywhere"));
    const agentRule = rows.find((row) => !row.textContent?.includes("Everywhere"));
    invariant(everywhere !== undefined && agentRule !== undefined, "the demo lists an everywhere rule and an agent rule");
    expect(buttonNamed(everywhere, "Revoke").disabled).toBe(refused);
    expect(everywhere.textContent).toContain("Host only");
    expect(everywhere.querySelector(".b3-reason")?.textContent ?? null).toBe(refused ? "Only the daemon's own host can do this" : null);
    expect(buttonNamed(agentRule, "Revoke").disabled).toBe(false);
    expect(agentRule.textContent).not.toContain("Host only");
  });

  test("schedules: list, pause, confirmed cancel, history and a validated new schedule", async () => {
    const { h, root } = await open({}, "Schedules");
    expect(sent(h, "list_schedules")[0]).toEqual({ cmd: "list_schedules", target: AGENT_ID });
    expect(root.textContent).toContain("Morning check");
    await click(h, root, "Pause");
    expect(sent(h, "update_schedule")).toEqual([{ cmd: "update_schedule", id: "sch_1", action: "pause" }]);
    await click(h, root, "Cancel schedule");
    await click(h, openDialog(), "Cancel schedule");
    expect(sent(h, "update_schedule")[1]).toEqual({ cmd: "update_schedule", id: "sch_1", action: "cancel" });
    await click(h, root, "History");
    expect(root.querySelector('[aria-label="Schedule history"]')?.textContent).toContain("delivered");
    await type(h, root, "Schedule name", "Hourly sweep");
    await pick(h, root, "Every (seconds)", "Custom");
    await type(h, root, "Custom interval (seconds)", "30");
    await type(h, root, "Message to deliver", "Sweep stale branches");
    expect(buttonNamed(root, "Create schedule").disabled).toBe(true);
    expect(root.textContent).toContain("The interval is at least 60 seconds");
    await pick(h, root, "Every (seconds)", "Hourly");
    await click(h, root, "Create schedule");
    expect(sent(h, "create_schedule")[0]).toMatchObject({ spec: { name: "Hourly sweep", target: AGENT_ID, recurrence: { kind: "interval", seconds: 3600 }, payload: "Sweep stale branches" } });
  });

  test("records: the run report, an artifact preview, deliveries and runs, all held in the store", async () => {
    const { h, root } = await open({}, "Records");
    expect(root.textContent).toContain("Ship the 1.3 release");
    expect(root.textContent).toContain("$3.42");
    await click(h, root, "Preview");
    expect(root.querySelector(".b3-pre")?.textContent).toContain("Faster fleet view");
    expect(root.querySelector('[aria-label="Deliveries"]')?.textContent).toContain("wake");
    const reads = Object.keys(h.app.store.get().reads[h.conn] ?? {}).sort();
    expect(reads).toEqual(["artifact_bytes:art_1", `artifacts:${AGENT_ID}`, `inbox:${AGENT_ID}`, "run_report:run_01J0B3CONTROLS000000001", "runs"]);
  });

  test("a failed read shows its error with a retry that rereads into the store", async () => {
    let fail = true;
    const { h, root } = await open({ script: { list_schedules: () => (fail ? { code: "unavailable", message: "store busy" } : { tag: "schedules", data: [] }) } }, "Schedules");
    expect(root.querySelector('[data-kind="error"]')?.textContent).toContain("store busy");
    fail = false;
    await click(h, root, "Try again");
    expect(root.textContent).toContain("No schedules deliver to this agent");
    expect(h.app.store.get().reads[h.conn]?.[`schedules:${AGENT_ID}`]).toMatchObject({ value: [], error: null });
  });

  test("an unknown agent and a missing connection each say so", async () => {
    const h = current = await connected({ agents: demoAgents() });
    const root = await h.mount(<><ControlsScreen conn={h.conn} id="agt_missing" /><ControlsScreen conn="aether:gone" id={AGENT_ID} /></>);
    expect(root.textContent).toContain("has no agent agt_missing");
    expect(root.textContent).toContain("No connection aether:gone is open");
  });
});

describe("previewText", () => {
  test("decodes text and recognises binary bytes", () => {
    expect(previewText({ base64: btoa("hello"), truncated: true })).toEqual({ text: "hello", truncated: true, size: 5 });
    expect(previewText({ base64: btoa(String.fromCharCode(0xff, 0xfe, 0x00)) }).text).toBeNull();
    expect(() => previewText({})).toThrow();
  });
});

beforeEach(() => { for (const key of drafts.keys()) drafts.clear(key); });

test("approval tool suggestions include live requests and documented harness tools", async () => {
  const { h, root } = await open({}, "Approvals");
  await act(async () => { h.daemon.emit(AGENT_ID, { type: "approval.requested", approval: "apr_custom", call: "call_custom", tool: "custom_search", side_effect: "read_only", description: "Search" }); });
  await h.settle();
  await type(h, root, "Tool", "inspect");
  expect([...root.querySelectorAll<HTMLElement>('[role="option"]')].some(c => c.textContent === "inspect_state")).toBe(true);
  await type(h, root, "Tool", "custom");
  const choices = [...root.querySelectorAll<HTMLElement>('[role="option"]')];
  const option = choices.find(c => c.textContent === "custom_search");
  expect(option).toBeDefined();
  await act(async () => option?.click());
  await click(h, root, "Add rule");
  await click(h, openDialog(), "Add rule");
  expect(h.daemon.commands("add_approval_rule")[0]).toMatchObject({ match: { tool: "custom_search" } });
});

test("all approval policy switches preserve side-effect values and subtree", async () => {
  const { h, root } = await open({}, "Approvals");
  await click(h, root, "Reads");
  await click(h, root, "Repeatable changes");
  await click(h, root, "Changes");
  await click(h, root, "External effects");
  await click(h, root, "Also for the agents it delegated to");
  await click(h, root, "Apply approval policy");
  expect(h.daemon.commands("set_approvals")[0]).toMatchObject({ agent: AGENT_ID, policy: { require_for: ["read_only", "idempotent_mutation", "mutation"] }, subtree: true });
});
