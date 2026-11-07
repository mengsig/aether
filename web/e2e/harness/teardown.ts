// Teardown that tries every step: one failed step must not leave a later process or root behind.

import assert from "node:assert/strict";

/** Runs every step in order, then reports each one that failed. */
export async function runAll(steps: ReadonlyArray<() => Promise<unknown>>): Promise<void> {
  assert.ok(steps.length > 0, "a teardown has steps");
  const failures: unknown[] = [];
  for (const step of steps) await step().catch((error: unknown) => failures.push(error));
  if (failures.length > 0) throw new AggregateError(failures, "the teardown failed");
}

/** Tears a failed launch down and throws `message` caused by `error`, or both errors if teardown fails too. */
export async function failLaunch(error: unknown, message: string, teardown: () => Promise<void>): Promise<never> {
  assert.ok(message !== "", "a failed launch says what failed");
  try {
    await teardown();
  } catch (failure) {
    throw new AggregateError([error, failure], `${message}, and its teardown failed`, { cause: failure });
  }
  throw new Error(message, { cause: error });
}
