import { afterEach, expect, test, vi } from "vitest";
import { Rpc } from "./rpc.ts";

afterEach(() => vi.useRealTimers());

test("native memory confirmation can outlast the usual timeout and still settle", async () => {
  vi.useFakeTimers();
  const send = vi.fn();
  const rpc = new Rpc({ send, close() {} }, 20_000, true);
  const reply = rpc.request({ cmd: "memory_import", dry_run: false, artifact: "{}" });
  await vi.advanceTimersByTimeAsync(30_000);
  expect(rpc.inFlight).toBe(1);
  expect(send).toHaveBeenCalledWith(expect.objectContaining({ cmd: "memory_import", req: 1 }));
  rpc.settle({ frame: "reply", id: 1, result: { Ok: { data: {} } } });
  await expect(reply).resolves.toEqual({ data: {} });
  expect(rpc.inFlight).toBe(0);
});

test("other requests time out and an open confirmation fails on disconnect", async () => {
  vi.useFakeTimers();
  const rpc = new Rpc({ send() {}, close() {} }, 100, true);
  const confirmation = rpc.request({ cmd: "memory_export" });
  const ordinary = rpc.request({ cmd: "memory_import", dry_run: true, artifact: "{}" });
  const timedOut = expect(ordinary).rejects.toMatchObject({ code: "timeout" });
  await vi.advanceTimersByTimeAsync(100);
  await timedOut;
  expect(rpc.inFlight).toBe(1);
  const disconnected = expect(confirmation).rejects.toMatchObject({ code: "disconnected" });
  rpc.fail(new Error("connection closed"));
  await disconnected;
  expect(rpc.inFlight).toBe(0);
});
