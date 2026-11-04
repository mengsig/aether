// @aether-ui/core: the only package that talks to daemons (CONTRACTS.md §2.1, §2.2).

export { createApp, archonMounted, CLIENT_VERSION, type App, type AppOptions, type LocalAction } from "./app.ts";
export { ConnectionManager, type ManagerOptions } from "./manager.ts";
export { Connection, ARCHON_VERSIONS, type Endpoint } from "./connection.ts";
export { AetherSync, MAX_DELTA_AGENTS } from "./aether-sync.ts";
export { ArchonSync } from "./archon-sync.ts";
export { Store, dropConnection, INITIAL_STATE, replaces } from "./store.ts";
export type { State, AetherConnState, ArchonConnState, ArchonItem, ArchonGraph, ArchonPlan, Stamped, StreamState, StreamTool, TranscriptState, Submission, Report, FrameScheduler } from "./store.ts";
export { applyRead, failRead, markRequested, readReplaces, settleSubmission, stampOf, type ReadEntry } from "./reads.ts";
export { computeCapabilities } from "./capabilities.ts";
export { aetherAccess, archonAccess, checkAllowed, withIdempotencyKey, type Access } from "./commands.ts";
export { DaemonError, RefusedError, codeOf } from "./errors.ts";
export { registerRoutes, registerEntityAction, registry, Registry } from "./registry.ts";
export { matchRoute, matchPattern, routeHref, navigate, currentHash, type RouteMatch } from "./router.ts";
export { PairingFlow, aetherPairing, archonPairing, normalizeDeviceName, type PairingState, type PairRun } from "./pairing.ts";
export { aetherSocketTransport, type AetherSocketOptions } from "./transports/ws-aether.ts";
export { archonSocketTransport, ARCHON_MOUNT } from "./transports/ws-archon.ts";
export { ipcTransport } from "./transports/ipc.ts";
export { indexedDbIdentity, type DeviceIdentity, type IdentityStore } from "./transports/identity.ts";
export type { SocketFactory, SocketLike } from "./transports/socket.ts";
export * from "./selectors.ts";
export { AppProvider, useApp, useSelector, useCapabilities, useCommand, useRoutes, useEntityActions, useRoute, useReread, usePermission, permissionFor, type Permission, type CommandInput } from "./react.ts";
