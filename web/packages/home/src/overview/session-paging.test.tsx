// @vitest-environment happy-dom
import type { aether } from "@aether-ui/protocol";
import { expect, test } from "vitest";
import { deriveHome } from "./derive.ts";
import { LearnTile } from "./archon-tiles.tsx";
import { selectHomeSources } from "./sources.ts";
import { harness } from "./fixture/harness.test-util.tsx";
import { AETHER_CONN, aetherInfo, NOW, seedWorld, SESSIONS } from "./fixture/world.ts";

test("overview counts only completed membership and finds its newest session", async () => {
  const h = harness();
  seedWorld(h.app.store, [aetherInfo()], true);
  const original = SESSIONS.sessions[0];
  expect(original).toBeDefined();
  if (!original) throw new Error("fixture needs a session");
  const session = original as aether.NoesisSession;
  const rows = Array.from({ length: 201 }, (_, n) => ({ ...session, id: `ns_${n}`, goal: `Goal ${n}`, updated_at: n === 200 ? "2099-01-01T00:00:00Z" : original.updated_at }));
  h.app.store.updateAether(AETHER_CONN, (state) => ({ ...state, learn: { ...state.learn,
    sessions: Object.fromEntries([...rows, { ...session, id: "stale" }].map((row) => [row.id, row])),
    reads: { ...state.learn.reads, "all-sessions": { ticket: 1, value: { ids: rows.map((row) => row.id) } } },
  } }));
  await h.frame();
  const view = deriveHome(selectHomeSources(0)(h.app.store.get()), NOW, 0, {});
  expect(view.learn.sessions).toBe(201);
  expect(view.learn.session?.goal).toBe("Goal 200");
});

test("an incomplete list is loading even when individual sessions are cached", async () => {
  const h = harness();
  seedWorld(h.app.store, [aetherInfo()], true);
  h.app.store.updateAether(AETHER_CONN, (state) => ({ ...state, learn: { ...state.learn,
    sessions: Object.fromEntries(SESSIONS.sessions.map((row) => [row.id, row as aether.NoesisSession])),
  } }));
  await h.frame();
  const view = deriveHome(selectHomeSources(0)(h.app.store.get()), NOW, 0, {});
  expect(view.learn.state).toBe("loading");
  expect(view.learn.session).toBeNull();
});

test("Learn distinguishes session loading and failure from mastery", async () => {
  const h = harness();
  seedWorld(h.app.store, [aetherInfo()], true);
  await h.frame();
  const sources = selectHomeSources(0)(h.app.store.get());
  const loading = deriveHome(sources, NOW, 0, {});
  const container = await h.mount(<LearnTile view={loading} />);
  expect(container.querySelector('[aria-busy="true"]')).not.toBeNull();
  expect(container.textContent).not.toContain("No session yet");
  await h.unmount();
  const failed = deriveHome(sources, NOW, 0, { [`${AETHER_CONN} noesis:all-sessions`]: "Connection lost" });
  expect(failed.learn.mastery.state).not.toBe("problem");
  const error = await h.mount(<LearnTile view={failed} />);
  expect(error.textContent).toContain("Sessions could not be read");
  expect(error.textContent).toContain("Connection lost");
  expect(error.textContent).not.toContain("Mastery could not be read");
  expect(error.querySelector('a[href="#/learn"]')).not.toBeNull();
  await h.unmount();
});
