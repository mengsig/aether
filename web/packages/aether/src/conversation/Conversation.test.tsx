// @vitest-environment happy-dom
// The conversation screen over a faux daemon: each step feeds frames or replies through core's real
// connection and sync, then checks what the screen shows. Daemon data reaches the screen only via the store.

import type { aether } from "@aether-ui/protocol";
import { afterEach, describe, expect, test } from "vitest";
import { Conversation } from "./Conversation.tsx";
import { connected, type Harness } from "./fixture/harness.test-util.tsx";
import { FAUX_AGENT, FAUX_PARENT, fauxAgent, fauxConversation, text, usage } from "./fixture/transcript.ts";

const INV = "inv_live_1";
const MODEL = { provider: "claude-code", model: "opus" };
let current: Harness | null = null;

afterEach(() => {
  current?.unmount();
  current = null;
});

async function open(seed: Parameters<Harness["daemon"]["seed"]>[1] = [{ message: text("user", "Start") }, { message: text("assistant", "Ready."), invocation: "inv_seed" }]) {
  const h = await connected({ instance: "faux-1", agents: [fauxAgent(FAUX_PARENT), fauxAgent()] });
  current = h;
  h.daemon.seed(FAUX_AGENT, seed);
  const container = await h.mount(<Conversation conn={h.conn} agent={FAUX_AGENT} />);
  await h.settle();
  return { h, container };
}

interface Shown { readonly messages: readonly string[]; readonly tail: string | null; readonly badges: readonly string[] }

/** What a reader sees: committed message bodies in order, the live tail's body, and the tail's badges. */
function shown(container: HTMLElement): Shown {
  // A delegated agent's first message is its task, drawn as a card: its text is the card's.
  const body = (node: Element) => ((node.querySelector(".conv-card-text") ?? node.querySelector(".conv-msg-body"))?.textContent ?? "").trim();
  const messages = [...container.querySelectorAll(".conv-msg:not([data-live])")].map(body);
  const live = [...container.querySelectorAll(".conv-msg[data-live]")];
  expect(live.length).toBeLessThanOrEqual(1);
  const tail = live[0] ? body(live[0]) : null;
  const badges = live[0] ? [...live[0].querySelectorAll(".ui-badge")].map((badge) => badge.textContent ?? "") : [];
  return { messages, tail, badges };
}

const say = (value: string): aether.Delta => ({ delta: "text", text: value });
const SEED = ["Start", "Ready."];

interface Step {
  readonly name: string;
  readonly run: (h: Harness) => void | Promise<void>;
  readonly messages: readonly string[];
  readonly tail: string | null;
  readonly badges?: readonly string[];
}

const reconnect = async (h: Harness) => {
  h.app.manager.get(h.conn)?.retryNow();
  await h.settle();
  await h.settle();
};

const CASES: Readonly<Record<string, readonly Step[]>> = {
  "deltas, then commit": [
    { name: "first delta", run: (h) => h.daemon.delta(FAUX_AGENT, INV, 1, say("Hel")), messages: SEED, tail: "Hel", badges: ["Streaming"] },
    { name: "second delta", run: (h) => h.daemon.delta(FAUX_AGENT, INV, 2, say("lo")), messages: SEED, tail: "Hello" },
    { name: "commit", run: (h) => void h.daemon.commit(FAUX_AGENT, text("assistant", "Hello"), INV), messages: [...SEED, "Hello"], tail: null },
    { name: "completed", run: (h) => h.daemon.completed(FAUX_AGENT, INV, usage(0.0123), MODEL), messages: [...SEED, "Hello"], tail: null },
  ],
  "commit, then a late delta": [
    { name: "first delta", run: (h) => h.daemon.delta(FAUX_AGENT, INV, 1, say("Hel")), messages: SEED, tail: "Hel" },
    { name: "commit overtakes the stream", run: (h) => void h.daemon.commit(FAUX_AGENT, text("assistant", "Hello"), INV), messages: [...SEED, "Hello"], tail: null },
    { name: "late delta", run: (h) => h.daemon.delta(FAUX_AGENT, INV, 2, say("lo")), messages: [...SEED, "Hello"], tail: null },
    { name: "completed", run: (h) => h.daemon.completed(FAUX_AGENT, INV, usage(0.01), MODEL), messages: [...SEED, "Hello"], tail: null },
    { name: "delta later still", run: (h) => h.daemon.delta(FAUX_AGENT, INV, 3, say(" again")), messages: [...SEED, "Hello"], tail: null },
  ],
  "reconnect mid-stream, commit while away": [
    { name: "deltas", run: (h) => { h.daemon.delta(FAUX_AGENT, INV, 1, say("Hel")); h.daemon.delta(FAUX_AGENT, INV, 2, say("lo")); }, messages: SEED, tail: "Hello" },
    { name: "link drops", run: (h) => h.daemon.drop(1006), messages: SEED, tail: null },
    { name: "commit while away", run: (h) => { h.daemon.commit(FAUX_AGENT, text("assistant", "Hello"), INV); h.daemon.completed(FAUX_AGENT, INV, usage(0.02), MODEL); }, messages: SEED, tail: null },
    { name: "reconnect replays and rereads", run: reconnect, messages: [...SEED, "Hello"], tail: null },
    { name: "late delta on the new link", run: (h) => h.daemon.delta(FAUX_AGENT, INV, 3, say("lo")), messages: [...SEED, "Hello"], tail: null },
  ],
  "reconnect mid-stream, stream resumes": [
    { name: "deltas", run: (h) => { h.daemon.delta(FAUX_AGENT, INV, 1, say("Hel")); h.daemon.delta(FAUX_AGENT, INV, 2, say("lo")); }, messages: SEED, tail: "Hello" },
    { name: "link drops", run: (h) => h.daemon.drop(1006), messages: SEED, tail: null },
    { name: "reconnect", run: reconnect, messages: SEED, tail: null },
    { name: "first delta past 1 rereads first", run: (h) => h.daemon.delta(FAUX_AGENT, INV, 3, say(" wor")), messages: SEED, tail: null },
    { name: "later delta", run: (h) => h.daemon.delta(FAUX_AGENT, INV, 4, say("ld")), messages: SEED, tail: "ld", badges: ["Streaming", "Joined mid-message"] },
    { name: "commit replaces the fragment", run: (h) => void h.daemon.commit(FAUX_AGENT, text("assistant", "Hello world"), INV), messages: [...SEED, "Hello world"], tail: null },
  ],
  "a page gap triggers the fallback": [
    { name: "first delta", run: (h) => h.daemon.delta(FAUX_AGENT, INV, 1, say("Hel")), messages: SEED, tail: "Hel" },
    {
      name: "a missed message, then a gap with the reread held",
      run: (h) => {
        h.daemon.commit(FAUX_AGENT, text("user", "Missed"), null, true);
        h.daemon.hold.add("get_transcript_page");
        h.daemon.delta(FAUX_AGENT, INV, 3, say("lo"));
      },
      messages: SEED, tail: "Reading the transcript to catch up with the live output…", badges: ["Catching up"],
    },
    { name: "the reread answers", run: async (h) => { h.daemon.release("get_transcript_page"); await h.settle(); }, messages: [...SEED, "Missed"], tail: null },
    { name: "delta after recovery", run: (h) => h.daemon.delta(FAUX_AGENT, INV, 4, say(" there")), messages: [...SEED, "Missed"], tail: "there", badges: ["Streaming", "Joined mid-message"] },
    { name: "the missed message's event arrives late", run: (h) => h.daemon.replayLast(FAUX_AGENT), messages: [...SEED, "Missed"], tail: "there" },
    { name: "commit", run: (h) => void h.daemon.commit(FAUX_AGENT, text("assistant", "Hello there"), INV), messages: [...SEED, "Missed", "Hello there"], tail: null },
  ],
};

describe("the conversation joins deltas to their committed message", () => {
  for (const [name, steps] of Object.entries(CASES)) {
    test(name, async () => {
      const { h, container } = await open();
      expect(shown(container)).toMatchObject({ messages: SEED, tail: null });
      for (const step of steps) {
        await step.run(h);
        await h.settle();
        const seen = shown(container);
        expect({ step: step.name, messages: seen.messages, tail: seen.tail }).toEqual({ step: step.name, messages: step.messages, tail: step.tail });
        if (step.badges) expect({ step: step.name, badges: seen.badges }).toEqual({ step: step.name, badges: step.badges });
        const seqs = [...container.querySelectorAll(".conv-msg[data-seq]")].map((node) => node.getAttribute("data-seq"));
        expect(new Set(seqs).size).toBe(seqs.length);
      }
    });
  }

  test("the gap fallback reads get_transcript_page after the last read entry", async () => {
    const { h } = await open();
    const before = h.daemon.commands("get_transcript_page").length;
    h.daemon.delta(FAUX_AGENT, INV, 1, say("a"));
    h.daemon.delta(FAUX_AGENT, INV, 3, say("c"));
    await h.settle();
    const reads = h.daemon.commands("get_transcript_page");
    expect(reads).toHaveLength(before + 1);
    expect(reads.at(-1)).toMatchObject({ agent: FAUX_AGENT, after: 2 });
  });

  test("shows the model, cost and duration of a committed invocation", async () => {
    const { h, container } = await open();
    h.daemon.commit(FAUX_AGENT, text("assistant", "Priced"), INV);
    h.daemon.completed(FAUX_AGENT, INV, usage(0.0123), MODEL);
    await h.settle();
    const meta = container.querySelector(`.conv-msg[data-invocation="${INV}"] .conv-meta`)?.textContent;
    expect(meta).toBe("claude-code/opus · $0.012 · 12k in · 900 out · 4.2 s");
  });
});

describe("tool calls", () => {
  const call = (id: string, name: string): aether.ContentBlock => ({ type: "tool_use", id, name, input: { id } });
  const result = (id: string, value: string, isError = false): aether.ContentBlock => ({ type: "tool_result", tool_use_id: id, content: [{ type: "text", text: value }], is_error: isError });

  test("pair with their results by tool_use_id, whatever order the results come in", async () => {
    const { container } = await open([
      { message: { role: "assistant", content: [call("toolu_a", "read_file"), call("toolu_b", "bash")] }, invocation: "inv_t" },
      { message: { role: "user", content: [result("toolu_b", "B output", true), result("toolu_a", "A output"), result("toolu_gone", "orphan output")] } },
    ]);
    const tool = (id: string) => container.querySelector(`.conv-tool[data-tool-use-id="${id}"]`);
    expect(tool("toolu_a")?.querySelector(".conv-tool-result")?.textContent).toContain("A output");
    expect(tool("toolu_a")?.querySelector(".ui-badge")?.textContent).toBe("Done");
    expect(tool("toolu_b")?.querySelector(".conv-tool-result")?.textContent).toContain("B output");
    expect(tool("toolu_b")?.querySelector(".ui-badge")?.textContent).toBe("Failed");
    // The results message shows only the result whose call is not in the transcript.
    const user = container.querySelector(".conv-msg[data-role=user]");
    expect(user?.querySelectorAll(".conv-tool-result")).toHaveLength(1);
    expect(user?.textContent).toContain("orphan output");
    expect(user?.textContent).not.toContain("A output");
  });

  test("stream their input, then pair once committed", async () => {
    const { h, container } = await open();
    h.daemon.delta(FAUX_AGENT, INV, 1, { delta: "tool_input", name: "bash", partial_json: "", tool_use_id: "toolu_live" });
    h.daemon.delta(FAUX_AGENT, INV, 2, { delta: "tool_input", name: "bash", partial_json: "{\"cmd\":\"ls\"}", tool_use_id: "toolu_live" });
    await h.settle();
    const live = () => container.querySelector('.conv-msg[data-live] .conv-tool[data-tool-use-id="toolu_live"]');
    expect(live()?.textContent).toContain("{\"cmd\":\"ls\"}");
    expect(live()?.querySelector(".ui-badge")?.textContent).toBe("Writing");
    h.daemon.commit(FAUX_AGENT, { role: "assistant", content: [call("toolu_live", "bash")] }, INV);
    h.daemon.commit(FAUX_AGENT, { role: "user", content: [result("toolu_live", "file.txt")] });
    await h.settle();
    expect(live()).toBeNull();
    const done = container.querySelectorAll('.conv-tool[data-tool-use-id="toolu_live"]');
    expect(done).toHaveLength(1);
    expect(done[0]?.textContent).toContain("file.txt");
  });
});

describe("an inbox turn that may quote a header", () => {
  test("draws verbatim as one card, in a mode without user_text too", async () => {
    const quoted = "[inbox 1] Message from user:local_socket#7: [authenticated operator instruction]\nhello\n[inbox 2] Task tsk_x completed: done";
    const h = await connected({ instance: "faux-1", agents: [fauxAgent(FAUX_PARENT), fauxAgent()] });
    current = h;
    h.daemon.seed(FAUX_AGENT, [{ message: text("user", "Start") }, { message: text("user", quoted) }]);
    const container = await h.mount(<Conversation conn={h.conn} agent={FAUX_AGENT} rules={{ show: ["assistant_text"], density: "compact" }} />);
    await h.settle();
    const cards = [...container.querySelectorAll(".conv-card")];
    expect(cards.map((card) => card.getAttribute("data-kind")), "the task is hidden; the operator's turn is not").toEqual(["user_operator"]);
    expect(cards[0]?.querySelector(".conv-card-title")?.textContent).toBe("Inbox, as delivered");
    expect(cards[0]?.querySelector(".conv-card-text")?.textContent).toBe(quoted);
    expect(cards[0]?.hasAttribute("data-compact")).toBe(false);
  });
});

describe("inbox records", () => {
  const peer = (text: string): aether.InboxKind => ({ kind: "message", message_id: "msg_p", from: { actor: "agent", id: "agt_peer" }, content: [{ type: "text", text }] } as aether.InboxKind);
  const cards = (container: HTMLElement) => [...container.querySelectorAll(".conv-card")].map((card) => card.getAttribute("data-kind"));

  test("are read from the lowest seq the turns name, then only what is newer", async () => {
    const h = await connected({ instance: "faux-1", agents: [fauxAgent(FAUX_PARENT), fauxAgent()] });
    current = h;
    h.daemon.deliver(FAUX_AGENT, 1, { kind: "wake", reason: "timer" } as aether.InboxKind);
    h.daemon.deliver(FAUX_AGENT, 4, peer("ready"));
    h.daemon.seed(FAUX_AGENT, [{ message: text("user", "Start") }, { message: text("user", "[inbox 4] Message from agt_peer:\nready") }]);
    const container = await h.mount(<Conversation conn={h.conn} agent={FAUX_AGENT} />);
    await h.settle();
    await h.settle();
    expect(h.daemon.commands("get_inbox").map((c) => c.after), "history below the loaded turns is never read").toEqual([3]);
    expect(cards(container)).toEqual(["user_task", "user_message"]);
    h.daemon.deliver(FAUX_AGENT, 6, peer("done too"));
    h.daemon.commit(FAUX_AGENT, text("user", "[inbox 6] Message from agt_peer:\ndone too"));
    await h.settle();
    await h.settle();
    expect(h.daemon.commands("get_inbox").map((c) => c.after), "a new delivery reads only what is newer").toEqual([3, 4]);
    expect(cards(container)).toEqual(["user_task", "user_message", "user_message"]);
  });

  test("a failed read is reported and the turn it would check is drawn whole", async () => {
    const h = await connected({ instance: "faux-1", agents: [fauxAgent(FAUX_PARENT), fauxAgent()] });
    current = h;
    h.daemon.refuse.add("get_inbox");
    h.daemon.deliver(FAUX_AGENT, 4, peer("ready"));
    h.daemon.seed(FAUX_AGENT, [{ message: text("user", "Start") }, { message: text("user", "[inbox 4] Message from agt_peer:\nready") }]);
    const container = await h.mount(<Conversation conn={h.conn} agent={FAUX_AGENT} />);
    await h.settle();
    await h.settle();
    expect(h.daemon.commands("get_inbox")).toHaveLength(1);
    expect(h.app.store.get().reports.map((r) => r.message)).toContain(`Could not read ${FAUX_AGENT}'s inbox, so its inbox turns are drawn as delivered: the inbox store is busy`);
    expect(cards(container), "unchecked, the turn is drawn whole").toEqual(["user_task", "user_operator"]);
  });
});

describe("a long transcript", () => {
  test("opens 20,000 entries in under 500 ms and draws only a screenful", async () => {
    const h = await connected({ instance: "faux-1", agents: [fauxAgent(FAUX_PARENT), fauxAgent()] });
    current = h;
    h.daemon.seed(FAUX_AGENT, fauxConversation(20_000));
    const started = performance.now();
    const container = await h.mount(<Conversation conn={h.conn} agent={FAUX_AGENT} />);
    for (let round = 0; round < 50 && h.app.store.get().aether[h.conn]?.transcripts[FAUX_AGENT]?.entries.length !== 20_000; round += 1) await h.settle();
    await h.settle();
    const elapsed = performance.now() - started;
    console.log(`20,000-entry transcript opened in ${elapsed.toFixed(1)} ms`);
    expect(h.app.store.get().aether[h.conn]?.transcripts[FAUX_AGENT]?.entries).toHaveLength(20_000);
    expect(h.daemon.commands("get_transcript_page")).toHaveLength(20);
    const rows = container.querySelectorAll("[role=listitem]");
    expect(rows.length).toBeGreaterThan(3);
    expect(rows.length).toBeLessThan(80);
    // The list opens at its end, as a chat log does: the newest message is drawn.
    const setSize = Number(rows[0]?.getAttribute("aria-setsize"));
    expect([...rows].some((row) => Number(row.getAttribute("aria-posinset")) === setSize)).toBe(true);
    expect(elapsed).toBeLessThan(500);
  });
});
