// @vitest-environment happy-dom
// The sheet on `#/learn?understand=…` over the faux Noesis daemon, with Archon reads seeded into core's
// store as the item page leaves them: what it shows, what it sends, and that it sends nothing while off.

import { actionsFor } from "@aether-ui/ui";
import type { aether, archon } from "@aether-ui/protocol";
import { afterEach, describe, expect, test, vi } from "vitest";
import { $, byText, click, CONN, connected, hashChanged, type Harness } from "../learn/fixture/harness.test-util.tsx";
import { LESSON, scenario, statusOf } from "../learn/fixture/scenario.ts";
import type { FauxNoesisOptions } from "../learn/fixture/faux-noesis.ts";
import SessionsRoute from "../learn/SessionsScreen.tsx";
import { itemBundle } from "./archon-bundles.ts";
import { understandHref, type UnderstandRequest } from "./bundle.ts";
import { ARCHON_CONN, oversized, SHIPPED, SHIPPED_SUMMARY } from "./fixture/evidence.ts";
import { actions } from "./routes.ts";

let harness: Harness | null = null;
afterEach(() => {
  harness?.unmount();
  harness = null;
});

function statusWith(enabled: boolean, engineering: boolean): aether.NoesisStatus {
  const base = statusOf(enabled);
  const settings = { ...base.settings, engineering_context: engineering };
  return { ...base, settings, effective: settings };
}

const ITEM: UnderstandRequest = { kind: "item", conn: ARCHON_CONN, id: "qi_cache" };

function seed(h: Harness, detail: archon.ItemDetail, summary: archon.ItemSummary | null): void {
  h.app.store.updateArchon(ARCHON_CONN, (s) => ({
    ...s,
    details: { ...s.details, [detail.item.id]: { value: detail as never, asOf: 1 } },
    items: summary === null ? s.items : { ...s.items, [detail.item.id]: { summary: summary as never, state: detail.item.state.state, asOf: 1 } },
  }));
}

async function open(request: UnderstandRequest, options: Partial<FauxNoesisOptions> = {}, detail: archon.ItemDetail = SHIPPED) {
  const h = await connected(scenario(true, { status: statusWith(true, true), ...options }), vi);
  harness = h;
  seed(h, detail, detail === SHIPPED ? SHIPPED_SUMMARY : null);
  location.hash = understandHref(request);
  const container = await h.mount(<SessionsRoute />);
  await h.settle();
  return { h, container };
}

const created = (h: Harness) => h.daemon.sent.filter((command) => command.cmd === "noesis_create_session");
const pills = () => [...document.querySelectorAll("dialog[open] .understand-item")].map((row) => row.querySelector(".ui-pill")?.textContent);

describe("learning from an item", () => {
  test("previews every fact with its status and the size against 32 KiB", async () => {
    await open(ITEM);
    const sheet = $(document, "dialog[open]");
    expect(sheet.textContent).toContain("Cache the dependency graph between builds");
    expect(pills()).toEqual(["Verified", "Preliminary", "Verified", "Verified", "Preliminary", "Preliminary", "Reviewed", "Verified", "Reviewed"]);
    expect($(sheet, ".understand-size-text").textContent).toMatch(/KiB of 32\.0 KiB$/);
    expect(($(sheet, "[data-testid=understand-goal]") as HTMLTextAreaElement).value).toBe("Understand Cache the dependency graph between builds");
  });

  test("sends the bundle with every status unchanged, then opens the new session", async () => {
    const { h } = await open(ITEM);
    await click(h, byText($(document, "dialog[open]"), "button", "Start learning"));
    const [command] = created(h);
    expect(command).toMatchObject({ cmd: "noesis_create_session", goal: "Understand Cache the dependency graph between builds", mode: "teach" });
    const expected = itemBundle(SHIPPED, SHIPPED_SUMMARY);
    const sent = command?.context as aether.EvidenceBundle;
    expect(sent).toEqual(expected);
    expect(sent.items.map((item) => item.status)).toEqual(expected.items.map((item) => item.status));
    expect(location.hash).toMatch(/^#\/learn\/sessions\/ns_faux_\d+$/);
  });

  test("shows a bundle over 32 KiB as a refusal and never sends it", async () => {
    const { h } = await open({ ...ITEM, id: "qi_big" }, {}, oversized());
    const sheet = $(document, "dialog[open]");
    expect($(sheet, "[data-testid=understand-refused]").textContent).toContain("over the 32.0 KiB");
    const start = byText(sheet, "button", "Start learning") as HTMLButtonElement;
    expect(start.disabled).toBe(true);
    await click(h, start);
    expect(created(h)).toEqual([]);
  });

  test("shows the daemon's engineering_context_disabled answer calmly", async () => {
    const { h } = await open(ITEM, { refuse: "unavailable: engineering_context_disabled" });
    await click(h, byText($(document, "dialog[open]"), "button", "Start learning"));
    expect($(document, "dialog[open] [data-kind=engineering]").textContent).toContain("Learning from your work was turned off");
    expect(location.hash).toContain("understand=item");
  });

  test("an observer sees the evidence but may not start a session", async () => {
    const { h } = await open(ITEM);
    h.app.store.update((s) => ({ ...s, capabilities: { ...s.capabilities, [`can.operate.${CONN}`]: false } }));
    await h.settle();
    const sheet = $(document, "dialog[open]");
    expect(sheet.textContent).toContain("This device only observes Aether");
    const start = byText(sheet, "button", "Start learning") as HTMLButtonElement;
    expect(start.disabled).toBe(true);
    await click(h, start);
    expect(created(h)).toEqual([]);
  });

  test("keeps the item on core's screen while open, so an Archon change rereads its detail", async () => {
    const h = await connected(scenario(true, { status: statusWith(true, true) }), vi);
    harness = h;
    const shown = vi.spyOn(h.app.manager, "showItems").mockImplementation(() => undefined);
    vi.spyOn(h.app.manager, "refreshItem").mockResolvedValue(undefined);
    const archond = { id: ARCHON_CONN, daemon: "archon", state: "ready", protocol: 24, features: ["changes"], label: "archond" } as const;
    h.app.store.update((s) => ({ ...s, connections: { ...s.connections, [ARCHON_CONN]: archond } }));
    seed(h, SHIPPED, SHIPPED_SUMMARY);
    location.hash = understandHref(ITEM);
    await h.mount(<SessionsRoute />);
    await h.settle();
    expect(shown).toHaveBeenLastCalledWith(ARCHON_CONN, ["qi_cache"]);
    await click(h, $(document, "dialog[open] button[aria-label='Close Learn from this']"));
    await hashChanged(h);
    expect(shown).toHaveBeenLastCalledWith(ARCHON_CONN, []);
  });

  test("closing the sheet clears the query", async () => {
    const { h } = await open(ITEM);
    await click(h, $(document, "dialog[open] button[aria-label='Close Learn from this']"));
    await hashChanged(h);
    expect(location.hash).toBe("#/learn");
    expect(document.querySelector("dialog[open]")).toBeNull();
  });
});

describe("other entities", () => {
  test("an Aether agent: its identity and newest words, all preliminary", async () => {
    await open({ kind: "agent", conn: CONN, id: LESSON.agent });
    const sheet = $(document, "dialog[open]");
    expect(sheet.textContent).toContain("Agent noesis-teacher");
    expect(new Set(pills())).toEqual(new Set(["Preliminary"]));
  });

  test("a run whose item is not loaded asks to open it first", async () => {
    await open({ kind: "run", conn: ARCHON_CONN, id: "run_elsewhere" });
    expect($(document, "dialog[open]").textContent).toContain("Open the run first");
  });

  test("a pull request id in neither accepted form is refused clearly", async () => {
    const { h } = await open({ kind: "pull_request", conn: ARCHON_CONN, id: "acme/forge#212" });
    expect($(document, "dialog[open] [role=alert]").textContent).toContain("Not a pull request");
    expect(h.daemon.names()).not.toContain("list_github_objects");
  });
});

describe("off by default", () => {
  test("with Noesis off the sheet shows the off state and sends nothing", async () => {
    const { h } = await open(ITEM, { status: statusWith(false, true) });
    expect($(document, "dialog[open] [data-testid=understand-off]").textContent).toContain("Noesis is off");
    expect(document.querySelector("dialog[open] .understand-item")).toBeNull();
    expect(created(h)).toEqual([]);
    expect(actionsFor(actions, h.app.store.get().capabilities, { kind: "item", conn: ARCHON_CONN, id: "qi_cache" })).toEqual([]);
  });

  test("with engineering context off nothing is offered, read or sent", async () => {
    const { h } = await open(ITEM, { status: statusWith(true, false) });
    expect($(document, "dialog[open] [data-testid=understand-off]").textContent).toContain("Learning from your work is off");
    expect(document.querySelector("dialog[open] button[type=submit]")).toBeNull();
    expect(created(h)).toEqual([]);
    expect(h.app.store.get().capabilities["noesis.engineering_context"]).toBe(false);
    for (const kind of ["item", "run", "agent", "pull_request"] as const) {
      expect(actionsFor(actions, h.app.store.get().capabilities, { kind, conn: ARCHON_CONN, id: "x" })).toEqual([]);
    }
  });

  test("another daemon's engineering context does not open the selected daemon's sheet", async () => {
    const h = await connected(scenario(true, { status: statusWith(true, false) }), vi);
    harness = h;
    const other = "aether:second";
    const first = h.app.store.get().connections[CONN];
    if (first === undefined) throw new Error("the faux Aether connection is ready");
    h.app.store.update((s) => ({ ...s, connections: { ...s.connections, [other]: { ...first, id: other } } }));
    h.app.store.updateAether(other, (s) => ({ ...s, noesis: statusWith(true, true) as never }));
    h.app.manager.refreshCapabilities();
    await h.settle();
    expect(h.app.store.get().capabilities["noesis.engineering_context"]).toBe(true);
    location.hash = understandHref({ kind: "agent", conn: CONN, id: LESSON.agent });
    await h.mount(<SessionsRoute />);
    await h.settle();
    expect($(document, "dialog[open] [data-testid=understand-off]").textContent).toContain("Learning from your work is off");
    expect(document.querySelector("dialog[open] button[type=submit]")).toBeNull();
    expect(created(h)).toEqual([]);
  });

  test("with engineering context on, the action is offered for each kind", async () => {
    const { h } = await open(ITEM);
    const caps = h.app.store.get().capabilities;
    expect(caps["noesis.engineering_context"]).toBe(true);
    expect(actionsFor(actions, caps, { kind: "agent", conn: CONN, id: LESSON.agent }).map((action) => action.label)).toEqual(["Learn from this"]);
  });
});
