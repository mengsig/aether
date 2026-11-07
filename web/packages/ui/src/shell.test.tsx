// @vitest-environment happy-dom
import type { EntityAction, RouteDef } from "@aether-ui/protocol";
import { act } from "react";
import { afterEach, describe, expect, test, vi } from "vitest";
import { AppShell, type AppShellProps } from "./app-shell.tsx";
import { CommandPalette, filterCommands } from "./command-palette.tsx";
import { EntityActionMenu } from "./entity-actions.tsx";
import { activeHref, navHref, navSections } from "./nav-rail.tsx";
import { byText, click, press, render, type } from "./render.test-util.tsx";
import { ThemeProvider } from "./theme.tsx";

afterEach(() => {
  document.body.innerHTML = "";
  delete document.documentElement.dataset.theme;
});

const Screen = (props: { params: Readonly<Record<string, string>> }) => <p className="screen">screen {JSON.stringify(props.params)}</p>;
const load = vi.fn(async () => ({ default: Screen }));
const routes: RouteDef[] = [
  { path: "#/fleet", owner: "B1", nav: { label: "Fleet", section: "aether" }, load },
  { path: "#/fleet/topology", owner: "B1", nav: { label: "Topology", section: "aether" }, load },
  { path: "#/agents/:conn/:agent", owner: "B2", nav: { label: "Agent", section: "aether" }, load },
  { path: "#/archon/queue", owner: "C1", nav: { label: "Queue", section: "archon" }, load },
  { path: "#/learn", owner: "F5", nav: { label: "Learn", section: "learn" }, requires: (caps) => caps["noesis.available"] === true, load },
  { path: "#/settings/*", owner: "B3", nav: { label: "Settings", section: "settings" }, load },
];

async function settle(): Promise<void> {
  for (let round = 0; round < 3; round += 1) await act(async () => new Promise((resolve) => setTimeout(resolve, 0)));
}

function shell(overrides: Partial<AppShellProps> = {}) {
  const props: AppShellProps = {
    routes, actions: [], capabilities: {}, match: null, hash: "#/", onNavigate: vi.fn(), onActionError: vi.fn(), ...overrides,
  };
  return { props, node: <ThemeProvider initialPreference="light"><AppShell {...props} /></ThemeProvider> };
}

describe("navigation model", () => {
  test("groups nav routes by section, drops param routes and unmet requirements, and roots wildcards", () => {
    const sections = navSections(routes, {});
    expect(sections.map((section) => [section.id, section.items.map((item) => item.href)])).toEqual([
      ["aether", ["#/fleet", "#/fleet/topology"]], ["archon", ["#/archon/queue"]], ["settings", ["#/settings"]],
    ]);
    expect(navSections(routes, { "noesis.available": true }).map((section) => section.id)).toContain("learn");
    expect(navHref("#/agents/:conn/:agent")).toBeNull();
    expect(() => navHref("fleet")).toThrow(/starts with #\//);
  });

  test("the longest owning link is active", () => {
    const sections = navSections(routes, {});
    expect(activeHref(sections, "#/fleet/topology")).toBe("#/fleet/topology");
    expect(activeHref(sections, "#/fleet")).toBe("#/fleet");
    expect(activeHref(sections, "#/settings/devices?x=1")).toBe("#/settings");
    expect(activeHref(sections, "#/fleetx")).toBeNull();
  });
});

describe("AppShell", () => {
  test("renders the nav from its route props and the matched screen with its params", async () => {
    const route = routes[2] as RouteDef;
    const { node } = shell({ match: { route, params: { conn: "local", agent: "agt_1" } }, hash: "#/agents/local/agt_1" });
    const { container } = await render(node);
    await settle();
    expect([...container.querySelectorAll(".ui-navrail-link")].map((link) => link.getAttribute("href"))).toEqual(["#/fleet", "#/fleet/topology", "#/archon/queue", "#/settings"]);
    expect(container.querySelector(".screen")?.textContent).toBe('screen {"conn":"local","agent":"agt_1"}');
    expect(container.querySelector("h1")?.textContent).toBe("Agent");
    expect(document.title).toBe("Agent · Aether");
  });

  test("names a route with no nav entry by its listed title, else by the product", async () => {
    const route: RouteDef = { path: "#/archon/items/:item", owner: "C1", load };
    const match = { route, params: { item: "qi_1" } };
    const titled = await render(shell({ match, hash: "#/archon/items/qi_1", titles: { [route.path]: "Item" } }).node);
    await settle();
    expect(titled.container.querySelector("h1")?.textContent).toBe("Item");
    expect(document.title).toBe("Item · Aether");
    document.body.innerHTML = "";
    const untitled = await render(shell({ match, hash: "#/archon/items/qi_1" }).node);
    await settle();
    expect(untitled.container.querySelector("h1")?.textContent).toBe("Aether");
  });

  test("marks the current link and says when no screen or no permission applies", async () => {
    const { container, rerender } = await render(shell({ hash: "#/fleet/topology", match: { route: routes[1] as RouteDef, params: {} } }).node);
    expect(byText(container, "a", "Topology").getAttribute("aria-current")).toBe("page");
    expect(byText(container, "a", "Fleet").getAttribute("aria-current")).toBeNull();
    await rerender(shell({ hash: "#/nowhere" }).node);
    expect(container.querySelector(".ui-state-title")?.textContent).toBe("No screen here");
    await rerender(shell({ hash: "#/learn", match: { route: routes[4] as RouteDef, params: {} } }).node);
    expect(container.querySelector(".ui-state-title")?.textContent).toBe("Not available here");
    await rerender(shell({ routes: [], hash: "#/" }).node);
    expect(container.querySelector(".ui-state-title")?.textContent).toBe("No screens yet");
  });

  test("a screen that fails shows an error with a retry that loads it again", async () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => undefined);
    let fail = true;
    const flaky = vi.fn(async () => {
      if (fail) throw new Error("chunk missing");
      return { default: Screen };
    });
    const route: RouteDef = { path: "#/flaky", owner: "test", load: flaky };
    const { container } = await render(shell({ routes: [route], match: { route, params: {} }, hash: "#/flaky" }).node);
    await settle();
    expect(container.querySelector("[role=alert]")?.textContent).toContain("chunk missing");
    fail = false;
    await click(byText(container, "button", "Try again"));
    await settle();
    expect(flaky).toHaveBeenCalledTimes(2);
    expect(container.querySelector(".screen")).not.toBeNull();
    error.mockRestore();
  });

  test("a screen that throws without a message, or throws a non-Error, still keeps the shell up", async () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => undefined);
    for (const thrown of [new Error(""), undefined, "  "]) {
      const route: RouteDef = { path: "#/silent", owner: "test", load: async () => { throw thrown; } };
      const { container } = await render(shell({ routes: [route], match: { route, params: {} }, hash: "#/silent" }).node);
      await settle();
      expect(container.querySelector(".ui-shell")).not.toBeNull();
      expect(container.querySelector("[role=alert] .ui-state-body")?.textContent).toMatch(/stopped without saying why/);
      document.body.innerHTML = "";
    }
    error.mockRestore();
  });

  test("Ctrl+K opens the palette, typing filters, Enter navigates", async () => {
    const { node, props } = shell();
    const { container } = await render(node);
    await press(document.body, "k", { ctrlKey: true });
    const input = container.querySelector<HTMLInputElement>("[role=combobox]");
    expect(input).not.toBeNull();
    expect(document.activeElement).toBe(input);
    await type(input as HTMLInputElement, "topo");
    expect([...container.querySelectorAll("[role=option]")].map((option) => option.firstChild?.textContent)).toEqual(["Topology"]);
    await press(input as HTMLInputElement, "Enter");
    expect(props.onNavigate).toHaveBeenCalledWith("#/fleet/topology");
    expect(container.querySelector("[role=combobox]")).toBeNull();
  });

  test("arrow keys scroll the active command into view, wrapping at either end", async () => {
    const commands = Array.from({ length: 40 }, (_, index) => ({ id: `c${index}`, label: `Command ${index}`, group: "Test", run: vi.fn() }));
    const scrolled: string[] = [];
    const scroll = vi.spyOn(HTMLElement.prototype, "scrollIntoView").mockImplementation(function (this: HTMLElement) {
      scrolled.push(this.firstChild?.textContent ?? "");
    });
    const { container } = await render(<CommandPalette open onClose={vi.fn()} commands={commands} />);
    const input = container.querySelector<HTMLInputElement>("[role=combobox]") as HTMLInputElement;
    for (let step = 0; step < 25; step += 1) await press(input, "ArrowDown");
    expect(scrolled.at(-1)).toBe("Command 25");
    expect(document.getElementById(input.getAttribute("aria-activedescendant") ?? "")?.firstChild?.textContent).toBe("Command 25");
    for (let step = 0; step < 26; step += 1) await press(input, "ArrowUp");
    expect(scrolled.at(-1)).toBe("Command 39");
    scroll.mockRestore();
  });

  test("palette theme commands switch <html data-theme>", async () => {
    const { container } = await render(shell().node);
    expect(document.documentElement.dataset.theme).toBe("light");
    await click(byText(container, "button", "Jump to"));
    await type(container.querySelector("[role=combobox]") as HTMLInputElement, "dark theme");
    await press(container.querySelector("[role=combobox]") as Element, "Enter");
    expect(document.documentElement.dataset.theme).toBe("dark");
  });

  test("filterCommands puts label-prefix matches first and needs every word", () => {
    const run = () => undefined;
    const commands = [{ id: "1", label: "Queue history", group: "Go to", run }, { id: "2", label: "History", group: "Go to", run }];
    expect(filterCommands(commands, "hist").map((command) => command.id)).toEqual(["2", "1"]);
    expect(filterCommands(commands, "queue go").map((command) => command.id)).toEqual(["1"]);
    expect(filterCommands(commands, "nothing")).toEqual([]);
  });
});

describe("entity actions", () => {
  const entity = { kind: "agent", conn: "local", id: "agt_1" } as const;
  test("lists only matching, permitted actions and reports a failing one", async () => {
    const failure = new Error("denied");
    const actions: EntityAction[] = [
      { id: "open", kind: "agent", label: "Open", run: vi.fn() },
      { id: "pause", kind: "agent", label: "Pause", requires: (caps, ref) => caps[`can.operate.${ref.conn}`] === true, run: vi.fn() },
      { id: "merge", kind: "item", label: "Merge", run: vi.fn() },
      { id: "kill", kind: "agent", label: "Kill", run: () => Promise.reject(failure) },
    ];
    const onActionError = vi.fn();
    const Probe = () => <EntityActionMenu entity={entity} />;
    const route: RouteDef = { path: "#/probe", owner: "test", load: async () => ({ default: Probe }) };
    const { container } = await render(shell({ routes: [route], actions, onActionError, match: { route, params: {} }, hash: "#/probe" }).node);
    await settle();
    await click(byText(container, "button", "Actions"));
    expect([...container.querySelectorAll("[role=menuitem]")].map((item) => item.textContent)).toEqual(["Open", "Kill"]);
    await click(byText(container, "[role=menuitem]", "Kill"));
    await settle();
    expect(onActionError).toHaveBeenCalledWith(actions[3], failure);
  });
});
