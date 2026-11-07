// @vitest-environment happy-dom
import { afterEach, describe, expect, test, vi } from "vitest";
import { browseCommands, CommandPalette, filterCommands, indexCommands, MAX_SHOWN, pushRecent, searchIndex, type Command } from "./command-palette.tsx";
import { click, press, render, type } from "./render.test-util.tsx";

afterEach(() => {
  document.body.innerHTML = "";
});

const command = (id: string, label: string, group: string, extra: Partial<Command> = {}): Command => ({ id, label, group, run: vi.fn(), ...extra });

const agents = Array.from({ length: 30 }, (_, n) => command(`agent:${n}`, n === 17 ? "Audit Notifications" : `Worker ${n}`, "Agent", {
  subtitle: `${n === 17 ? "auditor" : "builder"} · running`, keywords: [`ag_${n}`, n === 17 ? "auditor" : "builder"],
}));
const screens = [command("go:#/fleet", "Fleet", "Go to · Aether"), command("go:#/archon/queue", "Queue", "Go to · Archon")];
const all = [...screens, ...agents];

const labels = (container: ParentNode) => [...container.querySelectorAll(".ui-palette-label")].map((node) => node.textContent);

describe("searching", () => {
  test("finds one agent among many by exact name, partial name, role and id", () => {
    expect(filterCommands(all, "Audit Notifications").map((c) => c.id)).toEqual(["agent:17"]);
    expect(filterCommands(all, "notif").map((c) => c.id)).toEqual(["agent:17"]);
    expect(filterCommands(all, "auditor").map((c) => c.id)).toEqual(["agent:17"]);
    expect(filterCommands(all, "ag_17").map((c) => c.id)).toEqual(["agent:17"]);
  });

  test("matches the subtitle and the group, and needs every word", () => {
    expect(filterCommands(all, "running agent")).toHaveLength(30);
    expect(filterCommands(all, "audit running").map((c) => c.id)).toEqual(["agent:17"]);
    expect(filterCommands(all, "audit queue")).toEqual([]);
  });

  test("ranks a label that starts with the query above one that merely contains it, then keeps order", () => {
    const commands = [
      command("a", "Run the audit", "Item"), command("b", "Audit log", "Screen"), command("c", "Audit Notifications", "Agent"),
      command("d", "Unrelated", "Item", { keywords: ["audit"] }),
    ];
    expect(filterCommands(commands, "audit").map((c) => c.id)).toEqual(["b", "c", "a", "d"]);
    expect(filterCommands(commands, "audit not").map((c) => c.id)).toEqual(["c"]);
  });

  test("a screen keeps its place ahead of entities that only share a prefix", () => {
    const commands = [command("go", "Fleet", "Go to"), command("bot", "Fleet bot", "Agent"), command("other", "Backup fleet", "Agent")];
    expect(filterCommands(commands, "fleet").map((c) => c.id)).toEqual(["go", "bot", "other"]);
  });

  test("caps the result list and reports how many matched", () => {
    const many = Array.from({ length: 200 }, (_, n) => command(`i${n}`, `Item ${n}`, "Item"));
    const found = searchIndex(indexCommands(many), "item");
    expect(found.shown).toHaveLength(MAX_SHOWN);
    expect(found.total).toBe(200);
    expect(searchIndex(indexCommands(many), "item 19").total).toBe(12);
  });

  test("filters 5,000 entities well inside a keystroke", () => {
    const many = Array.from({ length: 5000 }, (_, n) => command(`e${n}`, `Entity number ${n}`, "Item", { subtitle: "running · acme/site", keywords: [`qi_${n}`] }));
    const index = indexCommands(many);
    const started = performance.now();
    for (const query of ["e", "entity 4", "acme number 4999", "qi_123", "zzz"]) searchIndex(index, query);
    const perQuery = (performance.now() - started) / 5;
    expect(searchIndex(index, "qi_4999").shown.map((c) => c.id)).toEqual(["e4999"]);
    expect(perQuery).toBeLessThan(25);
  });

  test("refuses duplicate ids", () => {
    expect(() => indexCommands([command("x", "One", "A"), command("x", "Two", "A")])).toThrow(/repeats/);
  });
});

describe("recents", () => {
  test("pushRecent puts the newest first without repeats and keeps the newest few", () => {
    expect(pushRecent(["a", "b", "c"], "b")).toEqual(["b", "a", "c"]);
    expect(pushRecent(["a", "b", "c"], "d", 3)).toEqual(["d", "a", "b"]);
    expect(() => pushRecent([], "")).toThrow();
  });

  test("browsing lists recents that still exist first, then the rest once", () => {
    const shown = browseCommands(indexCommands(all), ["agent:17", "gone", "go:#/archon/queue"]).shown.map((c) => c.id);
    expect(shown.slice(0, 3)).toEqual(["agent:17", "go:#/archon/queue", "go:#/fleet"]);
    expect(new Set(shown).size).toBe(shown.length);
  });
});

describe("the palette", () => {
  test("typing narrows to the agent, shows its kind and status, and Enter runs it after closing", async () => {
    const onClose = vi.fn();
    const onRun = vi.fn();
    const { container } = await render(<CommandPalette open onClose={onClose} commands={all} onRun={onRun} />);
    const input = container.querySelector<HTMLInputElement>("[role=combobox]") as HTMLInputElement;
    await type(input, "audit notif");
    const option = container.querySelector("[role=option]") as HTMLElement;
    expect(labels(container)).toEqual(["Audit Notifications"]);
    expect(option.querySelector(".ui-palette-group")?.textContent).toBe("Agent");
    expect(option.querySelector(".ui-palette-subtitle")?.textContent).toBe("auditor · running");
    await press(input, "Enter");
    expect(onClose).toHaveBeenCalledOnce();
    expect(onRun).toHaveBeenCalledWith(agents[17]);
    expect(agents[17]?.run).toHaveBeenCalledOnce();
  });

  test("an empty query lists recents first, marked, and shows at most the cap with a hint", async () => {
    const many = Array.from({ length: 120 }, (_, n) => command(`i${n}`, `Item ${n}`, "Item"));
    const { container } = await render(<CommandPalette open onClose={vi.fn()} commands={many} recents={["i99", "i3"]} />);
    expect(labels(container).slice(0, 3)).toEqual(["Item 99", "Item 3", "Item 0"]);
    const groups = [...container.querySelectorAll(".ui-palette-group")].map((node) => node.textContent);
    expect(groups.slice(0, 3)).toEqual(["Recent · Item", "Recent · Item", "Item"]);
    expect(container.querySelectorAll("[role=option]")).toHaveLength(MAX_SHOWN);
    expect(container.querySelector(".ui-palette-more")?.textContent).toMatch(/Showing 50 of 120/);
  });

  test("arrows wrap, the pointer moves the active option and a click runs it", async () => {
    const onRun = vi.fn();
    const { container } = await render(<CommandPalette open onClose={vi.fn()} commands={screens} onRun={onRun} />);
    const input = container.querySelector<HTMLInputElement>("[role=combobox]") as HTMLInputElement;
    const options = () => [...container.querySelectorAll("[role=option]")];
    await press(input, "ArrowUp");
    expect(options().map((o) => o.getAttribute("aria-selected"))).toEqual(["false", "true"]);
    await press(input, "ArrowDown");
    expect(options().map((o) => o.getAttribute("aria-selected"))).toEqual(["true", "false"]);
    await click(options()[1] as Element);
    expect(onRun).toHaveBeenCalledWith(screens[1]);
  });

  test("says so when nothing matches, and Enter then runs nothing", async () => {
    const onClose = vi.fn();
    const { container } = await render(<CommandPalette open onClose={onClose} commands={all} />);
    const input = container.querySelector<HTMLInputElement>("[role=combobox]") as HTMLInputElement;
    await type(input, "no such thing");
    expect(container.querySelector(".ui-palette-empty")?.textContent).toBe("Nothing matches “no such thing”.");
    expect(container.querySelectorAll("[role=option]")).toHaveLength(0);
    await press(input, "Enter");
    expect(onClose).not.toHaveBeenCalled();
  });
});
