import { Component, createElement, lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState, type ComponentType, type ErrorInfo, type LazyExoticComponent, type ReactNode } from "react";
import type { Capabilities, ConnectionInfo, EntityAction, RouteDef, RouteProps } from "@aether-ui/protocol";
import { Button } from "./button.tsx";
import { invariant } from "./classes.ts";
import { CommandPalette, pushRecent, type Command } from "./command-palette.tsx";
import { EntityActionProvider } from "./entity-actions.tsx";
import { navSections, NavRail, type NavSection } from "./nav-rail.tsx";
import { ConnectionList, ThemeMenu } from "./shell-parts.tsx";
import { EmptyState, ErrorState, Skeleton } from "./states.tsx";
import { useTheme, THEME_PREFERENCES } from "./theme.tsx";
import { ToastRegion, type ToastMessage } from "./toast.tsx";

/** The route the app matched for the current hash (core's `useRoute()` gives exactly this). */
export interface ShellMatch {
  readonly route: RouteDef;
  readonly params: Readonly<Record<string, string>>;
}

export interface AppShellProps {
  readonly routes: readonly RouteDef[];
  readonly actions: readonly EntityAction[];
  readonly capabilities: Capabilities;
  readonly match: ShellMatch | null;
  readonly hash: string;
  readonly onNavigate: (href: string) => void;
  /** Where a failed entity action is reported; the shell never drops one. */
  readonly onActionError: (action: EntityAction, error: unknown) => void;
  readonly connections?: readonly ConnectionInfo[];
  readonly notices?: readonly ToastMessage[];
  readonly onDismissNotice?: (id: ToastMessage["id"]) => void;
  /** Extra palette commands, after navigation and theme. */
  readonly commands?: readonly Command[];
  /** The palette's recent destinations (command ids, newest first) to start from, and where each change is saved. */
  readonly initialRecents?: readonly string[];
  readonly onRecentsChange?: (recents: readonly string[]) => void;
  /** The header of each route with no nav entry, by route path. */
  readonly titles?: Readonly<Record<string, string>>;
}

type RouteComponent = ComponentType<RouteProps>;
const screens = new WeakMap<RouteDef, LazyExoticComponent<RouteComponent>>();

/** One lazy component per route definition, so a screen's chunk loads once. */
export function screenFor(route: RouteDef): LazyExoticComponent<RouteComponent> {
  const cached = screens.get(route);
  if (cached) return cached;
  const screen = lazy(async () => {
    const module = await route.load();
    const view = module.default;
    invariant(typeof view === "function" || (typeof view === "object" && view !== null), `route ${route.path} loaded no component`);
    return { default: view as RouteComponent };
  });
  screens.set(route, screen);
  return screen;
}

function paletteCommands(sections: readonly NavSection[], onNavigate: (href: string) => void, theme: ReturnType<typeof useTheme>): Command[] {
  const commands: Command[] = sections.flatMap((section) => section.items.map((item) => ({
    id: `go:${item.href}`, label: item.label, group: `Go to · ${section.label}`, run: () => onNavigate(item.href),
  })));
  if (theme) {
    for (const preference of THEME_PREFERENCES) {
      commands.push({ id: `theme:${preference}`, label: `Use ${preference} theme`, group: "Theme", transient: true, run: () => theme.setPreference(preference) });
    }
  }
  return commands;
}

/** The frame every screen renders in: the nav rail, a header, the routed screen and the toasts (CONTRACTS.md §2.5). */
/** The header for the current route: its nav label, else its listed title, else the product name. */
function screenTitle(match: ShellMatch | null, titles: Readonly<Record<string, string>> = {}): string {
  if (match === null) return "Overview";
  const route = match.route;
  return route.nav?.label ?? titles[route.path] ?? "Aether";
}

export function AppShell(props: AppShellProps) {
  const { routes, capabilities, hash, onNavigate } = props;
  const theme = useTheme();
  const [paletteOpen, setPaletteOpen] = useState(false);
  const sections = useMemo(() => navSections(routes, capabilities), [routes, capabilities]);
  const commands = useMemo(() => [...paletteCommands(sections, onNavigate, theme), ...(props.commands ?? [])],
    [sections, onNavigate, theme, props.commands]);
  const { recents, remember } = useRecents(props.initialRecents, props.onRecentsChange);
  const title = screenTitle(props.match, props.titles);
  useGlobalShortcut(() => setPaletteOpen(true));
  useScreenFocus(hash, title);

  return (
    <EntityActionProvider actions={props.actions} capabilities={capabilities} onError={props.onActionError}>
      <div className="ui-shell">
        <Button className="ui-skip" variant="primary" onClick={() => document.getElementById("ui-main")?.focus()}>Skip to content</Button>
        <NavRail sections={sections} hash={hash} footer={<ConnectionList connections={props.connections ?? []} />} />
        <div className="ui-shell-body">
          <header className="ui-shell-header">
            <h1 className="ui-shell-title">{title}</h1>
            <div className="ui-shell-tools">
              <Button className="ui-palette-trigger" onClick={() => setPaletteOpen(true)} aria-keyshortcuts="Control+K Meta+K">
                <span>Jump to…</span>
                <kbd className="ui-kbd">Ctrl K</kbd>
              </Button>
              {theme ? <ThemeMenu theme={theme} /> : null}
            </div>
          </header>
          <main className="ui-shell-main" id="ui-main" tabIndex={-1}>
            <RouteView match={props.match} hash={hash} capabilities={capabilities} hasRoutes={sections.length > 0} />
          </main>
        </div>
        <ToastRegion toasts={props.notices ?? []} onDismiss={props.onDismissNotice ?? (() => undefined)} />
        <CommandPalette open={paletteOpen} onClose={() => setPaletteOpen(false)} commands={commands} recents={recents} onRun={remember} />
      </div>
    </EntityActionProvider>
  );
}

/** The ids of the last destinations the palette ran; `remember` skips transient commands and reports each change. */
function useRecents(initial: readonly string[] = [], onChange?: (recents: readonly string[]) => void) {
  const [recents, setRecents] = useState<readonly string[]>(initial);
  const remember = useCallback((command: Command) => {
    if (command.transient) return;
    const next = pushRecent(recents, command.id);
    setRecents(next);
    onChange?.(next);
  }, [recents, onChange]);
  return { recents, remember };
}

function useGlobalShortcut(open: () => void): void {
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
        event.preventDefault();
        open();
      }
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [open]);
}

/** Names the document after the screen and, after a navigation, moves focus to it for screen-reader users. */
function useScreenFocus(hash: string, title: string): void {
  const first = useRef(true);
  useEffect(() => {
    document.title = `${title} · Aether`;
  }, [title]);
  useEffect(() => {
    if (first.current) {
      first.current = false;
      return;
    }
    document.getElementById("ui-main")?.focus({ preventScroll: true });
  }, [hash]);
}

function RouteView(props: { readonly match: ShellMatch | null; readonly hash: string; readonly capabilities: Capabilities; readonly hasRoutes: boolean }) {
  const { match } = props;
  if (match === null) {
    const home = props.hash === "" || props.hash === "#" || props.hash === "#/";
    if (!home) return <EmptyState title="No screen here" action={<a className="ui-link" href="#/">Back to the overview</a>}>Nothing is registered at {props.hash}.</EmptyState>;
    return props.hasRoutes
      ? <EmptyState title="Pick a screen">Choose one from the navigation, or press Ctrl K to jump anywhere.</EmptyState>
      : <EmptyState title="No screens yet">The feature packages have not registered any screens in this build.</EmptyState>;
  }
  if (match.route.requires?.(props.capabilities) === false) {
    return <EmptyState title="Not available here">This screen needs a daemon or feature that none of your connections offers.</EmptyState>;
  }
  return <ScreenBoundary key={match.route.path} route={match.route} params={match.params} />;
}

function ScreenSkeleton() {
  return (
    <div className="ui-screen-skeleton" aria-busy="true" aria-label="Loading the screen">
      <Skeleton width="40%" height="1.6em" />
      <Skeleton height="6rem" />
      <Skeleton width="70%" />
    </div>
  );
}

interface BoundaryState {
  /** What the user reads about the failure; never empty. */
  readonly failure: string | null;
  readonly attempt: number;
}

const SILENT_FAILURE = "It stopped without saying why; the browser console has the details.";

/** A readable message for anything a screen may throw, including errors with no message. */
function failureMessage(error: unknown): string {
  const message = error instanceof Error ? error.message : typeof error === "string" ? error : "";
  return message.trim() === "" ? SILENT_FAILURE : message;
}

interface BoundaryProps {
  readonly route: RouteDef;
  readonly params: Readonly<Record<string, string>>;
}

/** Shows a failed screen (a chunk that did not load, a render that threw) with a retry, and keeps the shell up. */
class ScreenBoundary extends Component<BoundaryProps, BoundaryState> {
  override state: BoundaryState = { failure: null, attempt: 0 };

  static getDerivedStateFromError(error: unknown): Partial<BoundaryState> {
    return { failure: failureMessage(error) };
  }

  override componentDidCatch(error: unknown, info: ErrorInfo): void {
    console.error("A screen failed to render", error, info.componentStack);
  }

  /** A lazy component keeps its rejection, so a retry needs a fresh one. */
  private retry(): void {
    screens.delete(this.props.route);
    this.setState((state) => ({ failure: null, attempt: state.attempt + 1 }));
  }

  override render(): ReactNode {
    if (this.state.failure !== null) {
      return <ErrorState title="This screen failed" message={this.state.failure} onRetry={() => this.retry()} />;
    }
    // The screen is looked up here, not passed in, so a retry picks up the fresh lazy component.
    return (
      <div key={this.state.attempt} className="ui-screen">
        <Suspense fallback={<ScreenSkeleton />}>{createElement(screenFor(this.props.route), { params: this.props.params })}</Suspense>
      </div>
    );
  }
}
