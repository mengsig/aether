import { createHash } from "node:crypto";
import type { Frame } from "../bridge/daemon-link.ts";

export class MemoryExports {
  private readonly exports = new Map<number, { digest: string; artifact: string }>();

  remember(owner: number, reply: Frame): void {
    this.forget(owner);
    const result = reply.result as { Ok?: { reply?: unknown; data?: { digest?: unknown; artifact?: unknown } } } | undefined;
    const data = result?.Ok?.data;
    if (result?.Ok?.reply !== "memory_export" || typeof data?.artifact !== "string" || typeof data.digest !== "string") return;
    if (!/^[0-9a-f]{64}$/.test(data.digest)) return;
    if (createHash("sha256").update(data.artifact, "utf8").digest("hex") !== data.digest) return;
    this.exports.set(owner, { digest: data.digest, artifact: data.artifact });
  }

  get(owner: number, digest: string): string {
    const entry = this.exports.get(owner);
    if (entry === undefined || entry.digest !== digest) throw new Error("No matching memory export is available in this window");
    return entry.artifact;
  }

  forget(owner: number): void {
    this.exports.delete(owner);
  }
}
