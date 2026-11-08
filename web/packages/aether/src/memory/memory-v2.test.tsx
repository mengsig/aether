// @vitest-environment happy-dom
import { drafts } from "@aether-ui/core";
import { act } from "react";
import { beforeEach, afterEach, expect, test, vi } from "vitest";
import type { DesktopBridge } from "@aether-ui/protocol";
import { buttonNamed, click, connected, type, type Harness } from "../controls/fixture/harness.test-util.tsx";
import { demoAgents } from "../controls/fixture/demo.ts";
import type { Role } from "../controls/fixture/daemon.ts";
import MemoryScreen from "./MemoryScreen.tsx";
import { MemoryRow } from "./MemoryRow.tsx";
import { DEMO_ARTIFACT, demoMemories, memoryScript, TASK_ID, INVOCATION_ID, V2_FEATURES } from "./fixture/data.ts";

let h: Harness;
afterEach(() => h?.unmount());
const OLD_FEATURES = V2_FEATURES.filter(feature => feature !== "memory_v2");

async function screen(role: Role = "local", features: readonly string[] = V2_FEATURES, desktop?: DesktopBridge) {
  h = await connected({ role, features, agents: demoAgents(), script: memoryScript(), ...(desktop ? { desktop } : {}) });
  return h.mount(<MemoryScreen params={{ conn: h.conn }} />);
}

const section = (root: HTMLElement, name: string) => root.querySelector(`section[aria-label="${name}"]`) as HTMLElement | null;

/** Chooses `text` as the import file, as a user picking it would. */
async function chooseFile(root: HTMLElement, text: string) {
  const input = root.querySelector('input[type="file"]') as HTMLInputElement;
  Object.defineProperty(input, "files", { configurable: true, value: [new File([text], "memories.jsonl", { type: "application/jsonl" })] });
  await act(async () => { input.dispatchEvent(new Event("change", { bubbles: true })); });
  await h.settle(); await h.settle();
}

test("the hello lists memory_v2, so the daemon sends standing", async () => {
  await screen();
  expect(h.daemon.commands("hello")[0]?.features).toContain("memory_v2");
});

test("a row shows tier, state, support, verification, refs and the stale note", async () => {
  h = await connected({ features: V2_FEATURES });
  const [verified, stale, inferred] = demoMemories();
  const root = await h.mount(<>{[verified, stale, inferred].map(m => m && <MemoryRow key={m.id} conn={h.conn} memory={m} refresh={async () => {}} />)}</>);
  const [first, second, third] = [...root.querySelectorAll(".mem-standing")].map(node => node.textContent ?? "");
  expect(first).toContain("Active");
  expect(first).toContain("Verified");
  expect(first).toContain("Tier architecture · 4 tasks · 3 independent runs");
  expect(first).toContain("path:docs/memory/MEMORY-V2.md");
  expect(first).toContain("commit:72a8cfc8");
  expect(second).toContain("Stale since 2026-11-04 11:02 UTC at c167cec8 · web/package-lock.json no longer pins vitest 4");
  expect(third).toContain("Candidate");
  expect(third).toContain("Inferred · unverified");
  expect(third).not.toMatch(/Verified|Observed/);
  expect(root.querySelector(`[data-verification="inferred"]`)).not.toBeNull();
});

test("without standing (an older daemon) a row renders as before", async () => {
  const root = await screen("local", OLD_FEATURES);
  const legacy = [...root.querySelectorAll("article.b3-item")].find(a => a.textContent?.includes("Written before admission"));
  expect(legacy?.querySelector(".mem-standing")).toBeNull();
  expect(legacy?.textContent).toBe("Written before admission existed; no standing is known.factGloballegacy · 2026-11-02T08:14:00ZPinForget");
  for (const name of ["Retrieval explain", "Maintenance", "Store"]) expect(section(root, name), name).toBeNull();
  for (const cmd of ["memory_migration_status", "memory_retrieval_explain", "consolidate_memories"]) expect(h.daemon.commands(cmd), cmd).toHaveLength(0);
});

test("explain shows each turn's injected memories, candidates, policy, pool and the untraced state", async () => {
  const root = await screen();
  const explain = section(root, "Retrieval explain") as HTMLElement;
  await type(h, explain, "Task id", "inv_wrong");
  expect(explain.textContent).toContain("Task ids start with tsk_");
  expect(buttonNamed(explain, "Explain").disabled).toBe(true);
  await type(h, explain, "Task id", TASK_ID);
  await click(h, explain, "Explain");
  expect(h.daemon.commands("memory_retrieval_explain")).toEqual([expect.objectContaining({ task: TASK_ID })]);
  const [traced, untraced] = [...explain.querySelectorAll(".mem-turn")] as HTMLElement[];
  const text = traced?.textContent ?? "";
  expect(traced?.querySelector(".mem-ids")?.textContent).toContain("Protocol changes ship behind a feature string");
  for (const fact of ["9 candidates", "Max items6", "Min score0.05", "Holdout rate5%", "0 over limit · 3 cut"]) expect(text, fact).toContain(fact);
  const outcomes = [...(traced?.querySelectorAll(".mem-candidate") ?? [])].map(row => row.getAttribute("data-outcome"));
  expect(outcomes).toEqual(["injected", "injected", "held_out", "trimmed", "over_limit", "cut"]);
  const first = traced?.querySelector(".mem-candidate")?.textContent ?? "";
  expect(first).toMatch(/#1.*Injected.*Pinned.*score 0\.92.*relevance 0\.81/);
  expect(traced?.querySelectorAll(".mem-candidate")[1]?.textContent).toContain("Stale · half score");
  expect(traced?.querySelector('[data-outcome="cut"]')?.textContent).toContain("score —");
  expect(untraced?.getAttribute("data-traced")).toBe("false");
  expect(untraced?.textContent).toContain("Untraced.");
  expect(untraced?.querySelector(".mem-candidates")).toBeNull();
});

test("explain by invocation sends exactly the invocation", async () => {
  const root = await screen();
  const explain = section(root, "Retrieval explain") as HTMLElement;
  await click(h, explain, "Invocation");
  await type(h, explain, "Invocation id", INVOCATION_ID);
  await click(h, explain, "Explain");
  const sent = h.daemon.commands("memory_retrieval_explain")[0] ?? {};
  expect(sent).toMatchObject({ invocation: INVOCATION_ID });
  expect(sent).not.toHaveProperty("task");
  await click(h, explain, "Explain");
  expect(h.daemon.commands("memory_retrieval_explain")).toHaveLength(2);
});

test("import runs dry first, and writes only after the confirm", async () => {
  const root = await screen();
  const tools = section(root, "Maintenance") as HTMLElement;
  expect(buttonNamed(tools, "Dry run import").disabled).toBe(true);
  await chooseFile(root, DEMO_ARTIFACT);
  await click(h, tools, "Dry run import");
  expect(h.daemon.commands("memory_import")).toEqual([expect.objectContaining({ artifact: DEMO_ARTIFACT, dry_run: true })]);
  const plan = tools.querySelector('.mem-plan[data-kind="import"]')?.textContent ?? "";
  expect(plan).toMatch(/Would import from 12 records.*Would insert9.*Unchanged2.*Conflicts1/);
  expect(plan).toContain("the stored text differs from the exported one");
  const reads = h.daemon.commands("memories").length;
  await click(h, tools, "Import");
  expect(h.daemon.commands("memory_import")).toHaveLength(1);
  expect(document.body.textContent).toContain("9 memories will be inserted from memories.jsonl in one commit.");
  await click(h, document.body, "Import memories");
  await h.settle();
  expect(h.daemon.commands("memory_import")[1]).toMatchObject({ artifact: DEMO_ARTIFACT, dry_run: false });
  expect(tools.querySelector('.mem-plan[data-dry-run="false"]')?.textContent).toContain("Imported from 12 records");
  expect(h.daemon.commands("memories").length).toBeGreaterThan(reads);
});

test("import takes any artifact the daemon exports, up to its 48 MiB escaped bound", async () => {
  const root = await screen();
  const tools = section(root, "Maintenance") as HTMLElement;
  const large = DEMO_ARTIFACT + " ".repeat(40 * 1024 * 1024);
  await chooseFile(root, large);
  expect(root.querySelector('[role="alert"]'), "40 MiB of plain text fits").toBeNull();
  await click(h, tools, "Dry run import");
  const sent = h.daemon.commands("memory_import");
  expect(sent).toHaveLength(1);
  expect(sent[0]?.artifact === large && sent[0]?.dry_run === true).toBe(true);
  // 25 MiB of quotes escapes to 50 MiB: past the bound, so it is refused before anything is sent.
  await chooseFile(root, '"'.repeat(25 * 1024 * 1024));
  expect(root.querySelector('[role="alert"]')?.textContent).toContain("50.0 MiB on the wire");
  expect(buttonNamed(tools, "Dry run import").disabled).toBe(true);
  expect(h.daemon.commands("memory_import")).toHaveLength(1);
});

test("a dry run's late reply never plans a file chosen after it", async () => {
  const OTHER = DEMO_ARTIFACT.replace("\n", "\n\n");
  let held = false;
  const base = memoryScript();
  const answer = base.memory_import;
  expect(answer).toBeDefined();
  const script = { ...base, memory_import: (command: Record<string, unknown>) => {
    if (command.artifact === DEMO_ARTIFACT && !held) { held = true; return "hold" as const; }
    return answer ? answer(command) : { code: "internal", message: "no scripted import" };
  } };
  h = await connected({ role: "local", features: V2_FEATURES, agents: demoAgents(), script });
  const root = await h.mount(<MemoryScreen params={{ conn: h.conn }} />);
  const tools = section(root, "Maintenance") as HTMLElement;
  await chooseFile(root, DEMO_ARTIFACT);
  await click(h, tools, "Dry run import");
  expect((root.querySelector('input[type="file"]') as HTMLInputElement).disabled).toBe(true);
  await chooseFile(root, OTHER);
  await act(async () => { h.daemon.release(); });
  await h.settle(); await h.settle();
  const importButtons = [...tools.querySelectorAll("button")].filter(b => b.textContent?.trim() === "Import");
  expect(importButtons, "the first file's plan is dropped").toHaveLength(0);
  await click(h, tools, "Dry run import");
  await click(h, tools, "Import");
  await click(h, document.body, "Import memories");
  await h.settle();
  const sent = h.daemon.commands("memory_import").map(c => [c.artifact === OTHER, c.dry_run]);
  expect(sent).toEqual([[false, true], [true, true], [true, false]]);
});

test("consolidate plans for the typed scope, and a new scope drops the plan", async () => {
  const root = await screen();
  const tools = section(root, "Maintenance") as HTMLElement;
  await type(h, tools, "Consolidation scope", "agent:");
  expect(buttonNamed(tools, "Plan consolidation").disabled).toBe(true);
  await type(h, tools, "Consolidation scope", "repo:github.com/mengsig/aether");
  await click(h, tools, "Plan consolidation");
  expect(h.daemon.commands("consolidate_memories")).toEqual([expect.objectContaining({ dry_run: true, scope: { scope: "repository", id: "github.com/mengsig/aether" } })]);
  expect(tools.querySelector('.mem-plan[data-kind="consolidate"]')?.textContent).toMatch(/Planned over 42 memories · 2 clusters/);
  await click(h, tools, "Consolidate");
  expect(h.daemon.commands("consolidate_memories")).toHaveLength(1);
  await click(h, document.body, "Merge memories");
  await h.settle();
  expect(h.daemon.commands("consolidate_memories")[1]).toMatchObject({ dry_run: false, scope: { scope: "repository", id: "github.com/mengsig/aether" } });
  expect(tools.querySelector('.mem-plan[data-dry-run="false"]')?.textContent).toMatch(/merged.*Promoted.*skipped.*a member changed under the plan/);
  await click(h, tools, "Plan consolidation");
  await type(h, tools, "Consolidation scope", "global");
  expect(tools.querySelector('.mem-plan[data-dry-run="true"]')).toBeNull();
  expect([...tools.querySelectorAll("button")].some(b => b.textContent === "Consolidate")).toBe(false);
});

test("export offers the artifact as a download with its record count and digest", async () => {
  const createObjectURL = vi.spyOn(URL, "createObjectURL").mockReturnValue("blob:memories");
  const root = await screen();
  const tools = section(root, "Maintenance") as HTMLElement;
  await click(h, tools, "Export memories");
  expect(h.daemon.commands("memory_export")[0]).not.toHaveProperty("scope");
  const result = tools.querySelector('.mem-plan[data-kind="export"]') as HTMLElement;
  expect(result.textContent).toContain("2 records exported");
  expect(result.textContent).toContain("5d41402abc4b2a76b9719d911017c592ae2f5b0c3a7e0f1d2c4b6a8e9f0d1c2b");
  const link = result.querySelector("a") as HTMLAnchorElement;
  expect(link.getAttribute("href")).toBe("blob:memories");
  expect(link.getAttribute("download")).toBe("aether-memories-5d41402abc4b.jsonl");
  expect(await (createObjectURL.mock.calls[0]?.[0] as Blob).text()).toBe(DEMO_ARTIFACT);
  createObjectURL.mockRestore();
});

test("observers and paired devices see the refusal, not the host-only tools, but read the store status", async () => {
  for (const role of ["observer", "operator"] as const) {
    const root = await screen(role);
    for (const name of ["Retrieval explain", "Maintenance"]) {
      expect(section(root, name)?.textContent, `${role} ${name}`).toContain("Only the daemon's own host can do this");
      expect(section(root, name)?.querySelector("button, input"), `${role} ${name}`).toBeNull();
    }
    expect(section(root, "Store")?.textContent).toContain("Schema 32");
    for (const cmd of ["memory_retrieval_explain", "consolidate_memories", "memory_export", "memory_import"]) expect(h.daemon.commands(cmd)).toHaveLength(0);
    h.unmount();
  }
});

test("on the desktop the host can access the maintenance and explain tools", async () => {
  const desktop = { version: 1, connect: async () => "bridge", close: async () => {}, privileged: vi.fn(), getPreference: async () => false } as unknown as DesktopBridge;
  const root = await screen("local", V2_FEATURES, desktop);
  expect(section(root, "Maintenance")?.textContent).toContain("Plan consolidation");
  expect(section(root, "Retrieval explain")?.querySelector("input")).not.toBeNull();
});

test("the store panel shows the schema, migrations, counts, backups and last import", async () => {
  const root = await screen("observer");
  const store = section(root, "Store")?.textContent ?? "";
  for (const text of ["Schema 32", "V30 applied", "V31 applied", "V32 applied", "Memories128", "Implicit (no admission row)11", "Holdout log rows14",
    "schema-31-before-32-01J0G3.sqlite.zst", "V31 → V32 · 4.6 MiB · 2026-11-05 05:58 UTC", "Last import 2026-11-05 06:10 UTC · 9 inserted · 2 unchanged · 1 conflicts"]) {
    expect(store, text).toContain(text);
  }
  expect(h.daemon.commands("memory_migration_status")).toHaveLength(1);
  expect(root.textContent).toContain("Inferred · unverified");
});

beforeEach(() => { for (const key of drafts.keys()) drafts.clear(key); });

test("export and consolidation suggestions send the selected scope unchanged", async () => {
  vi.spyOn(URL, "createObjectURL").mockReturnValue("blob:scope-export");
  vi.spyOn(URL, "revokeObjectURL").mockReturnValue(undefined);
  const root = await screen();
  const maintenance = section(root, "Maintenance") as HTMLElement;
  await type(h, maintenance, "Export scope", "global");
  const global = [...maintenance.querySelectorAll<HTMLElement>('[role="option"]')].find(o => o.textContent === "Global");
  expect(global).toBeDefined();
  await act(async () => global?.click());
  await click(h, maintenance, "Export memories");
  expect(h.daemon.commands("memory_export")[0]).toMatchObject({ scope: { scope: "global" } });
  await type(h, maintenance, "Consolidation scope", "global");
  const choice = [...maintenance.querySelectorAll<HTMLElement>('[role="option"]')].find(o => o.textContent === "Global");
  await act(async () => choice?.click());
  await click(h, maintenance, "Plan consolidation");
  expect(h.daemon.commands("consolidate_memories")[0]).toMatchObject({ scope: { scope: "global" }, dry_run: true });
  vi.restoreAllMocks();
});


test("maintenance scope suggestions use stored identities rather than workspace roots", async () => {
  vi.spyOn(URL, "createObjectURL").mockReturnValue("blob:scope-export");
  vi.spyOn(URL, "revokeObjectURL").mockReturnValue(undefined);
  h = await connected({ role: "local", features: V2_FEATURES, agents: demoAgents().map(agent => ({ ...agent, scope: { workspace_root: "/workspace/root" } })),
    script: { ...memoryScript(), list_workspaces: () => ({ tag: "workspaces", data: [{ path: "/other/root", exists: true }] }) } });
  const root = await h.mount(<MemoryScreen params={{ conn: h.conn }} />);
  const maintenance = section(root, "Maintenance") as HTMLElement;
  for (const [toolName, label, optionName, action] of [
    ["Export", "Export scope", "github.com/mengsig/aether · agent", "Export memories"],
    ["Consolidate", "Consolidation scope", "Repository github.com/mengsig/aether", "Plan consolidation"],
  ]) {
    const tool = maintenance.querySelector(`[aria-label="${toolName}"]`) as HTMLElement;
    await type(h, tool, label, "repo");
    const options = [...tool.querySelectorAll<HTMLElement>('[role="option"]')];
    const labels = options.map(option => option.textContent);
    expect(labels).toContain("Repository github.com/mengsig/aether");
    expect(labels).toContain("github.com/mengsig/aether · agent");
    expect(labels.join(" ")).not.toContain("/workspace/root");
    expect(labels.join(" ")).not.toContain("/other/root");
    const choice = options.find(option => option.textContent === optionName);
    expect(choice).toBeDefined();
    await act(async () => choice?.click());
    await click(h, tool, action);
  }
  expect(h.daemon.commands("memory_export")[0]).toMatchObject({ scope: { scope: "repository_role", id: "github.com/mengsig/aether", role: "agent" } });
  expect(h.daemon.commands("consolidate_memories")[0]).toMatchObject({ scope: { scope: "repository", id: "github.com/mengsig/aether" }, dry_run: true });
  vi.restoreAllMocks();
});
