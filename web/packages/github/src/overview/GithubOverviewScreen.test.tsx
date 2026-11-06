// @vitest-environment happy-dom
import { drafts } from "@aether-ui/core";
import { act } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { harness, click, buttonNamed, type Harness } from "../issue/test/harness.tsx";
import { agent, item, link, object, ref, type GithubWorld } from "../issue/fixture/world.ts";
import GithubOverviewScreen from "./GithubOverviewScreen.tsx";
import routes from "./routes.ts";

vi.mock("@aether-ui/graph", async (original) => ({ ...await original<typeof import("@aether-ui/graph")>(), GraphCanvas: ({ onActivate, model }: { onActivate: (id: string) => void; model: { nodes: { id: string; label: string }[] } }) =>
  <div>{model.nodes.map((node) => <button key={node.id} onClick={() => onActivate(node.id)}>{node.label}</button>)}</div> }));

let h: Harness | undefined;
beforeEach(() => { vi.useFakeTimers({ now: Date.parse("2026-11-06T12:00:00Z") }); drafts.clear("github/overview-filters"); });
afterEach(() => { h?.stop(); h = undefined; vi.useRealTimers(); });

function world(): GithubWorld {
  const issue = ref("example-org/site", 1, "issue");
  const pr = ref("example-org/site", 2, "pull_request");
  return { objects: [object(issue, "Improve navigation", "open", { author: "octocat", labels: ["ui"] }), object(pr, "Ship navigation", "merged")],
    items: new Map([["qi_one", item("qi_one", "Navigation work", { state: "running" }, { issue, stages: [{ aether_agent: "agt_one", stage_id: "build" }] })]]), links: [link("qi_one", issue, "origin"), link("qi_one", pr, "delivered")], agents: [agent("agt_one", "Example builder", "running")] };
}

async function pick(id: string, label: string) {
  const trigger = document.querySelector<HTMLElement>(`[data-testid="${id}"]`);
  if (!trigger) throw new Error(`missing picker ${id}`);
  await act(async () => trigger.click());
  const option = [...document.querySelectorAll<HTMLElement>('[role="option"]')].find((node) => node.textContent === label);
  if (!option) throw new Error(`missing option ${label}`);
  await act(async () => option.click());
  await h?.tick();
}

describe("GitHub overview", () => {
  it("registers a navigable GitHub overview route", () => {
    expect(routes[0]).toMatchObject({ path: "#/github", owner: "D4", nav: { label: "Overview", section: "github" } });
  });
  it("guides an operator when watching is off", async () => {
    h = await harness({ world: world(), before: (faux) => { faux.watcher.enabled = false; } });
    const root = await h.mount(<GithubOverviewScreen />);
    await h.until(() => root.textContent?.includes("GitHub watcher is off") === true, "watcher guidance");
    expect(root.querySelector('a[href="#/settings"]')?.textContent).toBe("Open Settings");
    expect(h.faux.commands("list_github_objects")).toHaveLength(0);
  });
  it("offers refresh with the actual poll interval for an empty cache", async () => {
    const empty = world(); empty.objects.length = 0;
    h = await harness({ world: empty });
    const root = await h.mount(<GithubOverviewScreen />);
    await h.until(() => root.textContent?.includes("Nothing fetched yet") === true, "empty cache");
    expect(root.textContent).toContain("polls every 120 s");
    expect(h.faux.commands("list_github_objects")[0]).toMatchObject({ repo: "example-org/site", limit: 500 });
  });
  it("shows cached objects, relationships, chart descriptions and filters without changing reads", async () => {
    h = await harness({ world: world(), aether: true });
    const root = await h.mount(<GithubOverviewScreen />);
    await h.until(() => root.querySelectorAll(".gh-object-table tbody tr").length === 2, "cached objects");
    expect(root.querySelector('a[href="#/github/github.com/example-org/site/1"]')?.textContent).toContain("Improve navigation");
    expect(root.querySelector('[data-status="pr-merged"]')).not.toBeNull();
    expect(root.querySelector('[data-testid="gh-graph"]')?.textContent).toContain("Navigation work");
    expect(root.querySelectorAll('svg[role="img"]')).toHaveLength(5);
    expect(root.querySelector('a[href^="#/agents/"]')?.textContent).toBe("Example builder");
    await click(h, buttonNamed(root, "#1 Improve navigation"));
    expect(root.querySelector(".gh-node-detail")?.textContent).toContain("Open GitHub object");
    await pick("gh-filter-repo", "example-org/site");
    await pick("gh-filter-kind", "Pull requests");
    await pick("gh-filter-state", "merged");
    expect(root.querySelectorAll(".gh-object-table tbody tr")).toHaveLength(1);
    expect(root.querySelector(".gh-object-table tbody")?.textContent).toContain("Ship navigation");
    expect(h.faux.commands("issue_items")[0]).toMatchObject({ ref: { host: "github.com", repo: "example-org/site", kind: "issue", number: 1 } });
  });
  it("shows failed watcher reads with a retry and Settings next step", async () => {
    h = await harness({ world: world(), before: (faux) => faux.failing.add("github_watcher") });
    const root = await h.mount(<GithubOverviewScreen />);
    await h.until(() => root.textContent?.includes("Watcher unreachable") === true, "watcher error");
    expect(root.textContent).toContain("github_watcher failed");
    expect(root.querySelector('a[href="#/settings"]')).not.toBeNull();
  });
});
