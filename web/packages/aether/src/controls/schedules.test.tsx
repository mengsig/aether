// @vitest-environment happy-dom
import { drafts } from "@aether-ui/core";
// The Schedules tab: cron and spawn schedules, every policy, fleet-wide listing and the observer view.

import { act } from "react";
import { beforeEach, afterEach, describe, expect, test } from "vitest";
import { AGENT_ID, CHILD_ID, controlsScript, demoAgents } from "./fixture/demo.ts";
import { buttonNamed, click, connected, pick, type, type Harness } from "./fixture/harness.test-util.tsx";
import type { ScriptOptions } from "./fixture/daemon.ts";
import { SchedulesPanel } from "./schedules.tsx";
import { invariant } from "../fleet/invariant.ts";

let current: Harness | null = null;
afterEach(() => { current?.unmount(); current = null; });

async function open(options: ScriptOptions = {}) {
  const h = current = await connected({ agents: demoAgents(), ...options, script: { ...controlsScript(), ...options.script } });
  const agent = demoAgents().find((a) => a.id === AGENT_ID);
  expect(agent).toBeDefined();
  const root = await h.mount(agent ? <SchedulesPanel conn={h.conn} agent={agent} /> : null);
  return { h, root };
}

const sent = (h: Harness, cmd: string) => h.daemon.commands(cmd).map((c) => Object.fromEntries(Object.entries(c).filter(([k]) => k !== "req" && k !== "idempotency_key")));

function openDialog(): HTMLElement {
  const dialog = document.querySelector<HTMLElement>("dialog[open]");
  expect(dialog).not.toBeNull();
  return dialog ?? document.body;
}

/** The spawn workspace is a folder typed as "Other folder" and committed with Enter. */
async function typeFolder(h: Harness, root: ParentNode, folder: string): Promise<void> {
  const other = [...root.querySelectorAll<HTMLLabelElement>(".start-choice")].find(l => l.textContent?.includes("Other folder"))?.querySelector("input");
  invariant(other, "the Other folder choice");
  await act(async () => other.click()); await h.settle();
  await type(h, root, "Folder path", folder);
  const field = [...root.querySelectorAll<HTMLInputElement>("input")].find(i => i.value === folder);
  invariant(field, "the typed folder path");
  await act(async () => field.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }))); await h.settle();
}

describe("SchedulesPanel", () => {
  test("a cron schedule sends its expression, timezone, policies and delivery limit", async () => {
    const { h, root } = await open();
    await type(h, root, "Schedule name", "Weekday standup");
    await click(h, root, "Cron");
    await pick(h, root, "Cron preset", "Custom");
    await type(h, root, "Cron expression", "30 9 * * 1-5");
    await type(h, root, "Timezone", "Europe/Oslo");
    await type(h, root, "Message to deliver", "Summarise yesterday");
    await pick(h, root, "When the agent is busy", "Queue one");
    await pick(h, root, "Missed occurrences", "Skip them");
    await type(h, root, "Maximum deliveries (blank for unlimited)", "20");
    await click(h, root, "Create schedule");
    expect(sent(h, "create_schedule")).toEqual([{
      cmd: "create_schedule",
      spec: {
        name: "Weekday standup", target: AGENT_ID, recurrence: { kind: "cron", expression: "30 9 * * 1-5", timezone: "Europe/Oslo" },
        payload: "Summarise yesterday", busy: "queue_one", missed: "skip", max_deliveries: 20,
      },
    }]);
    expect(root.querySelector(".b3-note")?.textContent).toBe("Schedule created");
  });

  test("a cron schedule with a bad timezone is refused on screen and nothing is sent", async () => {
    const { h, root } = await open();
    await type(h, root, "Schedule name", "Standup");
    await click(h, root, "Cron");
    await type(h, root, "Timezone", "Mars/Olympus");
    await type(h, root, "Message to deliver", "Go");
    expect(buttonNamed(root, "Create schedule").disabled).toBe(true);
    expect(root.textContent).toContain("not an IANA timezone");
    expect(h.daemon.commands("create_schedule")).toHaveLength(0);
  });

  test("a spawn schedule asks first, then sends the spawn template with no target", async () => {
    const { h, root } = await open();
    await type(h, root, "Schedule name", "Nightly triage");
    await click(h, root, "A new agent");
    expect(root.textContent).not.toContain("When the agent is busy");
    await pick(h, root, "Every (seconds)", "Daily");
    await type(h, root, "Objective for each new agent", "Triage the backlog");
    await typeFolder(h, root, "/srv/repo");
    await pick(h, root, "Model", "Sonnet 5.5");
    await click(h, root, "high");
    await type(h, root, "Maximum runs (blank for unlimited)", "7");
    await click(h, root, "Create schedule");
    expect(h.daemon.commands("create_schedule")).toHaveLength(0);
    expect(openDialog().textContent).toContain("/srv/repo");
    await click(h, openDialog(), "Create schedule");
    expect(sent(h, "create_schedule")).toEqual([{
      cmd: "create_schedule",
      spec: {
        name: "Nightly triage", recurrence: { kind: "interval", seconds: 86_400 }, missed: "coalesce", max_deliveries: 7,
        spawn: {
          role: "worker", name: "Nightly triage", objective: "Triage the backlog", model: { provider: "anthropic", model: "claude-sonnet-5-5" },
          scope: { workspace_root: "/srv/repo" }, approval: { require_for: ["external"] }, config: { effort: "high" },
        },
      },
    }]);
  });

  test("an incomplete spawn schedule says what is missing before submitting", async () => {
    const { h, root } = await open();
    await type(h, root, "Schedule name", "Nightly triage");
    await click(h, root, "A new agent");
    expect(buttonNamed(root, "Create schedule").disabled).toBe(true);
    expect(root.textContent).toContain("Write the objective");
    expect(h.daemon.commands("create_schedule")).toHaveLength(0);
  });

  test("the listing is this agent's by default and fleet-wide on request, each under its own read key", async () => {
    const fleet = [
      { id: "sch_1", name: "Morning check", created_at: "2026-11-01T00:00:00Z", updated_at: "2026-11-01T00:00:00Z", deliveries: 12, missed: "coalesce", busy: "queue_one", max_deliveries: 30, owner: { actor: "operator" }, payload: "Check", recurrence: { kind: "interval", seconds: 86_400 }, status: { state: "active", next_due: "2026-11-05T00:00:00Z" }, target: AGENT_ID },
      { id: "sch_2", name: "Nightly triage", created_at: "2026-11-01T00:00:00Z", updated_at: "2026-11-01T00:00:00Z", deliveries: 3, missed: "skip", max_deliveries: 7, owner: { actor: "operator" }, payload: null, recurrence: { kind: "cron", expression: "0 2 * * *", timezone: "Europe/Oslo" }, status: { state: "paused" }, spawn: { role: "worker", objective: "Triage the backlog" } },
    ];
    const list = (command: Record<string, unknown>) => ({ tag: "schedules", data: command.target === undefined ? fleet : fleet.slice(0, 1) });
    const { h, root } = await open({ script: { list_schedules: list } });
    expect(sent(h, "list_schedules")).toEqual([{ cmd: "list_schedules", target: AGENT_ID }]);
    expect(root.textContent).not.toContain("Nightly triage");
    expect(buttonNamed(root, "This agent's schedules").getAttribute("aria-checked")).toBe("true");
    await click(h, root, "All schedules");
    expect(sent(h, "list_schedules")[1]).toEqual({ cmd: "list_schedules" });
    const rows = [...root.querySelectorAll('[aria-label="Schedules"] > li')].map((li) => li.textContent ?? "");
    expect(rows).toHaveLength(2);
    expect(rows[0]).toContain("Every 24 h");
    expect(rows[0]).toContain("if busy, queue one");
    expect(rows[0]).toContain("up to 30 deliveries");
    expect(rows[1]).toContain("Cron 0 2 * * * · Europe/Oslo");
    expect(rows[1]).toContain("Starts a new agent: Triage the backlog");
    expect(rows[1]).toContain("missed runs are skipped");
    expect(Object.keys(h.app.store.get().reads[h.conn] ?? {}).sort()).toEqual([`schedules:${AGENT_ID}`, "schedules:all"]);
  });

  test("a paused spawn schedule from the fleet listing can be resumed and cancelled after confirming", async () => {
    const paused = { id: "sch_2", name: "Nightly triage", created_at: "2026-11-01T00:00:00Z", updated_at: "2026-11-01T00:00:00Z", deliveries: 3, missed: "skip", owner: { actor: "operator" }, payload: null, recurrence: { kind: "interval", seconds: 3600 }, status: { state: "paused" }, spawn: { role: "worker", objective: "Triage" } };
    const { h, root } = await open({ script: { list_schedules: () => ({ tag: "schedules", data: [paused] }) } });
    await click(h, root, "All schedules");
    await click(h, root, "Resume");
    await click(h, root, "Cancel schedule");
    await click(h, openDialog(), "Cancel schedule");
    expect(sent(h, "update_schedule")).toEqual([
      { cmd: "update_schedule", id: "sch_2", action: "resume" },
      { cmd: "update_schedule", id: "sch_2", action: "cancel" },
    ]);
    await click(h, root, "History");
    expect(sent(h, "schedule_history")).toEqual([{ cmd: "schedule_history", id: "sch_2", limit: 20 }]);
  });

  test("an occurrence refreshes progress and open history, and links to the spawned agent", async () => {
    const schedule = { id: "sch_2", name: "Nightly triage", updated_at: "2026-11-04T00:00:00Z", deliveries: 0, missed: "skip", payload: null, recurrence: { kind: "cron", expression: "* * * * *", timezone: "UTC" }, status: { state: "active", next_due: "2026-11-05T00:00:00Z" }, spawn: { role: "worker", objective: "Triage" } };
    let fired = false;
    const occurrence = { schedule: "sch_2", due: "2026-11-05T00:00:00Z", recorded_at: "2026-11-05T00:00:01Z", outcome: { outcome: "spawned", agent: CHILD_ID, run: "run_1", task: "tsk_1" } };
    const { h, root } = await open({ script: {
      list_schedules: () => ({ tag: "schedules", data: [fired ? { ...schedule, deliveries: 1, status: { state: "finished" } } : schedule] }),
      schedule_history: () => ({ tag: "schedule_history", data: fired ? [occurrence] : [] }),
    } });
    await click(h, root, "All schedules");
    await click(h, root, "History");
    expect(root.textContent).toContain("0 started");
    expect(root.textContent).toContain("It has not run yet");
    fired = true;
    await act(async () => { h.daemon.emit(CHILD_ID, { type: "custom", name: "schedule.advanced", payload: { ...schedule, deliveries: 1 } }); });
    await h.settle();
    await h.settle();
    expect(root.textContent).toContain("Finished");
    expect(root.textContent).toContain("1 started");
    expect(root.textContent).toContain("changelog-writer · waiting");
    const link = root.querySelector<HTMLAnchorElement>('[aria-label="Schedule history"] a');
    expect(link?.getAttribute("href")).toBe(`#/agents/${encodeURIComponent(h.conn)}/${CHILD_ID}`);
    expect(link?.textContent).toBe("Open conversation");
    const before = h.daemon.commands("list_schedules").length;
    await click(h, root, "Refresh schedules");
    expect(h.daemon.commands("list_schedules")).toHaveLength(before + 1);
  });

  test("history keeps a link when the spawned agent is absent from the fleet cache", async () => {
    const missing = "agt_missing";
    const { h, root } = await open({ script: { schedule_history: () => ({ tag: "schedule_history", data: [
      { schedule: "sch_1", due: "2026-11-05T00:00:00Z", recorded_at: "2026-11-05T00:00:01Z", outcome: { outcome: "spawned", agent: missing, run: "run_1", task: "tsk_1" } },
      { schedule: "sch_1", due: "2026-11-06T00:00:00Z", recorded_at: "2026-11-06T00:00:01Z", outcome: { outcome: "skipped", reason: "busy" } },
    ] }) } });
    await click(h, root, "History");
    expect(root.querySelector('[aria-label="Schedule history"]')?.textContent).toContain(missing);
    const links = root.querySelectorAll<HTMLAnchorElement>('[aria-label="Schedule history"] a');
    expect(links).toHaveLength(1);
    expect(links[0]?.getAttribute("href")).toBe(`#/agents/${encodeURIComponent(h.conn)}/${missing}`);
  });

  test("an observer sees the create button disabled with the reason", async () => {
    const { h, root } = await open({ role: "observer" });
    const button = buttonNamed(root, "Create schedule");
    expect(button.disabled).toBe(true);
    expect(button.closest(".b3-action")?.querySelector(".b3-reason")?.textContent).toBe("Observer devices cannot change daemon state");
    await act(async () => button.click());
    expect(h.daemon.commands("create_schedule")).toHaveLength(0);
  });
});

beforeEach(() => { for (const key of drafts.keys()) drafts.clear(key); });
