// @vitest-environment happy-dom
import mermaid from "mermaid";
import { expect, test } from "vitest";
import { diagramRefusal } from "./mermaid-policy.ts";

test("quoted flowchart labels parse the reported formula without relaxing source policy", async () => {
  const original = "flowchart TD\nP --> PF[P = 2(l + w); linear units]";
  const corrected = 'flowchart TD\nP --> PF["P = 2(l + w); linear units"]';
  expect(diagramRefusal(original)).toBeNull();
  await expect(mermaid.parse(original)).rejects.toThrow();
  expect(diagramRefusal(corrected)).toBeNull();
  await expect(mermaid.parse(corrected)).resolves.toMatchObject({ diagramType: "flowchart-v2" });
  expect(diagramRefusal('flowchart TD\nP --> PF["<script>alert(1)</script>"]')).toContain("HTML");
  expect(diagramRefusal('flowchart TD\nP --> PF["label"]; click PF "javascript:alert(1)"')).toContain("disabled");
});

