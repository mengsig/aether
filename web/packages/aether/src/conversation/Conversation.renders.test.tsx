// @vitest-environment happy-dom
import { createElement, memo, type ComponentProps } from "react";
import { afterEach, expect, test, vi } from "vitest";
import { Conversation } from "./Conversation.tsx";
import { connected, type Harness } from "./fixture/harness.test-util.tsx";
import { FAUX_AGENT, FAUX_PARENT, fauxAgent, text } from "./fixture/transcript.ts";

const renders = vi.hoisted(() => new Map<string, number>());
vi.mock("./Message.tsx", async (importOriginal) => {
  const original = await importOriginal<typeof import("./Message.tsx")>();
  const MessageView = memo((props: ComponentProps<typeof original.MessageView>) => {
    const key = props.view.row.key;
    renders.set(key, (renders.get(key) ?? 0) + 1);
    return createElement(original.MessageView, props);
  });
  return { ...original, MessageView };
});

let current: Harness | null = null;
afterEach(() => { current?.unmount(); current = null; renders.clear(); });

test("fleet updates leave message rows alone while card names stay current", async () => {
  const h = current = await connected({ instance: "faux-1", agents: [fauxAgent(FAUX_PARENT), fauxAgent()] });
  h.daemon.seed(FAUX_AGENT, [
    { message: text("user", "Your task:\nShip it") },
    { message: text("assistant", "Working") },
  ]);
  const container = await h.mount(<Conversation conn={h.conn} agent={FAUX_AGENT} />);
  await h.settle();
  expect(container.querySelector(".conv-card-title")?.textContent).toBe("Task from planner");
  expect(renders.size).toBe(2);
  renders.clear();
  h.app.store.updateAether(h.conn, (state) => ({ ...state, agents: { ...state.agents, [FAUX_PARENT]: fauxAgent(FAUX_PARENT, { name: "renamed" }) } }));
  await h.settle();
  expect(container.querySelector(".conv-card-title")?.textContent).toBe("Task from renamed");
  expect([...renders.entries()]).toEqual([]);
  h.daemon.delta(FAUX_AGENT, "inv_live", 1, { delta: "text", text: "hello" });
  await h.settle();
  expect(container.querySelector(".conv-msg[data-live]")?.textContent).toContain("hello");
  expect([...renders.keys()]).toEqual(["s:inv_live"]);
});
