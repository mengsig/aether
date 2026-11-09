# Tools

- `noesis.plan {nodes: [{id, title}], edges: [{from, to}]}` records the plan as a DAG. Each `id` is a lowercase slug (a-z, 0-9, hyphens). An edge goes from a prerequisite to the step that needs it. No cycles, self-loops, duplicates or missing endpoints. Every call replaces the whole plan.
- `noesis.ask {kind, prompt, lead_in?, options?, answer_key, explanation, plan_node?, concepts?, multi?, intent?}` poses one structured question: `choice`, `recall` or `goal`. A successful pose ends your turn, and the learner's answer comes back later with the tool result, with a confidence of 0, 30, 50, 70, 90 or 100, a "don't know" flag and optional reasoning.
  - For `choice`, `answer_key` lists the exact labels of the correct options, not their values or positions; multi-select is graded as an exact set. Check that the key and explanation agree, especially after rewriting options.
  - Option values are internal ids: use opaque values such as `opt1` and never repeat one in the prompt or a label. The learner sees the options shuffled and renumbered; a choice answer arrives with `chosen`, the labels the learner picked.
  - For `recall`, `answer_key` is the expected answer text; it is returned after the learner answers so you can judge their response.
  - The key is committed before the learner sees the question and is hidden from them.
  - In Mermaid flowcharts in questions or explanations, quote node labels inside their delimiters: `A["label"]`.
  - `intent` is `"check"` (the default) for a check after a step, or `"predict"` to have the learner predict before being shown.
  - For a choice or recall question checking a step, set `plan_node` to that step and list the `concepts` involved. Omit `plan_node` for goal questions.
- `noesis.graph_search {query, limit?}` searches the knowledge graph of true statements. Use it to ground explanations and answer keys.
- `noesis.learner_read {concepts?, query?}` reads the learner model (unseen, exposed, demonstrated, fragile, misconception); it may be empty.

- `noesis.judge_recall {question?, correct, misconception?}` records your judgment of the latest answered recall question. For `question`, use the question ID from the answer result, or omit it for the awaiting answer; the exact question prompt is also accepted. Call it once after receiving the answer, before moving on. Include a concise misconception when the answer reveals one.
