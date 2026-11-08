import { strict as assert } from "node:assert";
import { test } from "node:test";
import { startSingleInstance } from "../../src/main/single-instance.ts";

test("a second instance quits without starting the desktop", () => {
  const events: string[] = [];
  startSingleInstance({ requestSingleInstanceLock: () => false, quit: () => { events.push("quit"); } },
    () => { events.push("start"); });
  assert.deepEqual(events, ["quit"]);
});

test("the primary instance starts without quitting", () => {
  const events: string[] = [];
  startSingleInstance({ requestSingleInstanceLock: () => true, quit: () => { events.push("quit"); } },
    () => { events.push("start"); });
  assert.deepEqual(events, ["start"]);
});
