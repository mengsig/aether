// @vitest-environment happy-dom
import { act } from "react";
import { afterEach, expect, test, vi } from "vitest";
import { FleetScreen } from "./FleetScreen.tsx";
import { fixtureAgents, fixtureConnection, seedFleet } from "./fixture/agents.ts";
import { harness } from "./fixture/mount.test-util.tsx";
import { must } from "./fixture/must.ts";

afterEach(() => { document.body.innerHTML = ""; });

async function openArchive(container: HTMLElement) {
  const select = must(container.querySelector<HTMLSelectElement>(".fleet-bar > label select") ?? undefined, "archive filter");
  await act(async () => { select.value = "archived"; select.dispatchEvent(new Event("change", { bubbles: true })); });
}

async function click(container: HTMLElement, label: string) {
  const button = must([...container.querySelectorAll("button")].find(item => item.textContent === label), label);
  await act(async () => button.click());
}

test("loads pages with empty and active-only results and retains cursors through live updates", async () => {
  const h = harness();
  const info = fixtureConnection();
  seedFleet(h.app.store, info, {});
  const agent = { ...must(fixtureAgents(1).agt_00000, "agent"), archived: true };
  const send = vi.spyOn(h.app, "send")
    .mockResolvedValueOnce({ reply: "agents_page", data: { agents: [], next_after: "empty" } })
    .mockResolvedValueOnce({ reply: "agents_page", data: { agents: [{ ...agent, archived: false }], next_after: "first" } })
    .mockResolvedValueOnce({ reply: "agents_page", data: { agents: [agent], next_after: "second" } })
    .mockResolvedValueOnce({ reply: "agents_page", data: { agents: [] } });
  const container = await h.mount(<FleetScreen />);
  await openArchive(container);
  expect(send).toHaveBeenCalledTimes(1);
  expect(container.textContent).not.toContain("No archived agents");
  await click(container, "Load more");
  expect(send).toHaveBeenNthCalledWith(2, info.id, expect.objectContaining({ after: "empty" }));
  expect(container.textContent).not.toContain("No archived agents");
  await click(container, "Load more");
  await h.frame();
  expect(send).toHaveBeenNthCalledWith(3, info.id, expect.objectContaining({ after: "first" }));
  expect(container.querySelector('[data-agent="agt_00000"]')).not.toBeNull();
  h.app.store.updateAether(info.id, state => ({ ...state, agents: { ...state.agents, [agent.id]: { ...agent, name: "updated" } } }));
  await h.frame();
  expect(send).toHaveBeenCalledTimes(3);
  await click(container, "Load more");
  expect(send).toHaveBeenNthCalledWith(4, info.id, expect.objectContaining({ after: "second" }));
  expect(container.textContent).not.toContain("Load more");
});

test("retries a failed daemon without losing the other daemon's cursor", async () => {
  const h = harness();
  const first = fixtureConnection("aether:first");
  const second = fixtureConnection("aether:second");
  seedFleet(h.app.store, first, {});
  seedFleet(h.app.store, second, {});
  let failed = false;
  const send = vi.spyOn(h.app, "send").mockImplementation(async (id, command) => {
    if (id === second.id && !failed) { failed = true; throw new Error("offline"); }
    return { reply: "agents_page", data: { agents: [], ...(id === first.id && !("after" in command) ? { next_after: "cursor" } : {}) } };
  });
  const container = await h.mount(<FleetScreen />);
  await openArchive(container);
  expect(container.textContent).toContain("offline");
  await click(container, "Retry");
  expect(send.mock.calls.filter(call => call[0] === first.id)).toHaveLength(1);
  await click(container, "Load more");
  expect(send).toHaveBeenCalledWith(first.id, expect.objectContaining({ after: "cursor" }));
});

test("restarts the first page when a resync snapshot replaces cached agents", async () => {
  const h = harness();
  const info = fixtureConnection();
  seedFleet(h.app.store, info, {});
  const agent = { ...must(fixtureAgents(1).agt_00000, "agent"), archived: true };
  const send = vi.spyOn(h.app, "send").mockResolvedValue({ reply: "agents_page", data: { agents: [agent], next_after: "cursor" } });
  const container = await h.mount(<FleetScreen />);
  await openArchive(container);
  await h.frame();
  h.app.store.updateAether(info.id, state => ({ ...state, agents: {}, roots: [], lastSeq: 42 }));
  await h.frame();
  await h.frame();
  expect(send).toHaveBeenCalledTimes(2);
  expect(send.mock.calls[1]?.[1]).not.toHaveProperty("after");
  expect(container.querySelector('[data-agent="agt_00000"]')).not.toBeNull();
});

test("retries a failed later page at the same cursor", async () => {
  const h = harness();
  const info = fixtureConnection();
  seedFleet(h.app.store, info, {});
  const agent = { ...must(fixtureAgents(1).agt_00000, "agent"), archived: true };
  const send = vi.spyOn(h.app, "send")
    .mockResolvedValueOnce({ reply: "agents_page", data: { agents: [agent], next_after: "cursor" } })
    .mockRejectedValueOnce(new Error("page failed"))
    .mockResolvedValueOnce({ reply: "agents_page", data: { agents: [] } });
  const container = await h.mount(<FleetScreen />);
  await openArchive(container);
  await h.frame();
  await click(container, "Load more");
  expect(container.textContent).toContain("page failed");
  expect(container.querySelector('[data-agent="agt_00000"]')).not.toBeNull();
  await click(container, "Retry");
  expect(send).toHaveBeenNthCalledWith(3, info.id, expect.objectContaining({ after: "cursor" }));
  expect(container.textContent).not.toContain("page failed");
});

test("a resync cancels an in-flight later page and ignores its stale response", async () => {
  const h = harness();
  const info = fixtureConnection();
  seedFleet(h.app.store, info, {});
  const agent = { ...must(fixtureAgents(2).agt_00000, "agent"), archived: true };
  const stale = { ...must(fixtureAgents(2).agt_00001, "agent"), archived: true };
  let resolvePage: (reply: { reply: string; data: { agents: typeof stale[] } }) => void = () => { throw new Error("request not started"); };
  const send = vi.spyOn(h.app, "send")
    .mockResolvedValueOnce({ reply: "agents_page", data: { agents: [agent], next_after: "cursor" } })
    .mockImplementationOnce(() => new Promise(resolve => { resolvePage = resolve; }))
    .mockResolvedValueOnce({ reply: "agents_page", data: { agents: [agent] } });
  const container = await h.mount(<FleetScreen />);
  await openArchive(container);
  await h.frame();
  await click(container, "Load more");
  h.app.store.updateAether(info.id, state => ({ ...state, agents: {}, roots: [], lastSeq: 42 }));
  await h.frame();
  await act(async () => resolvePage({ reply: "agents_page", data: { agents: [stale] } }));
  await h.frame();
  expect(send).toHaveBeenCalledTimes(3);
  expect(h.app.store.get().aether[info.id]?.agents[stale.id]).toBeUndefined();
  expect(container.querySelector('[data-agent="agt_00000"]')).not.toBeNull();
});

test("a live removal does not restart archive pagination", async () => {
  const h = harness();
  const info = fixtureConnection();
  seedFleet(h.app.store, info, {});
  const agent = { ...must(fixtureAgents(1).agt_00000, "agent"), archived: true };
  const send = vi.spyOn(h.app, "send").mockResolvedValueOnce({ reply: "agents_page", data: { agents: [agent], next_after: "cursor" } })
    .mockResolvedValueOnce({ reply: "agents_page", data: { agents: [] } });
  const container = await h.mount(<FleetScreen />);
  await openArchive(container);
  await h.frame();
  h.app.store.updateAether(info.id, state => ({ ...state, agents: {} }));
  await h.frame();
  expect(send).toHaveBeenCalledTimes(1);
  const newRoot = { ...must(fixtureAgents(2).agt_00001, "agent"), parent: null };
  h.app.store.updateAether(info.id, state => ({ ...state, roots: [newRoot.id], agents: { ...state.agents, [newRoot.id]: newRoot } }));
  await h.frame();
  expect(send).toHaveBeenCalledTimes(1);
  await click(container, "Load more");
  expect(send).toHaveBeenNthCalledWith(2, info.id, expect.objectContaining({ after: "cursor" }));
});

test("waits for a disconnected daemon before reporting an empty archive", async () => {
  const h = harness();
  const info = { ...fixtureConnection(), state: "backoff" as const };
  seedFleet(h.app.store, info, {});
  const send = vi.spyOn(h.app, "send");
  const container = await h.mount(<FleetScreen />);
  await openArchive(container);
  expect(container.textContent).toContain("Waiting for an Aether connection");
  expect(container.textContent).not.toContain("No archived agents");
  expect(send).not.toHaveBeenCalled();
});
