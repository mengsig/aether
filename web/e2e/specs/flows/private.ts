// A stack of a flow's own, beside the suite's shared one, for a flow that must change what the
// later specs rely on (Archon absent, a GitHub stub on PATH). It gets its own hermetic audit.

import assert from "node:assert/strict";
import { startAudit, type Audit } from "../../harness/audit.ts";
import { binariesFromEnv, startStack, type Stack, type StackOptions } from "../../harness/stack.ts";

export interface PrivateStack {
  readonly stack: Stack;
  readonly audit: Audit;
}

export async function startPrivateStack(options: StackOptions = {}): Promise<PrivateStack> {
  const stack = await startStack(await binariesFromEnv(), options);
  // The browser and CLI calls are the shared fixture's clients, which it audits already.
  const audit = startAudit({
    root: () => stack.layout.root,
    binaries: [stack.binaries.aether, stack.binaries.archon],
    daemons: () => [stack.aether.pid, stack.archon.pid].filter((pid): pid is number => pid !== undefined),
    clients: () => [],
  });
  return { stack, audit };
}

/** Stops the stack, then fails on anything the audit saw while it ran. */
export async function stopPrivateStack(running: PrivateStack | undefined): Promise<void> {
  if (running === undefined) return;
  const { stack, audit } = running;
  try {
    await stack.stop();
  } finally {
    audit.stop();
  }
  assert.ok(audit.samples > 0, "the private stack's audit sampled at least once");
  if (audit.violations.length > 0) throw new Error(`The hermetic audit of a private stack saw:\n${audit.violations.join("\n")}`);
}
