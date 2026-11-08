import { buildScene, visibleGraph } from "@aether-ui/graph";
import { describe, expect, it } from "vitest";
import type { archon } from "@aether-ui/protocol";
import type { ItemView } from "../issue/join.ts";
import { filterRows, groupRows, itemsPerPr, objectHref, objectId, relationshipGraph, relationshipLayout, weeklyBuckets, type ObjectRow } from "./model.ts";

const item: ItemView = { id: "qi_one", title: "Build", relations: ["delivered"], execution: { status: "running", text: "Running", label: "Execution", detail: null }, outcome: null, pr: null, runs: [], agents: [{ agent: "agt_one", stages: ["build"] }] };
function row(number: number, state: archon.GithubObject["state"], kind: archon.GithubKind = "issue", repo = "example-org/site", updated = "2026-11-04T10:00:00Z"): ObjectRow {
  return { object: { ref: { host: "github.com", repo, number, kind }, title: `Object ${number}`, state, labels: [], updated_at: updated, fetched_at: updated }, items: [item] };
}

describe("GitHub overview model", () => {
  it("groups repositories and filters by repository, kind and state", () => {
    const rows = [row(1, "open"), row(2, "merged", "pull_request"), row(3, "closed", "issue", "example-org/api")];
    expect([...groupRows(rows).keys()]).toEqual(["example-org/site", "example-org/api"]);
    expect(filterRows(rows, { repo: "example-org/site", kind: "pull_request", state: "merged" })).toEqual([rows[1]]);
  });
  it("connects repository to objects to distinct linked items and agents", () => {
    const graph = relationshipGraph([row(1, "open"), row(2, "merged", "pull_request")]);
    expect(graph.groups).toEqual([{ id: "group:example-org/site", label: "example-org/site" }]);
    expect(graph.nodes).toHaveLength(5);
    expect(graph.edges).toHaveLength(5);
    const scene = buildScene(visibleGraph(graph), relationshipLayout(graph), "box");
    expect(scene.unplaced).toBe(0);
    expect(scene.groups).toHaveLength(1);
    expect(graph.nodes.find((node) => node.id === "item:qi_one")?.status).toBe("running");
    expect(graph.edges).toContainEqual({ from: "item:qi_one", to: "agent:agt_one", kind: "agent" });
  });
  it("buckets eight Monday-based UTC weeks without inventing event history", () => {
    const buckets = weeklyBuckets([row(1, "open"), row(2, "merged", "pull_request"), row(3, "closed", "issue", "example-org/site", "2026-09-14T00:00:00Z"), row(4, "open", "issue", "example-org/site", "2026-11-09T00:00:00Z")], Date.parse("2026-11-06T00:00:00Z"));
    expect(buckets).toHaveLength(8);
    expect(buckets[0]).toEqual({ label: "2026-09-14", open: 0, closed: 1, merged: 0 });
    expect(buckets[7]).toEqual({ label: "2026-11-02", open: 1, closed: 0, merged: 1 });
  });
  it("counts unique items for PRs only", () => {
    const pr = row(2, "merged", "pull_request");
    expect(itemsPerPr([row(1, "open"), { ...pr, items: [item, item] }])).toEqual([{ label: "example-org/site#2", value: 1 }]);
  });
  it("keeps object links compatible with the detail route and rejects malformed ids", () => {
    const ref = row(1, "open").object.ref;
    expect(objectHref(ref)).toBe("#/github/github.com/example-org/site/1");
    expect(() => objectId({ ...ref, number: 0 })).toThrow(/positive number/);
  });
});
