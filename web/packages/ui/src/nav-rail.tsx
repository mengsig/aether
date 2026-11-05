import type { Capabilities, RouteDef } from "@aether-ui/protocol";
import type { ReactNode } from "react";
import { invariant } from "./classes.ts";

export type NavSectionId = NonNullable<RouteDef["nav"]>["section"];
export const NAV_SECTIONS: ReadonlyArray<{ readonly id: NavSectionId; readonly label: string }> = [
  { id: "aether", label: "Aether" },
  { id: "archon", label: "Archon" },
  { id: "github", label: "GitHub" },
  { id: "learn", label: "Learn" },
  { id: "settings", label: "Settings" },
];

export interface NavItem {
  readonly href: string;
  readonly label: string;
  readonly owner: string;
}

export interface NavSection {
  readonly id: NavSectionId;
  readonly label: string;
  readonly items: readonly NavItem[];
}

/** The link for a nav route: a route with `:params` has no fixed link, and a final `/*` stands for its root. */
export function navHref(path: string): string | null {
  invariant(path.startsWith("#/"), `route ${path} starts with #/`);
  if (path.includes(":")) return null;
  return path.endsWith("/*") ? path.slice(0, -2) : path;
}

/** Routes that asked for a nav entry and whose `requires` holds, grouped in the fixed section order. */
export function navSections(routes: readonly RouteDef[], capabilities: Capabilities): NavSection[] {
  const sections = NAV_SECTIONS.map((section) => ({ ...section, items: [] as NavItem[] }));
  for (const route of routes) {
    const href = navHref(route.path);
    if (route.nav === undefined || href === null || route.requires?.(capabilities) === false) continue;
    const section = sections.find((candidate) => candidate.id === route.nav?.section);
    invariant(section !== undefined, `route ${route.path} names an unknown nav section`);
    section.items.push({ href, label: route.nav.label, owner: route.owner });
  }
  return sections.filter((section) => section.items.length > 0);
}

/** The nav link that owns a hash: the longest href equal to it or a parent of it. */
export function activeHref(sections: readonly NavSection[], hash: string): string | null {
  const path = hash.split("?")[0] ?? "";
  let best: string | null = null;
  for (const item of sections.flatMap((section) => section.items)) {
    const owns = path === item.href || path.startsWith(`${item.href}/`);
    if (owns && (best === null || item.href.length > best.length)) best = item.href;
  }
  return best;
}

export interface NavRailProps {
  readonly sections: readonly NavSection[];
  readonly hash: string;
  readonly footer?: ReactNode;
}

/** The primary navigation: the same ground as the page, parted by one hairline. */
export function NavRail({ sections, hash, footer }: NavRailProps) {
  const active = activeHref(sections, hash);
  return (
    <nav className="ui-navrail" aria-label="Primary">
      <a className="ui-brand" href="#/">
        <span className="ui-brand-mark" aria-hidden="true">a</span>
        <span className="ui-brand-name">Aether</span>
      </a>
      <div className="ui-navrail-sections">
        {sections.map((section) => (
          <div className="ui-navrail-section" key={section.id}>
            <h2 className="ui-navrail-heading" id={`nav-${section.id}`}>{section.label}</h2>
            <ul aria-labelledby={`nav-${section.id}`}>
              {section.items.map((item) => (
                <li key={item.href}>
                  <a className="ui-navrail-link" href={item.href} aria-current={item.href === active ? "page" : undefined}>{item.label}</a>
                </li>
              ))}
            </ul>
          </div>
        ))}
      </div>
      {footer === undefined ? null : <div className="ui-navrail-footer">{footer}</div>}
    </nav>
  );
}
