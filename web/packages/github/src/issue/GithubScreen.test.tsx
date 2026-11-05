// @vitest-environment happy-dom
import { registerEntityAction } from "@aether-ui/core";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import "../index.ts";
import { buttonNamed, click, facts, harness, type Harness } from "./fixture/harness.test-util.tsx";
import { GITHUB_FEATURES } from "./fixture/faux.ts";

const learned: string[] = [];
// Stands for F6's "learn from this": another feature's action, shown only when Aether is connected.
registerEntityAction({
  id: "test.learn", kind: "pull_request", label: "Learn from this",
  requires: (caps) => caps["aether.connected"] === true, run: (ref) => { learned.push(ref.id); },
});

let h: Harness | null = null;
beforeEach(() => { vi.useFakeTimers({ now: Date.parse("2026-11-04T10:00:00Z") }); });
afterEach(() => {
  h?.stop();
  h = null;
  learned.length = 0;
  vi.useRealTimers();
});

const section = (container: HTMLElement, title: string) => container.querySelector<HTMLElement>(`section[aria-label="${title}"]`);
const itemCard = (container: HTMLElement, id: string) => container.querySelector<HTMLElement>(`.gh-item[data-item="${id}"]`);

async function page(repo: string, number: number, options: Parameters<typeof harness>[0] = {}) {
  h = await harness(options);
  const container = await h.open(repo, number);
  return { h, container };
}

async function loaded(repo: string, number: number, options: Parameters<typeof harness>[0] = {}) {
  const opened = await page(repo, number, options);
  await opened.h.until(() => opened.container.querySelector(".gh-title") !== null, `the page of ${repo}#${number}`);
  return opened;
}

describe("with Archon only", () => {
  test("an issue page joins its cached state, linked item, delivered PR and runs", async () => {
    const { h, container } = await loaded("acme/site", 41);
    expect(container.querySelector(".gh-title")?.textContent).toBe("Dark mode for the dashboard");
    expect(facts(container.querySelector('[aria-label="GitHub state"]'))).toEqual(["Issue: Open"]);
    await h.until(() => itemCard(container, "qi_dark_mode")?.querySelector('[aria-label="Runs"]') !== null, "the item's runs");
    const card = itemCard(container, "qi_dark_mode");
    expect(facts(card)).toEqual(["Execution: Running", "Delivered PR: #57 · Draft"]);
    expect(card?.textContent).toContain("Review required · Checks pending");
    expect(card?.querySelector('a[href="#/archon/items/qi_dark_mode/runs/run_dark"]')).not.toBeNull();
    expect(section(container, "Agents")?.textContent).toBe("AgentsConnect Aether to see the agents that worked on this.");
  });

  test("a PR page shows merged beside the item's succeeded, never one as the other", async () => {
    const { h, container } = await loaded("acme/site", 52);
    expect(facts(container.querySelector('[aria-label="GitHub state"]'))).toEqual(["Pull request: Merged"]);
    expect(container.querySelector('[aria-label="GitHub state"]')?.textContent).toContain("Approved · 6 checks passed");
    await h.until(() => itemCard(container, "qi_changelog") !== null, "the linked item");
    expect(facts(itemCard(container, "qi_changelog"))).toEqual(["Execution: Succeeded"]);
    expect(itemCard(container, "qi_changelog")?.textContent).toContain("Changelog drafted and delivered.");
    expect(container.querySelector(".gh-import")).toBeNull();
    expect(container.textContent).not.toContain("Learn from this");
  });
});

describe("with Aether connected too", () => {
  test("names the agents the item's stages ran on, linked to their pages", async () => {
    const { h, container } = await loaded("acme/site", 41, { aether: true });
    await h.until(() => section(container, "Agents")?.querySelector("li") !== null, "the agents");
    const agents = [...(section(container, "Agents")?.querySelectorAll("li") ?? [])].map((row) => row.textContent);
    expect(agents).toEqual(["site-buildercompletedDark mode for the dashboard · build", "site-reviewerrunningDark mode for the dashboard · review"]);
    expect(section(container, "Agents")?.querySelector('a[href="#/agents/aether%3Astudio/agt_builder"]')).not.toBeNull();
  });

  test("says when a stage's agent is not on the connected Aether, and shows other features' PR actions", async () => {
    const { h, container } = await loaded("acme/site", 52, { aether: true });
    await h.until(() => section(container, "Agents")?.querySelector("li") !== null, "the agents");
    expect(section(container, "Agents")?.textContent).toContain("agt_writernot on the connected Aether");
    await click(h, buttonNamed(container, "Learn from this"));
    expect(learned).toEqual(["github.com/acme/site#52"]);
    expect(container.textContent).not.toContain("Open pull request page");
  });
});

describe("the page's states", () => {
  test("loading while archond has not answered", async () => {
    const { container } = await page("acme/site", 41, { before: (faux) => { faux.holding.add("list_github_objects"); faux.holding.add("issue_items"); } });
    expect(container.querySelector('[aria-busy="true"]')?.getAttribute("aria-label")).toBe("Loading the GitHub page");
  });

  test("an error with a retry when every read fails", async () => {
    const { h, container } = await page("acme/site", 41, { before: (faux) => { faux.failing.add("list_github_objects"); faux.failing.add("issue_items"); } });
    await h.until(() => container.querySelector('[role="alert"]') !== null, "the error");
    expect(container.querySelector('[role="alert"]')?.textContent).toContain("Could not read this from Archon");
    h.faux.failing.clear();
    await click(h, buttonNamed(container, "Try again"));
    await h.until(() => container.querySelector(".gh-title") !== null, "the page after the retry");
  });

  test("empty when Archon knows nothing of the number", async () => {
    const { h, container } = await page("acme/site", 99);
    await h.until(() => container.querySelector(".ui-state") !== null, "the empty state");
    expect(container.querySelector(".ui-state-title")?.textContent).toBe("Archon knows nothing of acme/site#99 yet");
  });

  test("a stale cache says when GitHub was last read", async () => {
    vi.setSystemTime(Date.parse("2026-11-04T10:40:00Z"));
    const { container } = await loaded("acme/site", 41);
    expect(container.querySelector(".gh-note")?.textContent).toMatch(/^Archon last read this from GitHub at .+; GitHub may have moved on since\.$/);
  });

  test("a fresh cache shows no stale note, and a failed reread keeps the last read with its error", async () => {
    const { h, container } = await loaded("acme/site", 41);
    expect(container.querySelector(".gh-note")).toBeNull();
    h.faux.failing.add("list_github_objects");
    h.faux.tell({ subject: "item", item: "qi_dark_mode", state: "running" });
    await h.until(() => container.querySelector(".gh-note") !== null, "the reread's failure", 3_000);
    expect(container.querySelector(".gh-note")?.textContent).toBe("Showing the last read: rereading failed (list_github_objects failed in the faux archond).");
    expect(container.querySelector(".gh-title")?.textContent).toBe("Dark mode for the dashboard");
  });

  test("an observer reads the page, and the import is disabled with the reason", async () => {
    const { container } = await loaded("acme/site", 77, { role: "observer" });
    expect(container.textContent).toContain("This device observes Archon");
    expect(buttonNamed(container, "Import as Archon work").disabled).toBe(true);
    expect(container.querySelector(".gh-reason")?.textContent).toBe("Observer devices cannot change daemon state");
  });

  test("not a GitHub object when the route's params name none", async () => {
    const { h, container } = await page("acme/site", 41);
    await h.until(() => container.querySelector(".gh-title") !== null, "the page");
    const bad = await h.open("acme/..", 3);
    expect(bad.querySelector(".ui-state-title")?.textContent).toBe("Not a GitHub issue or pull request");
  });
});

describe("capabilities", () => {
  test("without github_import the import is disabled with the reason", async () => {
    const { container } = await loaded("acme/site", 41, { features: GITHUB_FEATURES.filter((feature) => feature !== "github_import") });
    expect(buttonNamed(container, "Import as Archon work").disabled).toBe(true);
    expect(container.querySelector(".gh-reason")?.textContent).toBe("This archond cannot import issues: it does not serve github_import");
  });

  test("an archond with no GitHub features says so", async () => {
    const { h, container } = await page("acme/site", 41, { features: ["changes", "device_roles"] });
    await h.until(() => container.querySelector(".ui-state-title") !== null, "the explanation");
    expect(container.querySelector(".ui-state-title")?.textContent).toBe("This archond does not track GitHub");
  });

  test("an issue already imported cannot be imported again", async () => {
    const { container } = await loaded("acme/site", 41);
    expect(buttonNamed(container, "Import as Archon work").disabled).toBe(true);
    expect(container.querySelector(".gh-reason")?.textContent).toBe("Already Archon work: Dark mode for the dashboard");
  });
});

describe("importing an issue", () => {
  test("previews the issue, imports it on confirm, and then lists the new item", async () => {
    const { h, container } = await loaded("acme/site", 77);
    await click(h, buttonNamed(container, "Import as Archon work"));
    const dialog = container.querySelector("dialog");
    expect(dialog?.textContent).toContain("Export the dashboard as CSV");
    expect(dialog?.textContent).toContain("grants no authority");
    expect(h.faux.commands("import_issue")).toEqual([]);
    await click(h, buttonNamed(container, "Import"));
    expect(h.faux.commands("import_issue").map(({ ref, priority }) => ({ ref, priority }))).toEqual([
      { ref: { host: "github.com", repo: "acme/site", number: 77, kind: "issue" }, priority: "normal" },
    ]);
    await h.until(() => itemCard(container, "qi_import_77") !== null, "the imported item", 3_000);
    expect(container.querySelector('[role="status"]')?.textContent).toBe("Imported as Export the dashboard as CSV");
    await h.until(() => facts(itemCard(container, "qi_import_77")).includes("Execution: Queued"), "the imported item's summary", 3_000);
    expect(buttonNamed(container, "Import as Archon work").disabled).toBe(true);
  });

  test("Not now closes the preview without importing", async () => {
    const { h, container } = await loaded("acme/site", 77);
    await click(h, buttonNamed(container, "Import as Archon work"));
    await click(h, buttonNamed(container, "Not now"));
    expect(h.faux.commands("import_issue")).toEqual([]);
  });
});
