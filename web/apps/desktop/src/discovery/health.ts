// The health of one local daemon, for the connection and health panel: one short-lived link,
// one `hello`, the daemon's own version from its welcome. Nothing else is ever sent.

import { statSync } from "node:fs";
import { DaemonLink, type Frame } from "../bridge/daemon-link.ts";

export interface Health {
  readonly reachable: boolean;
  readonly version?: string;
}

export interface ProbeOptions {
  /** The hello frame for this daemon, request id `HELLO_REQ`. */
  readonly hello: Frame;
  readonly timeoutMs: number;
  /** Opens the link; tests hand in an in-memory one. */
  readonly connect?: (socket: string, timeoutMs: number) => Promise<DaemonLink>;
}

export const HELLO_REQ = 1;
export const PROBE_TIMEOUT_MS = 2_000;
const UNREACHABLE: Health = { reachable: false };

/** A socket file is there to try; a missing one is simply not running. */
function socketExists(socket: string): boolean {
  if (socket === "") return false;
  try {
    return statSync(socket).isSocket();
  } catch (error) {
    const code = (error as NodeJS.ErrnoException).code;
    if (code === "ENOENT" || code === "ENOTDIR") return false;
    // The page must not learn the path: IPC hands it only `message`; the cause stays in main.
    throw new Error(`A daemon socket could not be checked (${code ?? "unknown error"})`, { cause: error });
  }
}

/** The daemon's version from an `Ok` welcome, when it reports one as a short plain string. */
function welcomeVersion(reply: Frame): string | undefined {
  const welcome = (reply.result as { Ok?: { data?: unknown } } | undefined)?.Ok?.data;
  const version = (welcome as { daemon_version?: unknown } | null | undefined)?.daemon_version;
  return typeof version === "string" && /^[\x20-\x7e]{1,64}$/.test(version) ? version : undefined;
}

/**
 * Says hello on a fresh link and closes it. A daemon that answers, even with an error such as a
 * protocol mismatch, is reachable; one that is down or silent is not, which is the panel's answer.
 */
export async function probe(socket: string, options: ProbeOptions): Promise<Health> {
  if (options.hello.cmd !== "hello" || options.hello.req !== HELLO_REQ) throw new Error("a probe sends only hello");
  if (!(options.timeoutMs > 0)) throw new Error("a probe has a positive deadline");
  if (!socketExists(socket)) return UNREACHABLE;
  const connect = options.connect ?? ((path, ms) => DaemonLink.connect(path, ms));
  let link: DaemonLink;
  try {
    link = await connect(socket, options.timeoutMs);
  } catch {
    return UNREACHABLE;
  }
  try {
    const reply = await link.request(JSON.stringify(options.hello), HELLO_REQ, options.timeoutMs);
    const version = welcomeVersion(reply);
    return version === undefined ? { reachable: true } : { reachable: true, version };
  } catch {
    return UNREACHABLE;
  } finally {
    link.close();
  }
}
