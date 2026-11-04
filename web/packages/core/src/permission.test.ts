import type { ConnectionInfo } from "@aether-ui/protocol";
import { describe, expect, it } from "vitest";
import { permissionFor } from "./react.ts";
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
