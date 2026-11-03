// The shared frontend types of CONTRACTS.md §2.4 and §2.5 (Frozen), written
// verbatim with `export` added; `ui` and `graph` use them without `core`.

// §2.4 Desktop bridge
export interface DesktopBridge {
  readonly version: 1;
  discover(): Promise<LocalDaemon[]>;            // {id, kind: "aether"|"archon", label, reachable, version?}
  connect(daemon: string): Promise<string>;      // returns a connection id
  request(conn: string, frame: unknown): Promise<unknown>;   // validated ClientFrame → reply frame
  subscribe(conn: string, frame: unknown, onFrame: (f: unknown) => void): Promise<() => void>;
  close(conn: string): Promise<void>;
  privileged(action: PrivilegedAction): Promise<unknown>;   // main builds frame and confirmation text
  startDaemon(kind: "aether" | "archon"): Promise<void>;     // only if desktop.managed_start
  pickDirectory(purpose: string): Promise<string | null>;
  notify(n: {title: string; body: string; route?: string}): void;
  openExternal(url: string): Promise<void>;      // https: only
  openRemote(origin: string): Promise<void>;     // an entry of remote.origins; opens a remote window (ARCHITECTURE.md §c)
  onDeepLink(cb: (route: string) => void): () => void;       // aether-app://open?route=…
  diagnostics(): Promise<{versions: Record<string, string>; daemons: LocalDaemon[]; logPath: string}>;
  getPreference(key: PreferenceKey): Promise<unknown>;
  setPreference(key: PreferenceKey, value: unknown): Promise<void>;
}
export type PreferenceKey = "window.bounds" | "layout.panels" | "theme" | "desktop.managed_start" | "remote.origins";

// Closed set. A new kind is a CONTRACTS.md edit. `conn` must be a local connection.
export type PrivilegedAction =
  | {kind: "memory.accept" | "memory.reject"; conn: string; memory: string}
  | {kind: "device.pair"; conn: string; name: string; role: "operator" | "observer"}
  | {kind: "device.revoke"; conn: string; device: string}
  | {kind: "remote.enable" | "remote.disable"; conn: string}
  | {kind: "remote.mount"; conn: string; prefix: "/archon"; target_port: number}
  | {kind: "noesis.configure"; conn: string; patch: NoesisSettingsPatch}   // host-only keys of §6.1
  | {kind: "archon.operator_change"; conn: string; request: "merge" | "unblock" | "settings"; item?: string; patch?: unknown}
  | {kind: "engine.login"; conn: string; engine: "claude" | "codex"};      // opens a terminal; no secret crosses IPC

// §2.5 Shared frontend types
export type ConnId = string;                                   // §5.1
export type Standing = "local" | "operator" | "observer";
export type ConnState = "connecting" | "pairing" | "ready" | "backoff" | "closed";
export interface ConnectionInfo { id: ConnId; daemon: "aether" | "archon"; state: ConnState;
  protocol?: number; features: readonly string[]; standing?: Standing; label: string }
export type Capabilities = Readonly<Record<string, boolean | string>>;   // keys of §3

export interface HostAdapter { readonly kind: "browser" | "electron";
  readonly desktop?: DesktopBridge; readonly storage: KeyValueStore }
export interface KeyValueStore { get(key: string): Promise<unknown>; set(key: string, v: unknown): Promise<void> }
export interface Transport { readonly id: "ws-aether" | "ws-archon" | "ipc";
  open(target: string, onFrame: (f: unknown) => void, onClose: (code: number, reason: string) => void):
    Promise<{ send(frame: unknown): void; close(): void }> }

export type EntityKind = "agent" | "run" | "item" | "stage" | "initiative" | "issue" | "pull_request" | "proposition";
export interface EntityRef { kind: EntityKind; conn: ConnId; id: string }
export interface RouteDef { path: string; owner: string;               // path as in §2.3
  nav?: { label: string; section: "aether" | "archon" | "github" | "learn" | "settings" };
  requires?: (caps: Capabilities) => boolean;
  load: () => Promise<{ default: unknown /* React component of RouteProps */ }> }
export interface RouteProps { params: Readonly<Record<string, string>> }
export interface EntityAction { id: string; kind: EntityKind; label: string;
  requires?: (caps: Capabilities, ref: EntityRef) => boolean; run(ref: EntityRef): void | Promise<void> }

export interface GNode { id: string; label: string; group?: string; status?: string; weight?: number }
export interface GEdge { from: string; to: string; kind: string }
export interface GGroup { id: string; label: string; parent?: string }
export interface GraphModel { nodes: readonly GNode[]; edges: readonly GEdge[]; groups: readonly GGroup[];
  collapsed: ReadonlySet<string> }
export interface Layout { hash: string; positions: ReadonlyMap<string, { x: number; y: number }> }

// The two shapes §2.4 names but does not declare.

/** A daemon the desktop shell found on this machine. */
export interface LocalDaemon { id: string; kind: "aether" | "archon"; label: string; reachable: boolean;
  version?: string }

/** A patch of the host-only Noesis settings of CONTRACTS.md §6.1; main checks every value. */
export interface NoesisSettingsPatch {
  "noesis.enabled"?: boolean;
  "noesis.budget.interactive_usd_daily"?: number;
  "noesis.budget.background_usd_daily"?: number;
  "noesis.budget.background_tokens_daily"?: number;
  "noesis.billing.allow_api"?: boolean;
  "noesis.prompts.overrides"?: boolean;
}
