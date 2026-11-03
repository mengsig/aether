// The `ws-aether` transport: aetherd's remote listener, authenticated by the
// device key. The carrier speaks first with a challenge; the transport signs
// it and completes the client's hello with the device proof.

import type { Transport } from "@aether-ui/protocol";
import { DaemonError } from "../errors.ts";
import { errorText, invariant, object, string, type Json } from "../guards.ts";
import { signChallenge, type IdentityStore } from "./identity.ts";
import { browserSocket, checkSocketUrl, openSocket, parseFrame, sendFrame, type SocketFactory, type SocketLike } from "./socket.ts";

export interface AetherSocketOptions {
  readonly identity: IdentityStore;
  /** The page origin the signature binds, `location.origin` in a browser. */
  readonly origin: string;
  readonly socket?: SocketFactory;
  readonly challengeTimeoutMs?: number;
}

interface Challenge { instance: string; nonce: string }

const CLOSE_PROTOCOL_ERROR = 1002;

export function aetherSocketTransport(options: AetherSocketOptions): Transport {
  const factory = options.socket ?? browserSocket;
  return {
    id: "ws-aether",
    async open(target, onFrame, onClose) {
      const url = checkSocketUrl(target, "/ws");
      const identity = await options.identity.load();
      if (!identity?.deviceId) throw new DaemonError("unpaired", "Pair this browser with your daemon");
      let challenged: (challenge: Challenge) => void = () => {};
      let refused: (error: Error) => void = () => {};
      const challenge = new Promise<Challenge>((resolve, reject) => { challenged = resolve; refused = reject; });
      let state: { challenge?: Challenge; helloReq?: number; welcomed: boolean } = { welcomed: false };
      const opened: { socket?: SocketLike } = {};
      const fail = (reason: string) => {
        opened.socket?.close(CLOSE_PROTOCOL_ERROR, "Invalid server frame");
        onClose(CLOSE_PROTOCOL_ERROR, reason);
      };
      const onText = (raw: unknown) => {
        try {
          const frame = parseFrame(raw);
          if (!state.challenge) {
            if (frame.frame !== "challenge") throw new Error("Expected an authentication challenge before hello");
            const received = { instance: string(frame.instance_id, "challenge.instance_id"), nonce: string(frame.nonce, "challenge.nonce") };
            state = { ...state, challenge: received };
            challenged(received);
            return;
          }
          if (frame.frame === "challenge") throw new Error("Unexpected repeated authentication challenge");
          if (!state.welcomed && frame.frame === "reply" && frame.id === state.helloReq) {
            const pinned = state.challenge;
            state = { ...state, welcomed: true };
            const device = checkWelcome(frame, identity.deviceId ?? "", pinned);
            // Pinning the daemon id is what lets a later challenge detect an identity change.
            if (device) void options.identity.save({ ...identity, ...device, daemonId: pinned.instance })
              .catch((error: unknown) => fail(`Could not save the device identity: ${errorText(error)}`));
          }
          onFrame(frame);
        } catch (error) {
          refused(error instanceof Error ? error : new Error(errorText(error)));
          fail(`Protocol error: ${errorText(error)}`);
        }
      };
      const open = (opened.socket = await openSocket(factory, url, "aether", { onText, onClose }));
      const timer = setTimeout(() => refused(new Error("Challenge timed out")), options.challengeTimeoutMs ?? 15_000);
      try {
        const received = await challenge;
        if (identity.daemonId && identity.daemonId !== received.instance) {
          throw new DaemonError("identity_changed", "Daemon identity changed; verify and pair again");
        }
        const signature = await signChallenge(identity, { ...received, origin: options.origin });
        return {
          send(frame: unknown) {
            const command = object(frame, "client frame");
            if (command.cmd !== "hello") return sendFrame(open, command);
            invariant(state.helloReq === undefined, "hello is sent once per link");
            state = { ...state, helloReq: Number(command.req) };
            sendFrame(open, { ...command, device: { id: identity.deviceId, signature } });
          },
          close: () => open.close(1000, "bye"),
        };
      } catch (error) {
        open.close(1000, "authentication failed");
        throw error;
      } finally {
        clearTimeout(timer);
      }
    },
  };
}

/** The authenticated welcome must name this device and the daemon that challenged it. */
function checkWelcome(frame: Json, deviceId: string, challenge: Challenge): { role: "operator" | "observer"; name: string } | null {
  const result = object(frame.result, "hello reply");
  if (!Object.hasOwn(result, "Ok")) return null;
  const welcome = object(object(result.Ok, "welcome").data, "welcome.data");
  const device = object(welcome.device, "welcome.device");
  if (device.id !== deviceId || welcome.instance_id !== challenge.instance) throw new Error("Invalid authenticated welcome");
  if (device.role !== "operator" && device.role !== "observer") throw new Error("Invalid authenticated device role");
  return { role: device.role, name: string(device.name, "welcome.device.name") };
}
