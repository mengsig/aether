// @vitest-environment happy-dom
import { afterEach, expect, test, vi } from "vitest";
import { connected, CONN, stubCanvas, type Harness } from "./fixture/harness.test-util.tsx";
import { LESSON, scenario } from "./fixture/scenario.ts";
import SessionsRoute from "./SessionsScreen.tsx";

let h: Harness | undefined;
afterEach(() => { h?.unmount(); h = undefined; });
test("renders all 201 sessions in newest-first order", async () => {
  stubCanvas();
  h = await connected(scenario(true), vi);
  const rows = Array.from({ length: 201 }, (_, n) => ({ ...LESSON, id: `ns_${n}`, goal: `Goal ${n}`, updated_at: n === 200 ? "2099-01-01T00:00:00Z" : LESSON.updated_at }));
  vi.spyOn(h.app.manager, "send").mockResolvedValueOnce({ reply: "noesis_sessions", data: { sessions: rows.slice(0, 200), next_after: "next" } })
    .mockResolvedValueOnce({ reply: "noesis_sessions", data: { sessions: rows.slice(200) } });
  const root = await h.mount(<SessionsRoute />);
  await h.settle();
  expect(root.querySelectorAll(".learn-card-goal")).toHaveLength(201);
  expect(root.querySelector(".learn-card-goal")?.textContent).toBe("Goal 200");
  expect(h.app.store.get().aether[CONN]?.learn.reads["all-sessions"]).toBeDefined();
});

test("shows a refresh error alongside the last completed list", async () => {
  stubCanvas();
  h = await connected(scenario(true), vi);
  const send = vi.spyOn(h.app.manager, "send").mockResolvedValueOnce({ reply: "noesis_sessions", data: { sessions: [LESSON] } });
  await h.app.manager.readNoesis(CONN, { kind: "all-sessions" });
  await h.settle();
  send.mockResolvedValueOnce({ reply: "noesis_sessions", data: { sessions: [LESSON], next_after: "next" } })
    .mockRejectedValueOnce(new Error("later page failed"));
  const root = await h.mount(<SessionsRoute />);
  await h.settle();
  expect(root.textContent).toContain("later page failed");
  expect(root.textContent).toContain(LESSON.goal);
});
