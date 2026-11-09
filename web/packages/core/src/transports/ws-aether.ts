// The `ws-aether` transport: aetherd's remote listener. It signs the carrier's
// challenge with the device key and adds the proof to the client's hello.

import type { Transport } from "@aether-ui/protocol";
import { DaemonError } from "../errors.ts";
import { errorText, invariant, object, string, type Json } from "../guards.ts";
import { signChallenge, type DeviceIdentity, type IdentityStore } from "./identity.ts";
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

interface Peer {
  onFrame(frame: unknown): void;
  onClose(code: number, reason: string): void;
}

/** One link's authentication: the challenge it waits for, the hello it signs, the welcome it pins. */
class Handshake {
  readonly challenge: Promise<Challenge>;
  socket: SocketLike | null = null;
  private received: Challenge | null = null;
  private helloReq: number | null = null;
  private welcomed = false;
  private challenged: (challenge: Challenge) => void = () => {};
  private refused: (error: Error) => void = () => {};
  private readonly identity: DeviceIdentity & { deviceId: string };
  private readonly store: IdentityStore;
  private readonly peer: Peer;

  constructor(identity: DeviceIdentity & { deviceId: string }, store: IdentityStore, peer: Peer) {
    this.identity = identity;
    this.store = store;
    this.peer = peer;
    this.challenge = new Promise<Challenge>((resolve, reject) => { this.challenged = resolve; this.refused = reject; });
  }

  refuse(error: Error): void {
    this.refused(error);
  }

  onText = (raw: unknown): void => {
    try {
      const frame = parseFrame(raw);
      if (this.received === null) return this.onChallenge(frame);
      if (frame.frame === "challenge") throw new Error("Unexpected repeated authentication challenge");
      if (!this.welcomed && frame.frame === "reply" && frame.id === this.helloReq) this.onWelcome(frame, this.received);
      this.peer.onFrame(frame);
    } catch (error) {
      this.refused(error instanceof Error ? error : new Error(errorText(error)));
      this.fail(`Protocol error: ${errorText(error)}`);
    }
  };

  onClose = (code: number, reason: string): void => {
    this.refused(new DaemonError("disconnected", `Connection closed during authentication (${code})`));
    this.peer.onClose(code, reason);
  };

  /** Adds the device proof to the link's single hello. */
  hello(command: Json, signature: string): Json {
    invariant(this.helloReq === null, "hello is sent once per link");
    invariant(Number.isSafeInteger(command.req), "hello carries a request id");
    this.helloReq = Number(command.req);
    return { ...command, device: { id: this.identity.deviceId, signature } };
  }

  private onChallenge(frame: Json): void {
    if (frame.frame !== "challenge") throw new Error("Expected an authentication challenge before hello");
    const received = { instance: string(frame.instance_id, "challenge.instance_id"), nonce: string(frame.nonce, "challenge.nonce") };
    this.received = received;
    this.challenged(received);
  }

  private onWelcome(frame: Json, pinned: Challenge): void {
    this.welcomed = true;
    const device = checkWelcome(frame, this.identity.deviceId, pinned);
    if (!device) return;
    // Pinning the daemon id is what lets a later challenge detect an identity change.
    this.store.save({ ...this.identity, ...device, daemonId: pinned.instance })
      .catch((error: unknown) => this.fail(`Could not save the device identity: ${errorText(error)}`));
  }

  private fail(reason: string): void {
    // Browsers may send only 1000 or 3000-4999; the app still hears 1002.
    this.socket?.close(1000, "Invalid server frame");
    this.peer.onClose(CLOSE_PROTOCOL_ERROR, reason);
  }
}

export function aetherSocketTransport(options: AetherSocketOptions): Transport {
  const factory = options.socket ?? browserSocket;
  return {
    id: "ws-aether",
    async open(target, onFrame, onClose, signal) {
      const url = checkSocketUrl(target, "/ws");
      const identity = await options.identity.load();
      signal?.throwIfAborted();
      const deviceId = identity?.deviceId;
      if (!identity || !deviceId) throw new DaemonError("unpaired", "Pair this browser with your daemon");
      const handshake = new Handshake({ ...identity, deviceId }, options.identity, { onFrame, onClose });
      const socket = (handshake.socket = await openSocket(factory, url, "aether", { onText: handshake.onText, onClose: handshake.onClose }, signal));
      const timer = setTimeout(() => handshake.refuse(new Error("Challenge timed out")), options.challengeTimeoutMs ?? 15_000);
      let abort = () => {};
      const cancelled = new Promise<never>((_, reject) => {
        abort = () => reject(new DOMException("Connection attempt cancelled", "AbortError"));
        signal?.addEventListener("abort", abort, { once: true });
        if (signal?.aborted) abort();
      });
      try {
        const received = await Promise.race([handshake.challenge, cancelled]);
        if (identity.daemonId && identity.daemonId !== received.instance) {
          throw new DaemonError("identity_changed", "Daemon identity changed; verify and pair again");
        }
        const signature = await Promise.race([signChallenge(identity, { ...received, origin: options.origin }), cancelled]);
        return {
          send(frame: unknown) {
            const command = object(frame, "client frame");
            sendFrame(socket, command.cmd === "hello" ? handshake.hello(command, signature) : command);
          },
          close: () => socket.close(1000, "bye"),
        };
      } catch (error) {
        socket.close(1000, "authentication failed");
        throw error;
      } finally {
        clearTimeout(timer);
        signal?.removeEventListener("abort", abort);
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
