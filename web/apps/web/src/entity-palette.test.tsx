// @vitest-environment happy-dom
import { AppProvider, createApp, Registry } from "@aether-ui/core";
import type { aether } from "@aether-ui/protocol";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { expect, test, vi } from "vitest";
import { EntityPalette } from "./entity-palette.tsx";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

test("the palette includes current entities and updates while open", async () => {
  const app = createApp({ kind: "browser", storage: { get: async () => undefined, set: async () => {} } }, { scheduler: (flush) => flush() });
  const registry = new Registry();
  registry.registerRoutes([{ path: "#/agents/:conn/:agent", owner: "test", load: async () => ({ default: () => null }) }]);
  const agent = (name: string) => ({ id: "a1", name, role: "builder", archived: false, status: { state: "running" } }) as aether.Agent;
  app.store.updateAether("local", (state) => ({ ...state, agents: { a1: agent("First agent") } }));
  const container = document.body.appendChild(document.createElement("div"));
  const root = createRoot(container);
  await act(async () => root.render(<AppProvider app={{ ...app, registry }}>
    <EntityPalette open onClose={vi.fn()} commands={[{ id: "home", label: "Overview", group: "Go to", run: vi.fn() }]} />
  </AppProvider>));
  expect(document.body.textContent).toContain("First agent");
  expect(document.body.textContent).toContain("Overview");
  await act(async () => app.store.updateAether("local", (state) => ({ ...state, agents: { a1: agent("Renamed agent") } })));
  expect(document.body.textContent).toContain("Renamed agent");
  expect(document.body.textContent).not.toContain("First agent");
  await act(async () => root.unmount());
  container.remove();
});
