// Folder picker tokens: the page never sees or names a path to grant. Main opens the native
// picker and hands back an opaque token; main code later redeems it, once, for the path the
// user chose, for the same purpose, within a few minutes. Anything else is refused.

import { randomBytes } from "node:crypto";
import { statSync } from "node:fs";
import path from "node:path";
import { BridgeRefusal } from "../security/validate.ts";

export const TOKEN_TTL_MS = 5 * 60_000;
const MAX_LIVE = 16;
const PURPOSE = /^[a-z][a-z0-9._-]{0,63}$/;

interface Grant {
  readonly folder: string;
  readonly purpose: string;
  readonly expires: number;
}

/** A purpose is a short lowercase name such as `workspace`; it is never shown as text. */
export function checkPurpose(purpose: string): string {
  if (typeof purpose !== "string" || !PURPOSE.test(purpose)) throw new BridgeRefusal("A folder purpose is a short lowercase name");
  return purpose;
}

export class FolderTokens {
  private readonly grants = new Map<string, Grant>();
  private readonly now: () => number;

  constructor(now: () => number = Date.now) {
    this.now = now;
  }

  /** A fresh token for `folder`, the absolute path the user picked, bound to `purpose`. */
  issue(folder: string, purpose: string): string {
    if (!path.isAbsolute(folder)) throw new Error(`a picked folder is absolute, not ${folder}`);
    checkPurpose(purpose);
    this.prune();
    // Map order is issue order: past the cap, the oldest grant lapses first.
    while (this.grants.size >= MAX_LIVE) this.grants.delete(this.grants.keys().next().value as string);
    const token = randomBytes(32).toString("base64url");
    if (this.grants.has(token)) throw new Error("tokens are unique");
    this.grants.set(token, { folder, purpose, expires: this.now() + TOKEN_TTL_MS });
    return token;
  }

  /** The folder `token` grants for `purpose`. Any attempt uses the token up. */
  redeem(token: string, purpose: string): string {
    const grant = this.grants.get(token);
    this.grants.delete(token);
    if (grant === undefined) throw new BridgeRefusal("That folder choice was already used or never made");
    if (this.now() >= grant.expires) throw new BridgeRefusal("That folder choice expired; choose the folder again");
    if (grant.purpose !== purpose) throw new BridgeRefusal("That folder was chosen for something else");
    if (!isDirectory(grant.folder)) throw new BridgeRefusal("The chosen folder is gone");
    return grant.folder;
  }

  get live(): number {
    this.prune();
    return this.grants.size;
  }

  private prune(): void {
    const at = this.now();
    for (const [token, grant] of this.grants) if (at >= grant.expires) this.grants.delete(token);
  }
}

function isDirectory(folder: string): boolean {
  try {
    return statSync(folder).isDirectory();
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return false;
    throw new Error(`Checking the chosen folder ${folder}`, { cause: error });
  }
}
