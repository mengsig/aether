import { describe, expect, it } from "vitest";
import { KeyedSerial } from "./debounce.ts";

describe("KeyedSerial", () => {
  it("runs a key's tasks in order and keeps going after one fails", async () => {
    const serial = new KeyedSerial();
    const order: string[] = [];
    const failing = serial.run("k", async () => { order.push("a"); throw new Error("boom"); });
    const next = serial.run("k", async () => { order.push("b"); });
    await expect(failing).rejects.toThrow("boom");
    await next;
    expect(order).toEqual(["a", "b"]);
  });
});
