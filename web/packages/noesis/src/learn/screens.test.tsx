// @vitest-environment happy-dom
// The sessions list, the knowledge graph and the settings sheet against the faux daemon.

import type { GraphModel, Layout } from "@aether-ui/protocol";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { SETTLE_MS } from "./data.ts";
import { $, byText, click, connected, hashChanged, must, stubCanvas, type, type Harness } from "./fixture/harness.test-util.tsx";
import { EARLIER, GRAPH, LESSON, PRACTICE, scenario } from "./fixture/scenario.ts";
import { GraphScreen } from "./GraphScreen.tsx";
import { LearnFrame } from "./LearnFrame.tsx";
import SessionsRoute from "./SessionsScreen.tsx";

let harness: Harness | null = null;
beforeEach(stubCanvas);
afterEach(() => {
  harness?.unmount();
  harness = null;
});

describe("sessions", () => {
  test("lists every session, newest first, each with its next step", async () => {
    harness = await connected(scenario(true), vi);
    const root = await harness.mount(<SessionsRoute />);
    await harness.settle(SETTLE_MS);
    const goals = [...root.querySelectorAll(".learn-card-goal")].map((goal) => goal.textContent);
    expect(goals).toEqual([PRACTICE.goal, LESSON.goal, EARLIER.goal]);
    expect(root.querySelectorAll(".learn-card-next")).toHaveLength(3);
  });

  test("starting a session creates it and opens it", async () => {
    harness = await connected(scenario(true), vi);
    const root = await harness.mount(<SessionsRoute />);
    await type(harness, $(root, "[data-testid=learn-goal]"), "How TCP slow start works");
    await click(harness, byText(root, "button", "Start session"));
    const created = harness.daemon.sent.filter((c) => c.cmd === "noesis_create_session");
    expect(created).toMatchObject([{ goal: "How TCP slow start works", mode: "teach" }]);
    expect(location.hash).toMatch(/^#\/learn\/sessions\/ns_faux_\d+$/);
  });

  test("a refusal for want of a usable model reads calmly and links to Noesis settings", async () => {
    harness = await connected(scenario(true, { refuse: "no ready model for Noesis: set noesis.billing.allow_api to use API models" }), vi);
    location.hash = "#/learn";
    const root = await harness.mount(<SessionsRoute />);
    await type(harness, $(root, "[data-testid=learn-goal]"), "Anything");
    await click(harness, byText(root, "button", "Start session"));
    const note = root.querySelector(".learn-callout");
    expect(note?.textContent).toContain("Noesis needs a model it may use");
    expect(note?.querySelector("a")?.getAttribute("href")).toBe("#/learn?panel=settings");
    expect(location.hash).toBe("#/learn");
  });
});

describe("knowledge graph", () => {
  function countingLayout() {
    const calls: GraphModel[] = [];
    const layout = async (model: GraphModel): Promise<Layout> => {
      calls.push(model);
      return { hash: String(calls.length), positions: new Map(model.nodes.map((node, i) => [node.id, { x: i * 120, y: 0 }])) };
    };
    return { calls, layout };
  }

  async function graphScreen(layout: (model: GraphModel) => Promise<Layout>): Promise<{ h: Harness; root: HTMLElement }> {
    const h = await connected(scenario(true, { transport: "ipc" }), vi);
    harness = h;
    const root = await h.mount(<LearnFrame view="graph">{(gate) => <GraphScreen {...gate} layout={layout} />}</LearnFrame>);
    await h.settle(SETTLE_MS);
    return { h, root };
  }

  test("lays the graph out once, settles, and keeps the layout when only mastery changes", async () => {
    const counting = countingLayout();
    const { h, root } = await graphScreen(counting.layout);
    expect(counting.calls).toHaveLength(1);
    expect(must(counting.calls[0], "the first layout").nodes).toHaveLength(GRAPH.nodes.length);
    expect(root.querySelector(".learn-graph-canvas[data-settled]")).not.toBeNull();
    expect(root.querySelector("canvas")?.getAttribute("aria-label")).toBe("Knowledge graph");
    h.daemon.concepts = h.daemon.concepts.map((c) => (c.concept.ref === "p_vote" ? { ...c, state: "demonstrated" } : c));
    await h.app.manager.readNoesis("aether:faux-noesis", { kind: "learner" });
    await h.settle(SETTLE_MS);
    expect(counting.calls).toHaveLength(1);
  });

  test("searching asks the daemon with the query", async () => {
    const { h, root } = await graphScreen(countingLayout().layout);
    await type(h, $(root, "input[type=search]"), "vote");
    await click(h, byText(root, "button", "Search"));
    await h.settle(SETTLE_MS);
    expect(h.daemon.sent.filter((c) => c.cmd === "noesis_graph").map((c) => c.query)).toEqual([undefined, "vote"]);
  });

  test("deleting a source asks first, sends only on confirm, and carries the chosen graph option", async () => {
    const { h, root } = await graphScreen(countingLayout().layout);
    await click(h, byText(root, ".learn-graph-canvas button", "Up-to-date log rule"));
    const panel = $(root, "aside[aria-label=Proposition]");
    expect(panel.textContent).toContain("A voter refuses candidates");
    const deletes = () => h.daemon.sent.filter((c) => c.cmd === "noesis_delete_source" || c.cmd === "noesis_delete_proposition");

    await click(h, byText(panel, "button", "Delete source…"));
    expect($(document, "dialog[open]").textContent).toContain("Delete this source?");
    expect(deletes()).toEqual([]);
    await click(h, byText($(document, "dialog[open]"), "button", "Keep it"));
    expect(document.querySelector("dialog[open]")).toBeNull();
    expect(deletes()).toEqual([]);

    await click(h, byText(panel, "button", "Delete source…"));
    const dialog = $(document, "dialog[open]");
    await click(h, byText(dialog, "[role=radio]", "Remove those nothing else supports"));
    expect(deletes()).toEqual([]);
    await click(h, byText(dialog, "button", "Delete"));
    expect(deletes()).toMatchObject([{ cmd: "noesis_delete_source", source: "s_notes", graph: "delete_unsupported" }]);
    expect(document.querySelector("dialog[open]")).toBeNull();
  });

  test("deleting a statement asks first, sends only on confirm, and closes its panel", async () => {
    const { h, root } = await graphScreen(countingLayout().layout);
    await click(h, byText(root, ".learn-graph-canvas button", "One vote per term"));
    const panel = $(root, "aside[aria-label=Proposition]");
    const deletes = () => h.daemon.sent.filter((c) => c.cmd === "noesis_delete_source" || c.cmd === "noesis_delete_proposition");

    await click(h, byText(panel, "button", "Delete statement…"));
    expect($(document, "dialog[open]").textContent).toContain("Delete this statement?");
    await click(h, byText($(document, "dialog[open]"), "button", "Keep it"));
    expect(deletes()).toEqual([]);
    expect(root.querySelector("aside[aria-label=Proposition]")).not.toBeNull();

    await click(h, byText(panel, "button", "Delete statement…"));
    await click(h, byText($(document, "dialog[open]"), "button", "Delete"));
    expect(deletes()).toMatchObject([{ cmd: "noesis_delete_proposition", proposition: "p_vote" }]);
    expect(root.querySelector("aside[aria-label=Proposition]")).toBeNull();
  });

  test("a failed layout shows an error with a retry", async () => {
    let calls = 0;
    const { h, root } = await graphScreen(async (model) => {
      calls += 1;
      if (calls === 1) throw new Error("the layout worker stopped");
      return { hash: "ok", positions: new Map(model.nodes.map((node) => [node.id, { x: 0, y: 0 }])) };
    });
    expect(root.querySelector("[role=alert]")?.textContent).toContain("the layout worker stopped");
    await click(h, byText(root, "button", "Try again"));
    expect(root.querySelector(".learn-graph-canvas[data-settled]")).not.toBeNull();
  });
});

describe("settings", () => {
  async function openSettings(transport: "ipc" | "ws-aether"): Promise<{ h: Harness; sheet: Element }> {
    const h = await connected(scenario(true, { transport }), vi);
    harness = h;
    location.hash = "#/learn?panel=settings";
    await h.mount(<SessionsRoute />);
    await hashChanged(h);
    await h.settle(SETTLE_MS);
    const sheet = $(document, "dialog[open]");
    expect(sheet.textContent).toContain("Turn on Noesis");
    return { h, sheet };
  }

  test("shows today's spend against each budget", async () => {
    const { sheet } = await openSettings("ipc");
    const spend = $(sheet, ".learn-spend");
    expect(spend.textContent).toContain("$1.85 of $5.00");
    expect(spend.textContent).toContain("$0.40 of $2.00");
    expect(spend.querySelectorAll("meter").length).toBeGreaterThanOrEqual(2);
  });

  test("saves only what changed, host-only keys through the desktop's privileged action", async () => {
    const { h, sheet } = await openSettings("ipc");
    const budget = $(byText(sheet, ".ui-field", "Teaching, in dollars"), "input");
    await type(h, budget, "8");
    await click(h, byText(sheet, "button", "Save"));
    expect(h.daemon.privileged).toEqual([{ kind: "noesis.configure", conn: "local:faux-main", patch: { "noesis.budget.interactive_usd_daily": 8 } }]);
    expect(h.daemon.sent.filter((c) => c.cmd === "noesis_configure"), "the bridge refuses a host-only key as a plain change").toEqual([]);
    expect(h.daemon.status.settings.budget.interactive_usd_daily).toBe(8);
  });

  test("a change no host key is in goes as a plain change", async () => {
    const { h, sheet } = await openSettings("ipc");
    await click(h, $(sheet, "[data-testid=noesis-model-strong]"));
    await click(h, byText(sheet, "[role=option]", "claude-sonnet-5-5"));
    await click(h, byText(sheet, "button", "Save"));
    expect(h.daemon.sent.filter((c) => c.cmd === "noesis_configure").map((c) => c.patch)).toEqual([{ models: { strong: "claude-code/claude-sonnet-5-5" } }]);
    expect(h.daemon.privileged).toEqual([]);
  });

  test("a remote device sees host-only settings disabled, with the reason", async () => {
    const { sheet } = await openSettings("ws-aether");
    const enabled = byText(sheet, "[role=switch]", "Turn on Noesis") as HTMLButtonElement;
    expect(enabled.disabled).toBe(true);
    expect(sheet.textContent).toContain("Only the daemon's own host can change this");
  });
});
