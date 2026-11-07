// @vitest-environment happy-dom
import { drafts } from "@aether-ui/core";
import { beforeEach, afterEach, expect, test } from "vitest";
import { ControlsScreen } from "./ControlsScreen.tsx";
import { AGENT_ID, CHILD_ID, controlsScript, demoAgents } from "./fixture/demo.ts";
import { connected, click, type, pick, type Harness } from "./fixture/harness.test-util.tsx";

let h: Harness;
afterEach(() => h?.unmount());
async function screen(tab: string, id = AGENT_ID) {
  h = await connected({ agents: demoAgents(), script: controlsScript() });
  const root = await h.mount(<ControlsScreen conn={h.conn} id={id} />);
  await click(h, root, tab);
  return root;
}

test("the message draft survives unmount, is per agent, and clears after sending", async () => {
  let root = await screen("Steer");
  await type(h, root, "Message", "Remember this message");
  await click(h, root, "Also send to the agents it delegated to");
  h.unmount();
  root = await screen("Steer", CHILD_ID);
  expect(root.querySelector("textarea")?.value).toBe("");
  h.unmount();
  root = await screen("Steer");
  expect(root.querySelector("textarea")?.value).toBe("Remember this message");
  expect(root.querySelector('[role="switch"]')?.getAttribute("aria-checked")).toBe("true");
  await click(h, root, "Send");
  expect(h.daemon.commands("send_message")[0]).toMatchObject({ to: AGENT_ID, content: "Remember this message", subtree: true });
  expect(drafts.get(`controls/${AGENT_ID}/message`)).toBeUndefined();
  h.unmount();
  root = await screen("Steer");
  expect(root.querySelector("textarea")?.value).toBe("");
});

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

test("a refused message retains the draft for recovery", async () => {
  const root = await screen("Steer");
  h.daemon.script = { ...controlsScript(), send_message: () => ({ code: "invalid_argument", message: "Agent unavailable" }) };
  await type(h, root, "Message", "Keep on failure");
  await click(h, root, "Send");
  expect(root.textContent).toContain("Agent unavailable");
  expect(drafts.get(`controls/${AGENT_ID}/message`)).toMatchObject({ text: "Keep on failure" });
});

beforeEach(() => { for (const key of drafts.keys()) drafts.clear(key); });
