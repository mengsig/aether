// @vitest-environment happy-dom

import { aether } from "@aether-ui/protocol";
import { act } from "react";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { invariant } from "../fleet/invariant.ts";
import { ControlsScreen } from "./ControlsScreen.tsx";
import { AGENT_ID, controlsScript, demoAgents } from "./fixture/demo.ts";
import type { ScriptOptions } from "./fixture/daemon.ts";
import { buttonNamed, click, connected, type, type Harness } from "./fixture/harness.test-util.tsx";

let current: Harness | null = null;
beforeEach(() => {
  let next = 0;
  vi.spyOn(URL, "createObjectURL").mockImplementation(() => `blob:controls-${++next}`);
  vi.spyOn(URL, "revokeObjectURL").mockImplementation(() => undefined);
});
afterEach(() => {
  current?.unmount();
  current = null;
  vi.restoreAllMocks();
});

async function open(tab: string, options: ScriptOptions = {}) {
  const h = current = await connected({ agents: demoAgents(), ...options, script: { ...controlsScript(), ...options.script } });
  const root = await h.mount(<ControlsScreen conn={h.conn} id={AGENT_ID} />);
  const button = [...root.querySelectorAll<HTMLButtonElement>('[role="tab"]')].find((item) => item.textContent === tab);
  invariant(button !== undefined, `the controls have a ${tab} tab`);
  invariant(!button.disabled, `the ${tab} tab is available`);
  await act(async () => button.click());
  await h.settle();
  await h.settle();
  return { h, root };
}

const sent = (h: Harness, cmd: string) => h.daemon.commands(cmd).map((command) => Object.fromEntries(
  Object.entries(command).filter(([key]) => key !== "req" && key !== "idempotency_key"),
));

function dialog(): HTMLElement {
  const element = document.querySelector<HTMLElement>("dialog[open]");
  invariant(element !== null, "the action asks for confirmation");
  return element;
}

function download(root: HTMLElement): HTMLAnchorElement {
  const link = root.querySelector<HTMLAnchorElement>("a[download]");
  invariant(link !== null, "the artifact offers a download");
  invariant(link.href.startsWith("blob:"), "the download uses a Blob object URL");
  return link;
}

function confirmDialog(): HTMLElement {
  const dialog = document.querySelector<HTMLElement>("dialog[open]");
  if (dialog === null) throw new Error("a confirm dialog is open");
  return dialog;
}

describe("approval rule parity", () => {
  test("the untouched form would approve everything, so it asks first and warns", async () => {
    const { h, root } = await open("Approvals");
    await click(h, root, "Add rule");
    expect(confirmDialog().textContent).toContain("It matches every tool call");
    await click(h, confirmDialog(), "Keep it");
    expect(sent(h, "add_approval_rule")).toEqual([]);
    expect(document.querySelector("dialog[open]")).toBeNull();
  });

  test("a rule can target the run from the store's agent record", async () => {
    const { h, root } = await open("Approvals");
    await type(h, root, "Scope", "run");
    await type(h, root, "Tool", "shell");
    await click(h, root, "Add rule");
    await click(h, confirmDialog(), "Add rule");
    expect(sent(h, "add_approval_rule")).toEqual([{
      cmd: "add_approval_rule", decision: "approved", scope: { scope: "run", id: demoAgents()[0]?.run }, match: { tool: "shell" },
    }]);
    expect(h.daemon.commands("list_approval_rules")).toHaveLength(2);
  });

  test("a blank-tool deny rule keeps its resource prefix, note and subtree scope", async () => {
    const { h, root } = await open("Approvals");
    expect(root.textContent).toContain("blank matches any tool");
    await type(h, root, "Tool", "   ");
    await type(h, root, "Resource prefix (optional)", "  /private/  ");
    await type(h, root, "Note (optional)", "  Keep credentials private  ");
    await type(h, root, "Decision", "denied");
    await type(h, root, "Scope", "subtree");
    await click(h, root, "Add rule");
    await click(h, confirmDialog(), "Add rule");
    expect(sent(h, "add_approval_rule")).toEqual([{
      cmd: "add_approval_rule", decision: "denied", scope: { scope: "subtree", root: AGENT_ID },
      match: { resource_prefix: "/private/" }, note: "Keep credentials private",
    }]);
    expect(root.querySelector('[role="alert"]')).toBeNull();
  });

  test("a rule with every matcher blank covers any tool call", async () => {
    const { h, root } = await open("Approvals");
    await type(h, root, "Decision", "denied");
    await click(h, root, "Add rule");
    await click(h, confirmDialog(), "Add rule");
    expect(sent(h, "add_approval_rule")).toEqual([{
      cmd: "add_approval_rule", decision: "denied", scope: { scope: "agent", id: AGENT_ID }, match: {},
    }]);
    expect(root.textContent).toContain("Rule added");
  });

  test("run scope is disabled with a reason when the agent has no run", async () => {
    const agents = demoAgents().map((agent) => ({ ...agent, run: "" }));
    const { root } = await open("Approvals", { agents });
    const option = root.querySelector<HTMLOptionElement>('option[value="run"]');
    expect(option?.disabled).toBe(true);
    expect(option?.textContent).toContain("this agent has no run");
    expect(buttonNamed(root, "Add rule").disabled).toBe(false);
  });

  test("stored rules show their resource prefix, note and run id", async () => {
    const run = demoAgents()[0]?.run;
    const { root } = await open("Approvals", { script: {
      list_approval_rules: () => ({ tag: "approval_rules", data: [{
        id: "rule_resource", decision: "denied", scope: { scope: "run", id: run },
        match: { resource_prefix: "/private/" }, note: "Keep credentials private",
      }] }),
    } });
    const rules = root.querySelector('[aria-label="Standing rules"]');
    expect(rules?.textContent).toContain("/private/");
    expect(rules?.textContent).toContain("Keep credentials private");
    expect(rules?.textContent).toContain(`Run ${run}`);
  });
});

describe("artifact download parity", () => {
  test("binary bytes download with the artifact filename and stay in core's store", async () => {
    const data = { base64: btoa(String.fromCharCode(0xff, 0xfe, 0, 1)), truncated: false };
    const { h, root } = await open("Records", { script: { read_artifact: () => ({ tag: "artifact_bytes", data }) } });
    await click(h, root, "Preview");
    const link = download(root);
    expect(link.textContent).toBe("Download");
    expect(link.download).toBe("release-notes.md");
    expect(root.textContent).toContain("A binary artifact");
    expect(h.app.store.get().reads[h.conn]?.["artifact_bytes:art_1"]?.value).toEqual(data);
    expect(sent(h, "read_artifact")).toEqual([{ cmd: "read_artifact", id: "art_1", max_bytes: 262_144 }]);
    const blob = vi.mocked(URL.createObjectURL).mock.calls[0]?.[0];
    invariant(blob instanceof Blob, "the download is built from a Blob");
    expect(new Uint8Array(await blob.arrayBuffer())).toEqual(new Uint8Array([0xff, 0xfe, 0, 1]));
  });

  test("text downloads are explicitly marked partial when truncated", async () => {
    const { h, root } = await open("Records", { script: {
      read_artifact: () => ({ tag: "artifact_bytes", data: { base64: btoa("partial notes"), truncated: true } }),
    } });
    await click(h, root, "Preview");
    expect(download(root).textContent).toBe("Download (partial)");
    expect(download(root).download).toBe("release-notes.md");
    expect(root.querySelector(".b3-pre")?.textContent).toBe("partial notes");
    expect(root.textContent).toContain("Partial content: only the received bytes will be downloaded");
  });

  test("replacing the stored reply revokes the previous URL and hiding revokes its replacement", async () => {
    const { h, root } = await open("Records");
    await click(h, root, "Preview");
    const first = download(root).href;
    h.daemon.script = { ...h.daemon.script, read_artifact: () => ({ tag: "artifact_bytes", data: { base64: btoa("new bytes"), truncated: false } }) };
    await act(async () => { await h.app.reread(h.conn, "artifact_bytes:art_1", { ...aether.readArtifact({ id: "art_1", max_bytes: 262_144 }) }, ["artifact_bytes"]); });
    await h.settle();
    const second = download(root).href;
    expect(second).not.toBe(first);
    expect(URL.revokeObjectURL).toHaveBeenCalledWith(first);
    expect(root.querySelector(".b3-pre")?.textContent).toBe("new bytes");
    await click(h, root, "Hide");
    expect(URL.revokeObjectURL).toHaveBeenCalledWith(second);
    expect(root.querySelector("a[download]")).toBeNull();
  });

  test("unmounting the screen revokes its download URL", async () => {
    const { h, root } = await open("Records");
    await click(h, root, "Preview");
    const url = download(root).href;
    h.unmount();
    current = null;
    expect(URL.revokeObjectURL).toHaveBeenCalledExactlyOnceWith(url);
    expect(URL.createObjectURL).toHaveBeenCalledTimes(1);
  });
});

describe("records parity", () => {
  test("an evicted artifact cannot be previewed, but its stored details still open", async () => {
    const evicted = { id: "art_1", name: "release-notes.md", kind: { kind: "file" }, content_type: "text/markdown", size_bytes: 28, summary: "Draft notes",
      created_at: "2026-11-04T09:00:35Z", created_by: { actor: "system" }, digest: "sha256:abc123", evicted_at: "2026-11-04T10:00:00Z" };
    const { h, root } = await open("Records", { script: {
      list_artifacts: () => ({ tag: "artifacts", data: [evicted] }),
      get_artifact: () => ({ tag: "artifact", data: evicted }),
    } });
    expect(buttonNamed(root, "Preview").disabled).toBe(true);
    await click(h, root, "Details");
    expect(sent(h, "get_artifact")).toEqual([{ cmd: "get_artifact", id: "art_1" }]);
    const details = root.querySelector('[aria-label="Artifact details"]');
    expect(details?.textContent).toContain("sha256:abc123");
    expect(details?.textContent).toContain("Content removed");
    expect(h.daemon.commands("read_artifact")).toHaveLength(0);
    await click(h, root, "Hide details");
    expect(root.querySelector('[aria-label="Artifact details"]')).toBeNull();
  });

  test("each listed run opens its own report, not only this agent's run", async () => {
    const other = "run_01J0B3CONTROLSOTHER0001";
    const listing = (run: string, objective: string) => ({ run, agents: 1, agents_live: 0, objective, started_at: "2026-11-01T09:00:00Z", updated_at: "2026-11-01T10:00:00Z",
      usage: { cost_usd: 1, cost_complete: true, input_tokens: 1, output_tokens: 1, cache_read_tokens: 0, cache_write_tokens: 0 } });
    const base = controlsScript().get_run_report;
    invariant(base !== undefined, "the demo answers run reports");
    const { h, root } = await open("Records", { script: {
      list_runs: () => ({ tag: "runs", data: [listing(demoAgents()[0]?.run ?? "", "Ship the 1.3 release"), listing(other, "An older run")] }),
      get_run_report: (command) => {
        const answer = base(command);
        invariant(answer !== "hold" && "data" in answer, "the demo report is data");
        const report = answer.data as { body: Record<string, unknown> };
        return command.run === other ? { tag: "run_report", data: { ...report, run: other, body: { ...report.body, objective: "An older run" } } } : answer;
      },
    } });
    const runs = root.querySelector<HTMLElement>('[aria-label="Runs"]');
    invariant(runs !== null, "the runs are listed");
    const before = h.daemon.commands("get_run_report").length;
    const reports = [...runs.querySelectorAll("button")].filter((button) => button.textContent === "Report");
    expect(reports).toHaveLength(2);
    await act(async () => reports[1]?.click());
    await h.settle();
    expect(sent(h, "get_run_report").slice(before)).toEqual([{ cmd: "get_run_report", run: other }]);
    expect(runs.textContent).toContain("An older run");
    expect(runs.querySelector(".b3-facts")?.textContent).toContain("An older run");
  });
});

describe("instructions parity", () => {
  test("clearing instructions asks first and sends an empty string only after confirmation", async () => {
    const { h, root } = await open("Model & limits");
    expect(buttonNamed(root, "Clear instructions").getAttribute("data-variant")).toBe("danger");
    await click(h, root, "Clear instructions");
    expect(h.daemon.commands("update_agent")).toHaveLength(0);
    await click(h, dialog(), "Keep it");
    expect(h.daemon.commands("update_agent")).toHaveLength(0);
    await click(h, root, "Clear instructions");
    await click(h, dialog(), "Clear instructions");
    expect(sent(h, "update_agent")).toEqual([{ cmd: "update_agent", id: AGENT_ID, patch: { instructions: "" } }]);
    expect(root.textContent).toContain("Instructions cleared");
  });

  test("there is no clear action when the agent has no instructions", async () => {
    const agents = demoAgents().map((agent) => ({ ...agent, config: { ...agent.config, instructions: "" } }));
    const { root } = await open("Model & limits", { agents });
    expect([...root.querySelectorAll("button")].some((button) => button.textContent === "Clear instructions")).toBe(false);
    expect(root.textContent).toContain("No standing instructions yet");
  });

  test("replacing instructions still rejects a blank draft", async () => {
    const { h, root } = await open("Model & limits");
    await type(h, root, "New instructions", "   ");
    await click(h, root, "Replace instructions");
    await click(h, dialog(), "Replace");
    expect(root.textContent).toContain("Write the new instructions first");
    expect(h.daemon.commands("update_agent")).toHaveLength(0);
  });
});
