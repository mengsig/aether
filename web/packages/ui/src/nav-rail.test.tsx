// @vitest-environment happy-dom
import { afterEach, describe, expect, test } from "vitest";
import { FOCUSABLE } from "./focus.ts";
import { hasNavIcon } from "./nav-icons.tsx";
import { NavRail, redundantHeading, type NavSection } from "./nav-rail.tsx";
import { PageHeader } from "./page.tsx";
import { render } from "./render.test-util.tsx";

afterEach(() => {
  document.body.innerHTML = "";
});

const SECTIONS: NavSection[] = [
  { id: "aether", label: "Aether", items: [{ href: "#/fleet", label: "Fleet", owner: "B1" }, { href: "#/inbox", label: "Inbox", owner: "B2" }] },
  { id: "archon", label: "Archon", items: [{ href: "#/archon/queue", label: "Queue", owner: "C1" }] },
  { id: "settings", label: "Settings", items: [{ href: "#/settings", label: "Settings", owner: "B3" }] },
];

describe("nav hierarchy", () => {
  test("group headings are plain h2 text: not links, not focusable, and they name their lists", async () => {
    const { container } = await render(<NavRail sections={SECTIONS} hash="#/inbox" />);
    const headings = [...container.querySelectorAll<HTMLElement>(".ui-navrail-heading")];
    expect(headings.map((heading) => heading.textContent)).toEqual(["Aether", "Archon"]);
    for (const heading of headings) {
      expect(heading.tagName).toBe("H2");
      expect(heading.matches(FOCUSABLE)).toBe(false);
      expect(heading.querySelector(FOCUSABLE)).toBeNull();
      expect(heading.closest("a")).toBeNull();
      const list = container.querySelector(`ul[aria-labelledby="${heading.id}"]`);
      expect(list?.querySelectorAll("a.ui-navrail-link").length).toBeGreaterThan(0);
    }
  });

  test("a group whose only entry repeats its name has no heading; the list keeps the name", async () => {
    const { container } = await render(<NavRail sections={SECTIONS} hash="#/settings/devices" />);
    const settings = container.querySelector<HTMLElement>('[data-section="settings"]');
    expect(settings?.dataset.solo).toBe("true");
    expect(settings?.querySelector("h2")).toBeNull();
    expect(settings?.querySelector("ul")?.getAttribute("aria-label")).toBe("Settings");
    expect(settings?.querySelector("a")?.getAttribute("aria-current")).toBe("page");
    expect(container.querySelector<HTMLElement>('[data-section="archon"]')?.dataset.solo).toBeUndefined();
  });

  test("the redundancy rule needs one entry with the group's own name", () => {
    expect(redundantHeading(SECTIONS[2] as NavSection)).toBe(true);
    expect(redundantHeading(SECTIONS[1] as NavSection)).toBe(false);
    expect(redundantHeading({ id: "learn", label: "Learn", items: [{ href: "#/learn", label: " learn ", owner: "F5" }] })).toBe(true);
    expect(() => redundantHeading({ id: "github", label: "GitHub", items: [] })).toThrow(/no entries/);
  });

  test("each link carries a hidden icon before its label, and only the current link is marked", async () => {
    const { container } = await render(<NavRail sections={SECTIONS} hash="#/fleet?filter=running" />);
    const links = [...container.querySelectorAll<HTMLAnchorElement>(".ui-navrail-link")];
    expect(links.map((link) => link.querySelector("svg")?.getAttribute("aria-hidden"))).toEqual(["true", "true", "true", "true"]);
    expect(links.map((link) => link.textContent)).toEqual(["Fleet", "Inbox", "Queue", "Settings"]);
    expect(links.filter((link) => link.getAttribute("aria-current") === "page").map((link) => link.getAttribute("href"))).toEqual(["#/fleet"]);
  });

  test("every nav entry the features register today has its own icon", () => {
    const known = ["Fleet", "Topology", "Inbox", "Activity", "Memory", "Queue", "History", "Task graph", "Learn", "Sessions", "Knowledge", "My understanding", "Settings"];
    expect(known.filter((label) => !hasNavIcon(label))).toEqual([]);
    expect(hasNavIcon("Something new")).toBe(false);
  });
});

describe("PageHeader", () => {
  test("renders the title, lede and actions in the shared page head", async () => {
    const { container } = await render(<PageHeader title="agt_7 · Planner" lede="Running for 3 min" actions={<button type="button">Pause</button>} />);
    const head = container.querySelector<HTMLElement>("header.ui-page-head");
    expect(head?.dataset.actions).toBe("true");
    expect(head?.querySelector("h2.ui-page-title")?.textContent).toBe("agt_7 · Planner");
    expect(head?.querySelector(".ui-page-lede")?.textContent).toBe("Running for 3 min");
    expect(head?.querySelector(".ui-page-actions button")?.textContent).toBe("Pause");
  });

  test("leaves out what it is not given and refuses an empty title", async () => {
    const { container } = await render(<PageHeader title="Inbox" actions={null} />);
    const head = container.querySelector<HTMLElement>("header.ui-page-head");
    expect(head?.dataset.actions).toBeUndefined();
    expect(head?.querySelector(".ui-page-lede, .ui-page-actions")).toBeNull();
    expect(() => PageHeader({ title: "  " })).toThrow(/needs a title/);
  });
});
