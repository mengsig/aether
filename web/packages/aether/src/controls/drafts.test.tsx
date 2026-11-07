// @vitest-environment happy-dom
import { drafts } from "@aether-ui/core";
import { beforeEach, afterEach, expect, test } from "vitest";
import { ControlsScreen } from "./ControlsScreen.tsx";
import { AGENT_ID, CHILD_ID, controlsScript, demoAgents } from "./fixture/demo.ts";
import { connected, click, type, pick, type Harness } from "./fixture/harness.test-util.tsx";
import { invariant } from "../fleet/invariant.ts";

let h: Harness;
afterEach(() => h?.unmount());
async function screen(tab: string, id = AGENT_ID) {
  h = await connected({ agents: demoAgents(), script: controlsScript() });
  const root = await h.mount(<ControlsScreen conn={h.conn} id={id} />);
  await click(h, root, tab);
  return root;
}

test("the schedule draft survives navigation and clears after a successful command", async () => {
  let root = await screen("Schedules");
  await type(h, root, "Schedule name", "Persisted schedule");
  await type(h, root, "Message to deliver", "Check work");
  await click(h, root, "Cron");
  await pick(h, root, "Cron preset", "Every weekday at 09:00");
  await type(h, root, "Timezone", "UTC");
  h.unmount();
  root = await screen("Schedules", CHILD_ID);
  expect(root.querySelector<HTMLInputElement>('[data-testid="schedule-name"]')?.value).toBe("");
  h.unmount();
  root = await screen("Schedules");
  expect(root.querySelector<HTMLInputElement>('[data-testid="schedule-name"]')?.value).toBe("Persisted schedule");
  expect(root.querySelector("textarea")?.value).toBe("Check work");
  await click(h, root, "Create schedule");
  expect(h.daemon.commands("create_schedule")[0]).toMatchObject({ spec: { name: "Persisted schedule", target: AGENT_ID, payload: "Check work", recurrence: { kind: "cron", expression: "0 9 * * 1-5", timezone: "UTC" } } });
  expect(drafts.get(`controls/${AGENT_ID}/schedule`)).toBeUndefined();
});

test("a model choice made while a slow apply ran is kept, a finished one is forgotten", async () => {
  const root = await screen("Model & limits");
  h.daemon.script = { ...h.daemon.script, update_agent: () => "hold" };
  await pick(h, root, "Model", "Sonnet 5.5");
  await click(h, root, "Apply model");
  expect(h.daemon.commands("update_agent")).toHaveLength(1);
  await click(h, root, "max");
  // A release answers with what the script says then, so the held apply now succeeds.
  h.daemon.script = { ...h.daemon.script, update_agent: () => ({ tag: "ok", data: null }) };
  h.daemon.release();
  await h.settle();
  expect(drafts.get(`controls/${AGENT_ID}/model`)).toMatchObject({ model: "anthropic/claude-sonnet-5-5", effort: "max" });
  await click(h, root, "Apply model");
  expect(h.daemon.commands("update_agent")).toHaveLength(2);
  expect(drafts.get(`controls/${AGENT_ID}/model`)).toBeUndefined();
});

beforeEach(() => { for (const key of drafts.keys()) drafts.clear(key); });

/** The dialog's own confirm button, apart from the action button of the same name behind it. */
async function confirm(label: string) {
  const dialog = document.querySelector("dialog[open]");
  invariant(dialog !== null, "the action asks first");
  await click(h, dialog, label);
}

/** Starts a held identity save, types newer text while it runs, then lets it succeed. */
async function saveWhileTyping(root: Element, field: string, typed: string, action: string, confirmLabel: string | null) {
  h.daemon.script = { ...h.daemon.script, update_agent: () => "hold" };
  await click(h, root, action);
  if (confirmLabel !== null) await confirm(confirmLabel);
  expect(h.daemon.commands("update_agent")).toHaveLength(1);
  await type(h, root, field, typed);
  h.daemon.script = { ...h.daemon.script, update_agent: () => ({ tag: "ok", data: null }) };
  h.daemon.release();
  await h.settle();
}

test("a name typed while a slow rename ran is kept, and the instructions draft beside it", async () => {
  const root = await screen("Model & limits");
  await type(h, root, "New name", "first name");
  await type(h, root, "New instructions", "unsent instructions");
  await saveWhileTyping(root, "New name", "newer unsent name", "Rename", null);
  expect(h.daemon.commands("update_agent")[0]).toMatchObject({ patch: { name: "first name" } });
  expect(drafts.get(`controls/${AGENT_ID}/identity`)).toEqual({ name: "newer unsent name", instructions: "unsent instructions" });
  await click(h, root, "Rename");
  expect(drafts.get(`controls/${AGENT_ID}/identity`)).toEqual({ name: "", instructions: "unsent instructions" });
});

test("instructions typed while a slow replace ran are kept", async () => {
  const root = await screen("Model & limits");
  await type(h, root, "New instructions", "first instructions");
  await saveWhileTyping(root, "New instructions", "newer unsent instructions", "Replace instructions", "Replace");
  expect(h.daemon.commands("update_agent")[0]).toMatchObject({ patch: { instructions: "first instructions" } });
  expect(drafts.get(`controls/${AGENT_ID}/identity`)).toEqual({ name: "", instructions: "newer unsent instructions" });
});

test("instructions typed while a slow clear ran are kept", async () => {
  const root = await screen("Model & limits");
  await type(h, root, "New instructions", "typed before clearing");
  await saveWhileTyping(root, "New instructions", "typed while clearing", "Clear instructions", "Clear instructions");
  expect(h.daemon.commands("update_agent")[0]).toMatchObject({ patch: { instructions: "" } });
  expect(drafts.get(`controls/${AGENT_ID}/identity`)).toEqual({ name: "", instructions: "typed while clearing" });
});
