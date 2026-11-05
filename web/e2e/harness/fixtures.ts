// Playwright fixtures over the harness. One stack per worker (the suite runs one worker), with
// the hermetic audit sampling for its whole life; each test gets a fresh, unpaired page.

import { test as base, expect, type Page } from "@playwright/test";
import { startAudit, type Audit } from "./audit.ts";
import { binariesFromEnv, startStack, type Stack } from "./stack.ts";

interface WorkerFixtures {
  stack: Stack;
  audit: Audit;
}

interface TestFixtures {
  /** Page errors seen so far; a test that ends with any fails. */
  pageErrors: string[];
}

export const test = base.extend<TestFixtures, WorkerFixtures>({
  // Playwright reads fixture dependencies from the destructured first argument; this has none.
  // eslint-disable-next-line no-empty-pattern
  stack: [async ({}, provide) => {
    const stack = await startStack(await binariesFromEnv());
    try {
      await provide(stack);
    } finally {
      await stack.stop();
    }
  }, { scope: "worker", timeout: 120_000 }],

  audit: [async ({ stack }, provide) => {
    const audit = startAudit({
      root: () => stack.layout.root,
      daemons: () => [stack.aether.pid, stack.archon.pid].filter((pid): pid is number => pid !== undefined),
      // The worker and all it spawned: the browser, the faux engine's server, the CLI calls.
      clients: () => [process.pid],
    });
    try {
      await provide(audit);
    } finally {
      audit.stop();
    }
    if (audit.violations.length > 0) throw new Error(`The hermetic audit saw:\n${audit.violations.join("\n")}`);
  }, { scope: "worker", auto: true }],

  pageErrors: [async ({ page }, provide) => {
    const errors: string[] = [];
    page.on("pageerror", (error) => errors.push(error.message));
    page.on("console", (message) => { if (/content.security.policy|violat/i.test(message.text())) errors.push(message.text()); });
    await provide(errors);
    expect(errors, "no page or CSP errors").toEqual([]);
  }, { auto: true }],
});

export { expect };
export type { Page };
