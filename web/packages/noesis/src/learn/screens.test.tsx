// @vitest-environment happy-dom
// The sessions list, the knowledge graph and the settings sheet against the faux daemon.

import type { GraphModel, Layout } from "@aether-ui/protocol";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { SETTLE_MS } from "./data.ts";
import { $, byText, choose, click, connected, hashChanged, must, stubCanvas, type, type Harness } from "./fixture/harness.test-util.tsx";
import { EARLIER, GRAPH, LESSON, PRACTICE, scenario, SETTINGS, statusOf } from "./fixture/scenario.ts";
import type { FauxNoesisOptions } from "./fixture/faux-noesis.ts";
import { JOB_POLL_MS } from "./graph-jobs.ts";
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
    await type(harness, $(root, "#learn-goal"), "How TCP slow start works");
    await click(harness, byText(root, "button", "Start session"));
    const created = harness.daemon.sent.filter((c) => c.cmd === "noesis_create_session");
    expect(created).toMatchObject([{ goal: "How TCP slow start works", mode: "teach" }]);
    expect(location.hash).toMatch(/^#\/learn\/sessions\/ns_faux_\d+$/);
  });

  test("a refusal for want of a usable model reads calmly and links to Noesis settings", async () => {
    harness = await connected(scenario(true, { refuse: "no ready model for Noesis: set noesis.billing.allow_api to use API models" }), vi);
    location.hash = "#/learn";
    const root = await harness.mount(<SessionsRoute />);
    await type(harness, $(root, "#learn-goal"), "Anything");
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

  test("an open graph rereads when an extraction finishes, though the job event never arrives", async () => {
    const { h, root } = await graphScreen(countingLayout().layout);
    h.daemon.jobs = [{ id: "job_x", kind: "extract_session", state: "running", target: LESSON.id, created_at: LESSON.created_at, updated_at: LESSON.updated_at, usage: { input_tokens: 0, output_tokens: 0, cache_read_tokens: 0, cache_write_tokens: 0, cost_usd: 0, cost_complete: true } }];
    await h.app.manager.readNoesis("aether:faux-noesis", { kind: "jobs" });
    await h.settle(SETTLE_MS);
    const before = h.daemon.names().filter((cmd) => cmd === "noesis_graph").length;
    h.daemon.graph = { ...h.daemon.graph, nodes: [...h.daemon.graph.nodes, { ...must(GRAPH.nodes[0], "a node"), id: "p_new", name: "log-matching", statement: "Logs that share an entry share every earlier one." }] };
    h.daemon.finishJob("job_x", "succeeded", true);
    await h.settle(JOB_POLL_MS);
    // The poll's job read, then the graph read it starts, each wait out their SETTLE_MS debounce.
    await h.settle(SETTLE_MS);
    await h.settle(SETTLE_MS);
    expect(h.daemon.names().filter((cmd) => cmd === "noesis_graph").length).toBeGreaterThan(before);
    await click(h, byText(root, "button", "List"));
    expect($(root, ".learn-prop-list").textContent).toContain("Log matching");
  });

  test("a finished extraction's event rereads the graph at once", async () => {
    const { h } = await graphScreen(countingLayout().layout);
    h.daemon.jobs = [{ id: "job_y", kind: "extract_session", state: "running", target: LESSON.id, created_at: LESSON.created_at, updated_at: LESSON.updated_at, usage: { input_tokens: 0, output_tokens: 0, cache_read_tokens: 0, cache_write_tokens: 0, cost_usd: 0, cost_complete: true } }];
    const before = h.daemon.names().filter((cmd) => cmd === "noesis_graph").length;
    h.daemon.finishJob("job_y", "succeeded");
    await h.settle(SETTLE_MS);
    expect(h.daemon.names().filter((cmd) => cmd === "noesis_graph").length).toBe(before + 1);
  });

  test("the list view and the panel read titles a person can read, with the statement beneath", async () => {
    const { h, root } = await graphScreen(countingLayout().layout);
    await click(h, byText(root, "button", "List"));
    expect(root.querySelector("canvas")).toBeNull();
    const row = byText($(root, ".learn-prop-list"), "button", "One vote per term");
    expect(row.textContent).toContain("A server grants at most one vote per term.");
    await click(h, row);
    const panel = $(root, ".learn-panel");
    expect($(panel, "h2").textContent).toBe("One vote per term");
    expect(panel.textContent).toContain("A server grants at most one vote per term.");
    await click(h, byText(root, "button", "Graph"));
    expect([...root.querySelectorAll(".learn-graph-tools button")].map((button) => button.textContent)).toEqual(["Zoom in", "Zoom out", "Fit"]);
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
    await click(h, $(byText(dialog, "label", "Remove those nothing else supports"), "input"));
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
  async function openSettings(transport: "ipc" | "ws-aether", extra: Partial<FauxNoesisOptions> = {}): Promise<{ h: Harness; sheet: Element }> {
    const h = await connected(scenario(true, { transport, ...extra }), vi);
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
    await choose(h, $(sheet, "#learn-model-strong"), "anthropic/claude-haiku-5-5");
    await click(h, byText(sheet, "button", "Save"));
    expect(h.daemon.sent.filter((c) => c.cmd === "noesis_configure").map((c) => c.patch)).toEqual([{ models: { strong: "anthropic/claude-haiku-5-5" } }]);
    expect(h.daemon.privileged).toEqual([]);
  });

  test("the model choosers offer the daemon's models by billing, and only ready ones it may use", async () => {
    const status = statusOf(true);
    const { sheet } = await openSettings("ipc", { status: { ...status, settings: { ...status.settings, billing: { allow_api: false } } } });
    const strong = $(sheet, "#learn-model-strong") as HTMLSelectElement;
    expect([...strong.querySelectorAll("optgroup")].map((group) => group.label)).toEqual(["Subscription — covered by your plan", "API — billed per request"]);
    const option = (value: string) => must([...strong.options].find((candidate) => candidate.value === value), value);
    expect(option("anthropic/claude-haiku-5-5").disabled).toBe(false);
    expect(option("openai/gpt-faux").disabled).toBe(true);
    expect(option("openai/gpt-faux").textContent).toContain("not ready");
    expect(option("anthropic-api/claude-sonnet-5-5").disabled).toBe(true);
    expect(sheet.textContent).toContain("Teaching runs on anthropic/claude-opus-5-5.");
  });

  test("a stored model the daemon does not offer shows as unknown, asks for a pick, and other changes still save", async () => {
    const status = statusOf(true);
    const settings = { ...status.settings, models: { ...SETTINGS.models, strong: "retired/model-x" } };
    const { h, sheet } = await openSettings("ipc", { status: { ...status, settings, effective: settings, teaching_model: { refusal: "no ready model for Noesis" } } });
    const strong = $(sheet, "#learn-model-strong") as HTMLSelectElement;
    expect(strong.value).toBe("retired/model-x");
    expect(strong.selectedOptions[0]?.textContent).toContain("unknown here");
    expect(strong.getAttribute("aria-invalid")).toBe("true");
    expect(sheet.textContent).toContain("Pick one from the list.");
    await click(h, $(byText(sheet, "label", "Learn from my work"), "input"));
    await click(h, byText(sheet, "button", "Save"));
    expect(h.daemon.sent.filter((c) => c.cmd === "noesis_configure").map((c) => c.patch)).toEqual([{ engineering_context: true }]);
  });

  test("billing today says what the subscription covered and what the API key was charged", async () => {
    const { sheet } = await openSettings("ipc");
    expect($(sheet, "[aria-label='Billing today']").textContent).toBe("$0.20 billed to your API key; the rest ran on your subscription — no per-request charge (≈$2.05 at API list price).");
  });

  test("a remote device sees host-only settings disabled, with the reason", async () => {
    const { sheet } = await openSettings("ws-aether");
    const enabled = $(byText(sheet, "label", "Turn on Noesis"), "input") as HTMLInputElement;
    expect(enabled.disabled).toBe(true);
    expect(sheet.textContent).toContain("Only the daemon's own host can change this");
  });
});
