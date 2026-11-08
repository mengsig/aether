// @vitest-environment happy-dom
import { connectionDraftKey, drafts } from "@aether-ui/core";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { demoWorld } from "./fixture/world.ts";
import { itemDraftKey } from "./NewItemDialog.tsx";
import { QueueScreen } from "./QueueScreen.tsx";
import { folderName, repositoryCommand, EMPTY_REPOSITORY, WATCH_DESKTOP_HINT } from "./RegisterRepository.tsx";
import { archonHarness, byText, click, CONN, pick, type, type ArchonHarness } from "./test/harness.tsx";

let h: ArchonHarness | undefined;
beforeEach(() => vi.useFakeTimers({ now: Date.parse("2026-11-04T10:00:00Z") }));
afterEach(() => { h?.stop(); h = undefined; document.body.innerHTML = ""; vi.useRealTimers(); });

it("derives a name from Unix and Windows folder paths and validates registration", () => {
  expect(folderName("/home/user/example/ ")).toBe("example");
  expect(folderName("C:\\work\\example\\")).toBe("example");
  expect(() => repositoryCommand(EMPTY_REPOSITORY)).toThrow(/path/);
  expect(() => repositoryCommand({ ...EMPTY_REPOSITORY, path: "/work/example", branch: "" })).toThrow(/base branch/);
  expect(repositoryCommand({ ...EMPTY_REPOSITORY, path: " /work/example ", name: "example", pipeline: "direct", watch: true }))
    .toEqual({ cmd: "repo_add", path: "/work/example", name: "example", default_pipeline: "direct", base_branch: "main", ship: "pr", github_watch: true });
  expect(repositoryCommand({ ...EMPTY_REPOSITORY, path: "/work/local", ship: "local" })).toMatchObject({ ship: "local", github_watch: false });
});

it("registers a repository, fills its name, and opens a preselected new item", async () => {
  h = await archonHarness({ protocol: 24 });
  const root = await h.mount(<QueueScreen />);
  await click(h, byText(root, "button", "More"));
  await click(h, byText(root, '[role="menuitem"]', "Register a repository"));
  expect(document.querySelector<HTMLButtonElement>('[data-testid="repo-register-submit"]')?.disabled).toBe(true);
  await type(h, document.querySelector('[data-testid="repo-path"]') as Element, "/home/user/example");
  expect(document.querySelector<HTMLInputElement>('[data-testid="repo-name"]')?.value).toBe("example");
  await pick(h, "repo-default-pipeline", "direct");
  await click(h, document.querySelector('[role="switch"]') as Element);
  await click(h, document.querySelector('[data-testid="repo-register-submit"]') as Element);
  await h.until(() => document.querySelector('[data-testid="new-item-repo"]') !== null, "new item after registration");
  expect(h.faux.commands("repo_add")[0]).toMatchObject({ path: "/home/user/example", name: "example", default_pipeline: "direct", base_branch: "main", ship: "pr", github_watch: true });
  expect(document.querySelector('[data-testid="new-item-repo"]')?.textContent).toBe("example");
  expect(document.querySelector('[data-testid="new-item-pipeline"]')?.textContent).toBe("direct");
  expect(drafts.get(connectionDraftKey("archon", CONN, "register-repository"))).toBeUndefined();
});

it("keeps a new item draft across closing and clears it only on success", async () => {
  h = await archonHarness({ protocol: 24 });
  const root = await h.mount(<QueueScreen />);
  await click(h, byText(root, "button", "New item"));
  await type(h, document.querySelector('[data-testid="new-item-title"]') as Element, "Keep my work");
  await type(h, document.querySelector('[data-testid="new-item-brief"]') as Element, "A durable brief");
  await click(h, byText(document, "dialog[open] button", "Cancel"));
  await click(h, byText(root, "button", "New item"));
  expect(document.querySelector<HTMLInputElement>('[data-testid="new-item-title"]')?.value).toBe("Keep my work");
  // An item without a repository is valid (ask items need none), so the submit is already enabled.
  expect(document.querySelector<HTMLButtonElement>('[data-testid="new-item-submit"]')?.disabled).toBe(false);
  await pick(h, "new-item-repo", "site");
  await click(h, document.querySelector('[data-testid="new-item-submit"]') as Element);
  expect(drafts.get(itemDraftKey(CONN))).toBeUndefined();
  expect(h.faux.commands("enqueue")[0]).toMatchObject({ title: "Keep my work", repo: "repo_site", pipeline: null });
});

it("keeps what was typed while a slow enqueue ran, and keys the draft by connection", async () => {
  expect(itemDraftKey(CONN)).toBe("archon/archon:faux/new-item");
  expect(itemDraftKey("archon:other")).not.toBe(itemDraftKey(CONN));
  h = await archonHarness({ protocol: 24 });
  const root = await h.mount(<QueueScreen />);
  await click(h, byText(root, "button", "New item"));
  await type(h, document.querySelector('[data-testid="new-item-title"]') as Element, "Slow item");
  await type(h, document.querySelector('[data-testid="new-item-brief"]') as Element, "First brief");
  h.faux.hold.add("enqueue");
  await click(h, document.querySelector('[data-testid="new-item-submit"]') as Element);
  expect(h.faux.commands("enqueue")).toHaveLength(1);
  await type(h, document.querySelector('[data-testid="new-item-brief"]') as Element, "Second brief, typed meanwhile");
  h.faux.release("enqueue");
  await h.until(() => document.querySelector("dialog[open]") === null, "the dialog to close after the enqueue");
  expect(drafts.get(itemDraftKey(CONN))).toMatchObject({ title: "Slow item", brief: "Second brief, typed meanwhile" });
});

it("keeps filters through remount and chooses a state with its picker", async () => {
  h = await archonHarness({ protocol: 24 });
  const root = await h.mount(<QueueScreen />);
  await pick(h, "queue-filter-state", "Done2");
  await type(h, root.querySelector('input[type="search"]') as Element, "changelog");
  await h.unmount(root);
  const reopened = await h.mount(<QueueScreen />);
  expect(reopened.querySelector<HTMLInputElement>('input[type="search"]')?.value).toBe("changelog");
  expect(reopened.querySelector(".queue-title")?.textContent).toBe("Write the 1.5 changelog");
});

it("guides an empty queue to registration and keeps it unavailable to observers", async () => {
  const world = demoWorld(); world.items.clear(); world.repos.length = 0;
  h = await archonHarness({ protocol: 24 }, world);
  const root = await h.mount(<QueueScreen />);
  await h.until(() => root.querySelector(".ui-state-title")?.textContent === "Register a repository", "registration guidance");
  await click(h, byText(root, ".ui-state button", "Register a repository"));
  expect(document.querySelector('[data-testid="repo-path"]')).not.toBeNull();
  h.stop(); document.body.innerHTML = "";
  h = await archonHarness({ protocol: 24, role: "observer" }, world);
  const observer = await h.mount(<QueueScreen />);
  expect(observer.querySelector('[data-testid="repo-register"]')).toBeNull();
  expect(observer.querySelector('[aria-haspopup="menu"]')).toBeNull();
  expect(observer.querySelector(".ui-state button")).toBeNull();
});

it("uses the desktop folder picker and preserves an edited repository name", async () => {
  h = await archonHarness({ protocol: 24 });
  const browse = vi.fn(async () => "/home/user/example");
  Object.defineProperty(h.app.host, "kind", { value: "electron" });
  Object.defineProperty(h.app.host, "desktop", { value: { pickDirectory: browse } });
  const root = await h.mount(<QueueScreen />);
  await click(h, byText(root, "button", "More"));
  await click(h, byText(root, '[role="menuitem"]', "Register a repository"));
  await click(h, document.querySelector('[data-testid="repo-path-browse"]') as Element);
  expect(browse).toHaveBeenCalledWith("repository");
  expect(document.querySelector<HTMLInputElement>('[data-testid="repo-name"]')?.value).toBe("example");
  await type(h, document.querySelector('[data-testid="repo-name"]') as Element, "Custom name");
  await type(h, document.querySelector('[data-testid="repo-path"]') as Element, "/home/user/other");
  expect(document.querySelector<HTMLInputElement>('[data-testid="repo-name"]')?.value).toBe("Custom name");
  const watch = document.querySelector<HTMLButtonElement>('[role="switch"]');
  expect(watch?.disabled).toBe(true);
  expect(document.body.textContent).toContain(WATCH_DESKTOP_HINT);
  await click(h, byText(document.body, "button", "Keep it local"));
  await click(h, document.querySelector('[data-testid="repo-register-submit"]') as Element);
  await h.until(() => h?.faux.commands("repo_add").length === 1, "the registration");
  expect(h.faux.commands("repo_add")[0]).toMatchObject({ path: "/home/user/other", ship: "local", github_watch: false });
});

it("keeps a question answer through navigation and clears it after sending", async () => {
  h = await archonHarness({ protocol: 24 });
  const root = await h.mount(<QueueScreen />);
  await h.until(() => root.querySelector(".queue-answer input") !== null, "the question");
  await type(h, root.querySelector(".queue-answer input") as Element, "Keep the draft");
  await h.unmount(root);
  const reopened = await h.mount(<QueueScreen />);
  await h.until(() => reopened.querySelector(".queue-answer input") !== null, "the restored question");
  expect(reopened.querySelector<HTMLInputElement>(".queue-answer input")?.value).toBe("Keep the draft");
  await click(h, byText(reopened, ".queue-answer button", "Send"));
  expect(h.faux.commands("answer")[0]).toMatchObject({ question: "q_review_gate", text: "Keep the draft" });
  expect(drafts.get(connectionDraftKey("archon", CONN, "question-q_review_gate"))).toBeUndefined();
  expect(drafts.keys().filter((key) => key.includes("question-"))).toEqual([]);
});
