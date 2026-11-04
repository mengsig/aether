// Deep links, `aether-app://open?route=/fleet`, parsed and checked in main against an allowlist
// of the frontend's routes. Anything else is dropped. A valid route reaches the app window once
// it has loaded, through the preload's `desktop:deep-link` channel.

export const DEEP_LINK_SCHEME = "aether-app";
export const DEEP_LINK_CHANNEL = "desktop:deep-link";
const MAX_LINK_LENGTH = 1024;

/**
 * The routes a link or a notification may open: the shared frontend's screens (apps/web
 * features.test.ts pins that list), with `:name` a single id segment. Pairing is not here.
 */
export const DEEP_LINK_ROUTES: readonly string[] = [
  "/fleet", "/fleet/topology", "/agents/:conn/:agent",
  "/archon/queue", "/archon/items/:item", "/archon/items/:item/runs/:run",
];

const ID_SEGMENT = /^[A-Za-z0-9][A-Za-z0-9._:~-]{0,127}$/;
const ROUTE_PATTERNS = DEEP_LINK_ROUTES.map((route) => route.slice(1).split("/"));

/** Whether `route` is one of the allowlisted screens, with plain ids in its parameters. */
export function isAllowedRoute(route: string): boolean {
  if (route.length > MAX_LINK_LENGTH || !route.startsWith("/")) return false;
  const segments = route.slice(1).split("/");
  return ROUTE_PATTERNS.some((pattern) => pattern.length === segments.length && pattern.every((part, index) => {
    const segment = segments[index] as string;
    return part.startsWith(":") ? ID_SEGMENT.test(segment) : part === segment;
  }));
}

/** The route an `aether-app://open?route=…` link names, or undefined for anything else. */
export function parseDeepLink(link: string): string | undefined {
  if (typeof link !== "string" || link.length > MAX_LINK_LENGTH) return undefined;
  const url = URL.parse(link);
  if (url === null || url.protocol !== `${DEEP_LINK_SCHEME}:` || url.host !== "open") return undefined;
  if (url.username !== "" || url.password !== "" || url.hash !== "") return undefined;
  if (url.pathname !== "" && url.pathname !== "/") return undefined;
  const keys = [...url.searchParams.keys()];
  if (keys.length !== 1 || keys[0] !== "route") return undefined;
  const route = url.searchParams.get("route");
  return route !== null && isAllowedRoute(route) ? route : undefined;
}

/** The deep links among a process's arguments, as Linux and Windows pass them. */
export function linksInArgv(argv: readonly string[]): string[] {
  return argv.filter((arg) => arg.toLowerCase().startsWith(`${DEEP_LINK_SCHEME}://`));
}

/** The window a route goes to. */
export interface RouteTarget {
  isReady(): boolean;
  /** Brings the window forward. */
  focus(): void;
  /** Brings the window forward on `route`. */
  show(route: string): void;
}

/**
 * Takes links from anywhere (argv, `open-url`, a notification click) and hands the valid ones
 * to the app window. A link that arrives before the window is ready waits; only the latest does.
 */
export class DeepLinkRouter {
  private target: RouteTarget | undefined;
  private pending: string | undefined;

  /** Accepts a link; true when it named an allowed route. */
  acceptLink(link: string): boolean {
    const route = parseDeepLink(link);
    if (route === undefined) return false;
    this.open(route);
    return true;
  }

  acceptArgv(argv: readonly string[]): void {
    for (const link of linksInArgv(argv)) this.acceptLink(link);
  }

  /** Opens an allowed route; one main checked already, such as a notification's. */
  open(route: string): void {
    if (!isAllowedRoute(route)) throw new Error(`${route.slice(0, 80)} is not an allowed route`);
    if (this.target === undefined || !this.target.isReady()) {
      this.pending = route;
      return;
    }
    this.target.show(route);
  }

  /** Brings the app window forward, as a click on a notification with no route does. */
  focus(): void {
    if (this.target !== undefined && this.target.isReady()) this.target.focus();
  }

  /** The app window has loaded: it takes the waiting route, then every later one. */
  attach(target: RouteTarget): void {
    if (!target.isReady()) throw new Error("attach a route target once it has loaded");
    this.target = target;
    const waiting = this.pending;
    this.pending = undefined;
    if (waiting !== undefined) target.show(waiting);
  }
}
