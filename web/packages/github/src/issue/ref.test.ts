// @vitest-environment happy-dom
import type { Capabilities, EntityRef } from "@aether-ui/protocol";
import { describe, expect, test } from "vitest";
import { ACTIONS, canImport, canOpen } from "./actions.ts";
import { pageLinks } from "./join.ts";
import { entityId, pageHref, pageOfEntity, parsePage } from "./ref.ts";

const page = { host: "github.com", repo: "acme/site", number: 41 };
const issue: EntityRef = { kind: "issue", conn: "archon:faux", id: "github.com/acme/site#41" };

describe("page refs", () => {
  test("parses a route's params and refuses what names no object", () => {
    expect(parsePage({ host: "GitHub.com", owner: "acme", repo: "site", number: "41" })).toEqual(page);
    for (const bad of [{ number: "0" }, { number: "4a" }, { number: "-1" }, { owner: ".." }, { repo: "a/b" }, { host: "bad host" }, { number: "99999999999999999" }]) {
      expect(parsePage({ host: "github.com", owner: "acme", repo: "site", number: "41", ...bad })).toBeNull();
    }
  });

  test("an entity id round-trips to its page, and the page's href encodes each part", () => {
    expect(entityId(page)).toBe(issue.id);
    expect(pageOfEntity(issue)).toEqual(page);
    expect(pageOfEntity({ ...issue, kind: "item" })).toBeNull();
    expect(pageOfEntity({ ...issue, id: "qi_123" })).toBeNull();
    expect(pageHref({ host: "ghe.example:8443", repo: "acme/site", number: 7 })).toBe("#/github/ghe.example%3A8443/acme/site/7");
  });

  test("keeps one link per item and relation, and only links to the page's object", () => {
    const at = { host: "github.com", repo: "Acme/Site", number: 41, kind: "issue" as const };
    const one = { item: "qi_1", ref: at, relation: "origin" as const, source: "watcher" as const, created_at: "2026-11-04T09:00:00Z" };
    const other = { ...one, item: "qi_2", ref: { ...at, number: 42 } };
    expect(pageLinks(page, [[one, other], [one]])).toEqual([one]);
  });
});

describe("entity actions", () => {
  const ready: Capabilities = { "archon.connected": true, "archon.feature.github_links": true, "archon.feature.github_import": true, "can.operate.archon:faux": true };

  test("open needs a connected archond that tracks GitHub, and an id this package made", () => {
    expect(canOpen(ready, issue)).toBe(true);
    expect(canOpen({ ...ready, "archon.connected": false }, issue)).toBe(false);
    expect(canOpen({ "archon.connected": true }, issue)).toBe(false);
    expect(canOpen(ready, { ...issue, conn: "aether:studio" })).toBe(false);
    expect(canOpen(ready, { ...issue, id: "not-a-ref" })).toBe(false);
  });

  test("import is hidden without github_import or an operator's standing, and on pull requests", () => {
    expect(canImport(ready, issue)).toBe(true);
    expect(canImport({ ...ready, "archon.feature.github_import": false }, issue)).toBe(false);
    expect(canImport({ ...ready, "can.operate.archon:faux": false }, issue)).toBe(false);
    expect(canImport(ready, { ...issue, kind: "pull_request" })).toBe(false);
  });

  test("open and import navigate to the page; import opens its preview there, never importing at once", async () => {
    const byId = new Map(ACTIONS.map((action) => [action.id, action]));
    await byId.get("d4.open-issue")?.run(issue);
    expect(location.hash).toBe("#/github/github.com/acme/site/41");
    await byId.get("d4.import-issue")?.run(issue);
    expect(location.hash).toBe("#/github/github.com/acme/site/41?import");
    await byId.get("d4.open-pull-request")?.run({ ...issue, kind: "pull_request", id: "github.com/acme/site#57" });
    expect(location.hash).toBe("#/github/github.com/acme/site/57");
    expect(() => byId.get("d4.open-issue")?.run({ ...issue, id: "nope" })).toThrow("does not name a GitHub issue");
  });
});
