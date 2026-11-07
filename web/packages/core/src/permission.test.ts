import type { ConnectionInfo } from "@aether-ui/protocol";
import { describe, expect, it } from "vitest";
import { AETHER_HOST_READS } from "./commands.ts";
import { permissionFor, readPermissionFor } from "./react.ts";
import { INITIAL_STATE, type State } from "./store.ts";

const info = (standing: ConnectionInfo["standing"], state: ConnectionInfo["state"] = "ready"): ConnectionInfo =>
  ({ id: "aether:a", daemon: "aether", state, features: [], label: "studio", ...(standing ? { standing } : {}) });
const withConn = (c: ConnectionInfo): State => ({ ...INITIAL_STATE, connections: { [c.id]: c } });

describe("permissionFor", () => {
  it("gives an observer the reason a change is refused, and lets it read", () => {
    const state = withConn(info("observer"));
    expect(permissionFor(state, "aether:a", { cmd: "cancel", agent: "a" })).toEqual({ access: "operate", refusal: "Observer devices cannot change daemon state" });
    expect(permissionFor(state, "aether:a", { cmd: "memories", query: {} }).refusal).toBeNull();
  });

  it("marks host-only commands and refuses them to a remote operator", () => {
    expect(permissionFor(withConn(info("operator")), "aether:a", { cmd: "accept_memory", id: "m" }))
      .toEqual({ access: "host_only", refusal: "Only the daemon's own host can do this" });
    expect(permissionFor(withConn(info("local")), "aether:a", { cmd: "revoke_device", id: "d" }).refusal).toBeNull();
  });

  it("refuses everything on a connection that is not ready", () => {
    expect(permissionFor(withConn(info("operator", "backoff")), "aether:a", { cmd: "send_message" }).refusal).toBe("studio is not connected");
    expect(permissionFor(INITIAL_STATE, "aether:missing", { cmd: "memories" }).refusal).toBe("The daemon is not connected");
  });
});

describe("readPermissionFor", () => {
  it("allows exactly the desktop's host read on a local Aether connection", () => {
    const host = { ...withConn(info("local")), capabilities: { "host.electron": true } };
    expect([...AETHER_HOST_READS]).toEqual(["get_remote"]);
    expect(readPermissionFor(host, "aether:a", { cmd: "get_remote" })).toEqual({ access: "host_only", refusal: null });
    for (const command of [{ cmd: "list_devices", include_revoked: true }, { cmd: "configure_remote" }, { cmd: "get_pairing" }, { cmd: "start_pairing" }]) {
      expect(readPermissionFor(host, "aether:a", command).refusal).not.toBeNull();
    }
  });

  it("refuses the host read without desktop capabilities, local standing or a ready connection", () => {
    expect(readPermissionFor(withConn(info("local")), "aether:a", { cmd: "get_remote" }).refusal).not.toBeNull();
    for (const standing of ["operator", "observer", undefined] as const) {
      const state = { ...withConn(info(standing)), capabilities: { "host.electron": true } };
      expect(readPermissionFor(state, "aether:a", { cmd: "get_remote" }).refusal).not.toBeNull();
    }
    const offline = { ...withConn(info("local", "backoff")), capabilities: { "host.electron": true } };
    expect(readPermissionFor(offline, "aether:a", { cmd: "get_remote" }).refusal).toBe("studio is not connected");
    const archon = { ...withConn({ ...info("local"), daemon: "archon" as const }), capabilities: { "host.electron": true } };
    expect(readPermissionFor(archon, "aether:a", { cmd: "get_remote" }).refusal).not.toBeNull();
  });

  it("leaves ordinary reads and earlier refusals as they were", () => {
    expect(readPermissionFor(withConn(info("observer")), "aether:a", { cmd: "memories", query: {} }).refusal).toBeNull();
    expect(readPermissionFor(withConn(info("operator")), "aether:a", { cmd: "get_remote" }).refusal).toBe("Only the daemon's own host can do this");
  });
});
