// @vitest-environment happy-dom
// "Catch me up" and resuming, over the faux Noesis daemon: Archon's repositories, items and cached PRs and
// Aether's memories are seeded into core's store as its rereads leave them, and no read goes to a daemon.

import type { ReadEntry } from "@aether-ui/core";
import type { aether } from "@aether-ui/protocol";
import { afterEach, describe, expect, test, vi } from "vitest";
import { $, byText, click, CONN, connected, type Harness } from "../learn/fixture/harness.test-util.tsx";
import { scenario, statusOf } from "../learn/fixture/scenario.ts";
import SessionsRoute from "../learn/SessionsScreen.tsx";
import { understandHref, type UnderstandRequest } from "./bundle.ts";
import { catchUpBundle } from "./catchup-bundle.ts";
import { sinceFor } from "./catchup-range.ts";
import { ARCHON_CONN, PR_OBJECTS } from "./fixture/evidence.ts";
import { MEMORIES, MEMORY_SCOPE, MERGED, OTHER_REPO, REPO, SINCE, SUMMARIES } from "./fixture/catchup.ts";
import { sourceKey, SESSIONS_KEY, VISITS_KEY } from "./resume.ts";

let harness: Harness | null = null;
afterEach(() => {
  harness?.unmount();
  harness = null;
});

const entry = (value: unknown): ReadEntry => ({ value, asOf: null, issued: 1, requested: 1, settled: 1, error: null, errorIssued: 0 });

function seedRead(h: Harness, conn: string, key: string, value: unknown): void {
  h.app.store.update((s) => ({ ...s, reads: { ...s.reads, [conn]: { ...s.reads[conn], [key]: entry(value) } } }));
}

function seedArchon(h: Harness): void {
  const archond = { id: ARCHON_CONN, daemon: "archon", state: "ready", protocol: 24, features: ["changes", "github_import"], label: "archond" } as const;
  h.app.store.update((s) => ({ ...s, connections: { ...s.connections, [ARCHON_CONN]: archond } }));
  const items = Object.fromEntries(SUMMARIES.map((summary) => [summary.id, { summary: summary as never, state: summary.state.state, asOf: 1 }]));
  h.app.store.updateArchon(ARCHON_CONN, (s) => ({ ...s, items }));
  seedRead(h, ARCHON_CONN, "noesis.catchup.repos", [REPO, OTHER_REPO]);
  seedRead(h, ARCHON_CONN, "noesis.catchup.prs:github.com/acme/forge", { objects: MERGED });
  seedRead(h, CONN, `noesis.catchup.memories:${MEMORY_SCOPE}`, MEMORIES);
  seedRead(h, ARCHON_CONN, "noesis.understand.prs:acme/forge", { objects: PR_OBJECTS });
}

async function open(hash: string, sessions: readonly aether.NoesisSession[] = []) {
  const status = statusOf(true);
  const settings = { ...status.settings, engineering_context: true };
  const h = await connected(scenario(true, { status: { ...status, settings, effective: settings }, sessions }), vi);
  harness = h;
  vi.spyOn(h.app, "reread").mockResolvedValue(undefined);
  seedArchon(h);
  location.hash = hash;
  const container = await h.mount(<SessionsRoute />);
  await h.settle();
  return { h, container };
}

const REPOSITORY: UnderstandRequest = { kind: "repository", conn: ARCHON_CONN, id: REPO.id, since: SINCE };
const PR: UnderstandRequest = { kind: "pull_request", conn: ARCHON_CONN, id: "github.com/acme/forge#212" };
const created = (h: Harness) => h.daemon.sent.filter((command) => command.cmd === "noesis_create_session");
const pills = () => [...document.querySelectorAll("dialog[open] .understand-item")].map((row) => row.querySelector(".ui-pill")?.textContent);
const stored = async (h: Harness, key: string) => h.app.host.storage.get(key) as Promise<Record<string, string> | undefined>;

describe("choosing a repository to catch up on", () => {
  test("lists the registered repositories and three ranges, and opens the evidence for the range chosen", async () => {
    const { h } = await open("#/learn");
    await click(h, byText(document, "button", "Catch me up"));
    const sheet = $(document, "dialog[open]");
    const [repo, range] = [...sheet.querySelectorAll("select")];
    expect([...repo?.options ?? []].map((option) => option.textContent)).toEqual(["forge", "widget"]);
    expect([...range?.options ?? []].map((option) => option.textContent)).toEqual(["Since my last visit", "Last 7 days", "Last 30 days"]);
    expect(sheet.querySelector("input[type=text], textarea")).toBeNull();
    const before = sinceFor("week", new Date(), null);
    await click(h, byText(sheet, "button", "Catch me up"));
    const after = sinceFor("week", new Date(), null);
    const request = new URLSearchParams(location.hash.split("?")[1]);
    expect(request.get("understand")).toBe("repository");
    expect([request.get("id"), request.get("conn")]).toEqual([REPO.id, ARCHON_CONN]);
    expect([before, after]).toContain(request.get("since"));
  });

  test("with no repository registered it says so instead of offering an empty list", async () => {
    const { h } = await open("#/learn");
    seedRead(h, ARCHON_CONN, "noesis.catchup.repos", []);
    await click(h, byText(document, "button", "Catch me up"));
    expect($(document, "dialog[open]").textContent).toContain("No repositories yet");
    expect(document.querySelector("dialog[open] select")).toBeNull();
  });

  test("is not offered while engineering context is off", async () => {
    const { h } = await open("#/learn");
    h.app.store.update((s) => ({ ...s, capabilities: { ...s.capabilities, "noesis.engineering_context": false } }));
    await h.settle();
    expect(document.querySelector("[aria-label='Catch me up on a repository']")).toBeNull();
  });
});

describe("the catch-up evidence", () => {
  test("previews what changed with honest statuses, memory last, and starts a session with that bundle", async () => {
    const { h } = await open(understandHref(REPOSITORY));
    const sheet = $(document, "dialog[open]");
    expect(pills()).toEqual(["Verified", "Verified", "Preliminary", "Verified", "Reviewed", "Preliminary", "Preliminary"]);
    expect(($(sheet, "[data-testid=understand-goal]") as HTMLTextAreaElement).value).toBe("Catch up on forge since 2026-11-01");
    await click(h, byText(sheet, "button", "Start learning"));
    const expected = catchUpBundle({ repoName: "forge", since: SINCE, pulls: MERGED, items: SUMMARIES.filter((item) => item.repo === REPO.id), memories: MEMORIES, unread: [] });
    expect(created(h)).toHaveLength(1);
    expect(created(h)[0]).toMatchObject({ cmd: "noesis_create_session", goal: "Catch up on forge since 2026-11-01", mode: "teach", context: expected?.bundle });
    expect(location.hash).toMatch(/^#\/learn\/sessions\/ns_faux_\d+$/);
  });

  test("remembers the session and the catch-up time, so the next range can start from this visit", async () => {
    const { h } = await open(understandHref(REPOSITORY));
    await click(h, byText($(document, "dialog[open]"), "button", "Start learning"));
    await h.settle();
    expect(Object.keys((await stored(h, SESSIONS_KEY)) ?? {})).toEqual([sourceKey(REPOSITORY)]);
    const visits = await stored(h, VISITS_KEY);
    expect(Object.keys(visits ?? {})).toEqual([`${ARCHON_CONN}|${REPO.id}`]);
    expect(Date.now() - Date.parse(Object.values(visits ?? {})[0] ?? "")).toBeLessThan(60_000);
  });

  test("says when nothing changed in the range", async () => {
    await open(understandHref({ ...REPOSITORY, since: "2026-12-01" }));
    expect($(document, "dialog[open]").textContent).toContain("Nothing new");
  });

  test("a repository Archon does not list is reported, not guessed", async () => {
    await open(understandHref({ ...REPOSITORY, id: "repo_gone" }));
    expect($(document, "dialog[open]").textContent).toContain("No such repository");
  });
});

describe("learning again from the same evidence", () => {
  const earlier = (id: string, goal: string): aether.NoesisSession =>
    ({ id, goal, agent: "agt_old", mode: "teach", phase: "probe", created_at: "2026-11-02T09:00:00Z", updated_at: "2026-11-03T09:00:00Z" }) as aether.NoesisSession;

  test("a first look offers only Start learning", async () => {
    await open(understandHref(PR));
    const sheet = $(document, "dialog[open]");
    expect(byText(sheet, "button", "Start learning")).toBeDefined();
    expect(sheet.textContent).not.toContain("Continue where you left off");
  });

  test("the session this source started is the primary action, and Start fresh is secondary", async () => {
    const h0 = await open("#/learn", [earlier("ns_pr", "A goal I rewrote")]);
    await h0.h.app.host.storage.set(SESSIONS_KEY, { [sourceKey(PR)]: "ns_pr" });
    location.hash = understandHref(PR);
    dispatchEvent(new HashChangeEvent("hashchange"));
    await h0.h.settle();
    const sheet = $(document, "dialog[open]");
    expect(byText(sheet, "button", "Continue where you left off").dataset.variant).toBe("primary");
    expect(byText(sheet, "button", "Start fresh").dataset.variant).toBe("secondary");
    expect($(sheet, "[data-testid=understand-resume]").textContent).toContain("A goal I rewrote");
    await click(h0.h, byText(sheet, "button", "Continue where you left off"));
    expect(location.hash).toBe("#/learn/sessions/ns_pr");
    expect(created(h0.h)).toEqual([]);
  });

  test("a session with the source's own goal is found when nothing was remembered", async () => {
    await open(understandHref(PR), [earlier("ns_goal", "Understand #212 Cache the dependency graph between builds")]);
    expect(byText($(document, "dialog[open]"), "button", "Continue where you left off")).toBeDefined();
  });

  test("a remembered session past the first page of the list is still found and resumed", async () => {
    const filler = Array.from({ length: 200 }, (_, index) => earlier(`ns_old_${index}`, `An older goal ${index}`));
    const h0 = await open("#/learn", [...filler, earlier("ns_late", "A goal on a later page")]);
    expect(h0.h.app.store.get().aether[CONN]?.learn.sessions.ns_late, "the list read stops at its page").toBeUndefined();
    await h0.h.app.host.storage.set(SESSIONS_KEY, { [sourceKey(PR)]: "ns_late" });
    location.hash = understandHref(PR);
    dispatchEvent(new HashChangeEvent("hashchange"));
    await h0.h.settle();
    const sheet = $(document, "dialog[open]");
    expect($(sheet, "[data-testid=understand-resume]").textContent).toContain("A goal on a later page");
    await click(h0.h, byText(sheet, "button", "Continue where you left off"));
    expect(location.hash).toBe("#/learn/sessions/ns_late");
  });

  test("a remembered session that no longer exists is not offered", async () => {
    const { h } = await open("#/learn");
    await h.app.host.storage.set(SESSIONS_KEY, { [sourceKey(PR)]: "ns_deleted" });
    location.hash = understandHref(PR);
    dispatchEvent(new HashChangeEvent("hashchange"));
    await h.settle();
    expect($(document, "dialog[open]").textContent).not.toContain("Continue where you left off");
  });

  test("Start fresh makes a new session and remembers it in place of the old one", async () => {
    const { h } = await open(understandHref(PR), [earlier("ns_goal", "Understand #212 Cache the dependency graph between builds")]);
    await click(h, byText($(document, "dialog[open]"), "button", "Start fresh"));
    expect(created(h)).toHaveLength(1);
    const fresh = location.hash.split("/").at(-1);
    expect(fresh).toMatch(/^ns_faux_\d+$/);
    expect((await stored(h, SESSIONS_KEY))?.[sourceKey(PR)]).toBe(fresh);
  });

  test("a catch-up over the same range resumes, a different range does not", async () => {
    const { h } = await open("#/learn", [earlier("ns_week", "Catch up on forge since 2026-11-01")]);
    location.hash = understandHref(REPOSITORY);
    dispatchEvent(new HashChangeEvent("hashchange"));
    await h.settle();
    expect(byText($(document, "dialog[open]"), "button", "Continue where you left off")).toBeDefined();
    location.hash = understandHref({ ...REPOSITORY, since: "2026-11-02" });
    dispatchEvent(new HashChangeEvent("hashchange"));
    await h.settle();
    expect($(document, "dialog[open]").textContent).not.toContain("Continue where you left off");
  });
});
