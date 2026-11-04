// @vitest-environment happy-dom
// The inbox answers questions and approvals from core's attention, and the recovery review never
// resends a lost change on its own: it checks with resolve_submission, then the operator chooses.

import { act } from "react";
import { afterEach, describe, expect, test } from "vitest";
import { AGENT_ID, CHILD_ID, controlsScript, demoAgents } from "../controls/fixture/demo.ts";
import { stamp, type Answer, type ScriptOptions } from "../controls/fixture/daemon.ts";
import { buttonNamed, click, connected, type, type Harness } from "../controls/fixture/harness.test-util.tsx";
import { describeCommand } from "./describe.ts";
import { InboxScreen } from "./InboxScreen.tsx";

let current: Harness | null = null;
afterEach(() => { current?.unmount(); current = null; });

const ATTENTION = {
  questions: [{ agent: CHILD_ID, agent_name: "changelog-writer", question: "Should the 1.3 notes mention the fleet redesign?", question_seq: 7, asked_at: stamp(20) }],
  approvals: [{ id: "apr_1", agent_id: AGENT_ID, call_id: "call_1", tool: "http", description: "POST the release to the website", argument: "https://example.test/release", side_effect: "external", status: { state: "pending" }, requested_at: stamp(21) }],
  agents: [{ agent: CHILD_ID, agent_name: "changelog-writer", kind: "agent_failed", reason: "The model refused the tool call", since: stamp(22) }],
} as unknown as NonNullable<ScriptOptions["attention"]>;

async function open(options: ScriptOptions = {}) {
  const h = current = await connected({ agents: demoAgents(), attention: ATTENTION, ...options, script: { ...controlsScript(), ...options.script } });
  const root = await h.mount(<InboxScreen />);
  return { h, root };
}

function openDialog(): HTMLElement {
  const dialog = document.querySelector<HTMLElement>("dialog[open]");
  expect(dialog).not.toBeNull();
  return dialog ?? document.body;
}

/** Sends a cancel whose reply is a timeout, as a lost reply leaves it: unconfirmed in core's outbox. */
async function loseCancel(h: Harness): Promise<string> {
  await act(async () => { await h.app.send(h.conn, { cmd: "cancel", agent: AGENT_ID }).catch(() => undefined); });
  await h.settle();
  const outbox = h.app.store.get().aether[h.conn]?.outbox ?? [];
  expect(outbox).toEqual([expect.objectContaining({ state: "unknown" })]);
  return outbox[0]?.key ?? "";
}

const timedOut = (): Answer => ({ code: "timeout", message: "no reply within 30s" });

describe("InboxScreen", () => {
  test("answers a question and approves or, after asking, denies a tool call", async () => {
    const { h, root } = await open();
    expect(root.textContent).toContain("Should the 1.3 notes mention the fleet redesign?");
    expect(buttonNamed(root, "Send answer").disabled).toBe(true);
    await type(h, root, "Answer changelog-writer", "Yes, one line.");
    await click(h, root, "Send answer");
    expect(h.daemon.commands("reply_to_question")).toEqual([expect.objectContaining({ to: CHILD_ID, question_seq: 7, content: "Yes, one line." })]);
    await click(h, root, "Approve");
    expect(h.daemon.commands("resolve_approval")).toEqual([expect.objectContaining({ id: "apr_1", decision: "approved" })]);
    await click(h, root, "Deny");
    await type(h, openDialog(), "Reason (optional)", "Not before the tag");
    await click(h, openDialog(), "Deny");
    expect(h.daemon.commands("resolve_approval")[1]).toMatchObject({ id: "apr_1", decision: "denied", note: "Not before the tag" });
  });

  test("a stopped agent links to its controls and its conversation", async () => {
    const { root } = await open();
    const links = [...root.querySelectorAll("a")].map((a) => a.getAttribute("href"));
    expect(links).toContain(`#/controls/${encodeURIComponent("aether:b3")}/${CHILD_ID}`);
    expect(root.textContent).toContain("The model refused the tool call");
  });

  test("an observer reads the inbox with every answer disabled and the reason shown", async () => {
    const { root } = await open({ role: "observer" });
    for (const label of ["Send answer", "Approve", "Deny"]) expect(buttonNamed(root, label).disabled).toBe(true);
    expect(root.querySelector(".b3-standing")?.textContent).toContain("Observer devices cannot change daemon state");
    expect([...root.querySelectorAll(".b3-reason")].map((r) => r.textContent)).toContain("Observer devices cannot change daemon state");
  });

  test("a clear daemon says nothing needs you", async () => {
    const { root } = await open({ attention: {} });
    expect(root.textContent).toContain("Nothing on studio needs you.");
    expect(root.textContent).not.toContain("Recovery review");
  });
});

describe("recovery review", () => {
  test("a lost change waits for a check; absent_safe offers a confirmed resend with the same key", async () => {
    let lost = true;
    const { h, root } = await open({ script: { cancel: () => (lost ? timedOut() : { tag: "ok", data: null }), resolve_submission: () => ({ tag: "submission_resolved", data: { outcome: "absent_safe", horizon: stamp(1), retained_since: stamp(0) } }) } });
    const key = await loseCancel(h);
    expect(root.querySelector('[aria-label="Recovery review"]')?.textContent).toContain("Cancel · release-captain");
    expect(() => buttonNamed(root, "Send again")).toThrow();
    await click(h, root, "Check what happened");
    expect(h.daemon.commands("resolve_submission")[0]).toMatchObject({ command: { cmd: "cancel", idempotency_key: key }, horizon: "hz_b3_1" });
    expect(root.textContent).toContain("It never reached the daemon");
    expect(h.daemon.commands("cancel")).toHaveLength(1);
    lost = false;
    await click(h, root, "Send again");
    expect(h.daemon.commands("cancel")).toHaveLength(1);
    await click(h, openDialog(), "Send again");
    expect(h.daemon.commands("cancel").map((c) => c.idempotency_key)).toEqual([key, key]);
    expect(h.app.store.get().aether[h.conn]?.outbox).toEqual([]);
  });

  test("an executed change is cleared without being sent again", async () => {
    const { h, root } = await open({ script: { cancel: timedOut, resolve_submission: () => ({ tag: "submission_resolved", data: { outcome: "executed", horizon: stamp(1), reply: { reply: "ok", data: null } } }) } });
    await loseCancel(h);
    await click(h, root, "Check what happened");
    await h.settle();
    expect(h.app.store.get().aether[h.conn]?.outbox).toEqual([]);
    expect(h.daemon.commands("cancel")).toHaveLength(1);
    expect(root.textContent).not.toContain("Recovery review");
  });

  test("an unknown outcome warns before a resend, and discarding asks first and sends nothing", async () => {
    const { h, root } = await open({ script: { cancel: timedOut, resolve_submission: () => ({ tag: "submission_resolved", data: { outcome: "unknown", horizon: stamp(1), retained_since: stamp(0), reason: "pruned" } }) } });
    await loseCancel(h);
    await click(h, root, "Check what happened");
    expect(root.textContent).toContain("cannot tell whether it happened");
    expect(buttonNamed(root, "Send again").getAttribute("data-variant") ?? buttonNamed(root, "Send again").className).toMatch(/danger/);
    await click(h, root, "Send again");
    expect(openDialog().textContent).toContain("sending it again does it a second time");
    expect(openDialog().textContent).not.toContain("cannot run twice");
    await click(h, openDialog(), "Keep it");
    await click(h, root, "Discard");
    await click(h, openDialog(), "Keep it");
    expect(h.app.store.get().aether[h.conn]?.outbox).toHaveLength(1);
    await click(h, root, "Discard");
    await click(h, openDialog(), "Discard");
    expect(h.app.store.get().aether[h.conn]?.outbox).toEqual([]);
    expect(h.daemon.commands("cancel")).toHaveLength(1);
  });

  test("a change from a replaced daemon can only be discarded, never checked or resent", async () => {
    const { h, root } = await open();
    await act(async () => h.app.store.update((s) => ({ ...s, review: [{ conn: "aether:old", key: "k-old", command: { cmd: "pause", agent: AGENT_ID, idempotency_key: "k-old" }, state: "unknown" }] })));
    await h.settle();
    const review = root.querySelector('[aria-label="Recovery review"]');
    expect(review?.textContent).toContain("The daemon was replaced before it answered");
    expect(() => buttonNamed(review ?? root, "Check what happened")).toThrow();
    await click(h, review ?? root, "Discard");
    await click(h, openDialog(), "Discard");
    expect(h.app.store.get().review).toEqual([]);
    expect(h.daemon.commands("pause")).toHaveLength(0);
  });

  test("a lost unkeyed change is reviewed without a check or resend, and discarding sends nothing", async () => {
    const { h, root } = await open();
    await act(async () => { h.app.noteUnconfirmed(h.conn, { cmd: "wake", agent: AGENT_ID }, "disconnected"); });
    await h.settle();
    const review = root.querySelector('[aria-label="Recovery review"]');
    expect(review?.textContent).toContain("sent without a key");
    expect(() => buttonNamed(review ?? root, "Check what happened")).toThrow();
    expect(() => buttonNamed(review ?? root, "Send again")).toThrow();
    await click(h, review ?? root, "Discard");
    await click(h, openDialog(), "Discard");
    expect(h.app.store.get().aether[h.conn]?.outbox).toEqual([]);
    expect(h.daemon.sent.filter((c) => c.cmd === "wake" || c.cmd === "resolve_submission")).toEqual([]);
  });

  test("an observer sees a lost change but cannot check or resend it", async () => {
    const { h, root } = await open({ role: "observer" });
    await act(async () => h.app.store.updateAether(h.conn, (s) => ({ ...s, outbox: [{ conn: h.conn, key: "k1", command: { cmd: "cancel", agent: AGENT_ID, idempotency_key: "k1" }, state: "unknown" }] })));
    await h.settle();
    expect(buttonNamed(root, "Check what happened").disabled).toBe(true);
  });
});

describe("describeCommand", () => {
  test("names the verb and the agent, and shortens long text", () => {
    expect(describeCommand({ cmd: "cancel", agent: "agt_1", reason: "done" }, { agt_1: "captain" })).toEqual({ title: "Cancel · captain", detail: "done" });
    expect(describeCommand({ cmd: "archive", target: { agent: "agt_2" } }, {}).title).toBe("Archive · agt_2");
    expect(describeCommand({ cmd: "send_message", to: "agt_1", content: "x".repeat(200) }, {}).detail).toHaveLength(140);
    expect(() => describeCommand({}, {})).toThrow();
  });
});
