// @vitest-environment happy-dom
import { drafts } from "@aether-ui/core";
// The shared control: refused with the reason shown, host-only marked, confirmed before sending,
// and a lost reply reported as unconfirmed and left in core's outbox for the recovery review.

import { selectOutbox } from "@aether-ui/core";
import { beforeEach, afterEach, describe, expect, test } from "vitest";
import { connected, click, buttonNamed, type Harness } from "./fixture/harness.test-util.tsx";
import { Action, RunNote, useRun } from "./kit.tsx";

let current: Harness | null = null;
afterEach(() => { current?.unmount(); current = null; });

function Probe({ conn, command, confirm }: { readonly conn: string; readonly command: Record<string, unknown>; readonly confirm?: boolean }) {
  const run = useRun(conn);
  return (
    <div>
      <Action conn={conn} command={command} label="Go" onRun={() => run.send(command)}
        {...(confirm ? { confirm: { title: "Sure?", body: "It cannot be undone.", confirmLabel: "Yes, go" } } : {})} />
      <RunNote status={run.status} />
    </div>
  );
}

describe("Action", () => {
  test("an observer sees the control disabled with the reason beside it", async () => {
    const h = current = await connected({ role: "observer" });
    const root = await h.mount(<Probe conn={h.conn} command={{ cmd: "cancel", agent: "agt_1" }} />);
    expect(buttonNamed(root, "Go").disabled).toBe(true);
    expect(root.querySelector(".b3-reason")?.textContent).toBe("Observer devices cannot change daemon state");
  });

  test("a host-only command is marked and refused to a remote operator", async () => {
    const h = current = await connected({ role: "operator" });
    const root = await h.mount(<Probe conn={h.conn} command={{ cmd: "accept_memory", id: "mem_1" }} />);
    expect(root.textContent).toContain("Host only");
    expect(root.querySelector(".b3-reason")?.textContent).toBe("Only the daemon's own host can do this");
  });

  test("asks before sending, and a lost reply is reported unconfirmed and kept for review", async () => {
    const h = current = await connected({ role: "operator", script: { cancel: () => "hold" } });
    const root = await h.mount(<Probe conn={h.conn} command={{ cmd: "cancel", agent: "agt_1" }} confirm />);
    await click(h, root, "Go");
    expect(h.daemon.commands("cancel")).toHaveLength(0);
    await click(h, document.body, "Yes, go");
    expect(h.daemon.commands("cancel")).toHaveLength(1);
    h.daemon.drop();
    await h.settle();
    expect(root.querySelector(".b3-note")?.getAttribute("data-state")).toBe("unconfirmed");
    expect(h.app.store.get().aether[h.conn]?.outbox).toEqual([expect.objectContaining({ state: "unknown", command: expect.objectContaining({ cmd: "cancel" }) })]);
    expect(selectOutbox(h.conn)(h.app.store.get())).toHaveLength(1);
  });
});

beforeEach(() => { for (const key of drafts.keys()) drafts.clear(key); });
