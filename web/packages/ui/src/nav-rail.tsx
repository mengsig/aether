import type { Capabilities, RouteDef } from "@aether-ui/protocol";
import type { ReactNode } from "react";
import { invariant } from "./classes.ts";
import { NavIcon } from "./nav-icons.tsx";

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

/** A heading that only repeats its one entry's name ("Settings" over "Settings") is left out; the list keeps the name. */
export function redundantHeading(section: NavSection): boolean {
  invariant(section.items.length > 0, `nav section ${section.id} has no entries`);
  const only = section.items.length === 1 ? section.items[0] : undefined;
  return only !== undefined && only.label.trim().toLowerCase() === section.label.trim().toLowerCase();
}

/** The primary navigation: muted uppercase group labels over pill links, the current one tinted and barred. */
export function NavRail({ sections, hash, footer }: NavRailProps) {
  invariant(new Set(sections.map((section) => section.id)).size === sections.length, "nav sections repeat an id");
  const active = activeHref(sections, hash);
  return (
    <nav className="ui-navrail" aria-label="Primary">
      <a className="ui-brand" href="#/">
        <span className="ui-brand-mark" aria-hidden="true">a</span>
        <span className="ui-brand-name">Aether</span>
      </a>
      <div className="ui-navrail-sections">
        {sections.map((section) => <NavGroup key={section.id} section={section} active={active} />)}
      </div>
      {footer === undefined ? null : <div className="ui-navrail-footer">{footer}</div>}
    </nav>
  );
}

function NavGroup({ section, active }: { readonly section: NavSection; readonly active: string | null }) {
  const solo = redundantHeading(section);
  const headingId = `nav-${section.id}`;
  return (
    <div className="ui-navrail-section" data-section={section.id} data-solo={solo ? "true" : undefined}>
      {solo ? null : <h2 className="ui-navrail-heading" id={headingId}>{section.label}</h2>}
      <ul aria-labelledby={solo ? undefined : headingId} aria-label={solo ? section.label : undefined}>
        {section.items.map((item) => (
          <li key={item.href}>
            <a className="ui-navrail-link" href={item.href} aria-current={item.href === active ? "page" : undefined}>
              <NavIcon label={item.label} />
              <span className="ui-navrail-label">{item.label}</span>
            </a>
          </li>
        ))}
      </ul>
    </div>
  );
}
