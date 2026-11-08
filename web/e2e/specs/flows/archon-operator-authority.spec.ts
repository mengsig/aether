import { enqueueItem, pauseQueue } from "../../harness/agents.ts";
import { pairBoth } from "../../harness/pair.ts";
import { expect, test } from "../../harness/fixtures.ts";
import { SocketTap } from "./wire.ts";

test("a paired operator cannot grant privileged item autonomy", async ({ page, stack }) => {
  test.setTimeout(150_000);
  await pauseQueue(stack);
  const item = await enqueueItem(stack, "Operator autonomy boundaries");
  const tap = await SocketTap.install(page);
  await pairBoth(page, stack, { aether: "operator", archon: "operator" }, "Autonomy boundary journey");
  const before = await stack.archon.request({ cmd: "get_item", item });

  for (const autonomy of [{ merge: "auto" }, { tools: "yolo" }]) {
    for (const frame of [
      { cmd: "update_item", item, patch: { autonomy } },
      { cmd: "steer_item", item, steer: { autonomy } },
    ]) {
      const result = await tap.send("archon", frame);
      expect(result.ok, JSON.stringify(frame)).toBeUndefined();
      expect(JSON.stringify(result.err)).toContain("a paired device cannot set merge=auto or tools=yolo");
    }
  }
  const after = await stack.archon.request({ cmd: "get_item", item });
  expect(after.data.item).toEqual(before.data.item);
});
