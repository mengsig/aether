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
    const scripted = { name: "complete_task", arguments: { summary: "done", structured: { answer: 42 } }, text: "Working on it." };
    for (const reply of [scripted, { toolCall: scripted }]) {
      engine.setScript(() => reply);
      const tool = await request();
      const deltas = tool.flatMap((frame) => frame.choices[0].delta.tool_calls ?? []);
      assert.ok(deltas.length > 1, "the call is streamed in chunks");
      assert.match(deltas[0].id, /^call_/);
      assert.equal(deltas[0].function.name, "complete_task");
      assert.deepEqual(JSON.parse(deltas.map((delta) => delta.function.arguments).join("")), scripted.arguments);
      assert.equal(tool.map((frame) => frame.choices[0].delta.content ?? "").join(""), scripted.text);
      assert.equal(tool.at(-1).choices[0].finish_reason, "tool_calls");
    }
    assert.deepEqual(engine.requests.map((seen) => seen.lastUserText), ["hello", "hello", "hello"]);
  } finally {
    await engine.close();
  }
});
