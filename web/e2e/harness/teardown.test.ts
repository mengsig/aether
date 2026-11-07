import { describe, expect, it } from "vitest";
import { failLaunch, runAll } from "./teardown.ts";

describe("runAll", () => {
  it("runs every step after one fails and keeps each failure", async () => {
    const ran: string[] = [];
    const step = (name: string, fails: boolean) => async () => {
      ran.push(name);
      if (fails) throw new Error(name);
    };
    const thrown = await runAll([step("browser", true), step("app", false), step("sway", true), step("root", false)])
      .then(() => undefined, (error: unknown) => error);
    expect(ran).toEqual(["browser", "app", "sway", "root"]);
    expect(thrown).toBeInstanceOf(AggregateError);
    expect((thrown as AggregateError).errors.map((error: Error) => error.message)).toEqual(["browser", "sway"]);
  });

  it("resolves when every step succeeds", async () => {
    await expect(runAll([async () => undefined, async () => 1])).resolves.toBeUndefined();
  });
});

describe("failLaunch", () => {
  it("reports the launch error as the cause after a clean teardown", async () => {
    const launch = new Error("no DevTools port");
    let tornDown = false;
    const thrown = await failLaunch(launch, "the app did not start", async () => {
      tornDown = true;
    }).catch((error: unknown) => error);
    expect(tornDown).toBe(true);
    expect((thrown as Error).message).toBe("the app did not start");
    expect((thrown as Error).cause).toBe(launch);
  });

  it("reports both the launch and the teardown errors when teardown fails", async () => {
    const launch = new Error("no Wayland socket");
    const teardown = new Error("sway did not stop");
    const thrown = await failLaunch(launch, "the app did not start", () => Promise.reject(teardown)).catch((error: unknown) => error);
    expect(thrown).toBeInstanceOf(AggregateError);
    expect((thrown as AggregateError).errors).toEqual([launch, teardown]);
    expect((thrown as AggregateError).message).toMatch(/did not start, and its teardown failed/);
  });
});
