import assert from "node:assert/strict";
import { test } from "node:test";
import { startEngine } from "./engine.ts";

test("the faux engine streams text and scripted tool calls", async () => {
  const engine = await startEngine();
  try {
    const request = async () => {
      const response = await fetch(`${engine.baseUrl}/chat/completions`, {
        method: "POST", headers: { "content-type": "application/json" },
        body: JSON.stringify({ model: "mock-1", messages: [{ role: "user", content: "hello" }] }),
      });
      assert.equal(response.status, 200);
      return (await response.text()).split("\n\n").filter((frame) => frame.startsWith("data: {")).map((frame) => JSON.parse(frame.slice(6)));
    };
    const text = await request();
    assert.equal(text.at(-1).choices[0].finish_reason, "stop");
    engine.setScript(() => ({ name: "complete_task", arguments: { summary: "done", structured: { answer: 42 } } }));
    const tool = await request();
    const call = tool.find((frame) => frame.choices[0].delta.tool_calls)?.choices[0].delta.tool_calls[0];
    assert.match(call.id, /^call_/);
    assert.equal(call.function.name, "complete_task");
    assert.deepEqual(JSON.parse(call.function.arguments), { summary: "done", structured: { answer: 42 } });
    assert.equal(tool.at(-1).choices[0].finish_reason, "tool_calls");
    assert.deepEqual(engine.requests.map((seen) => seen.lastUserText), ["hello", "hello"]);
  } finally {
    await engine.close();
  }
});
