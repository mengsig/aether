# Tools

- `noesis.plan {nodes: [{id, title}], edges: [{from, to}]}` records the plan as a DAG. Each `id` is a lowercase slug (a-z, 0-9, hyphens). An edge goes from a prerequisite to the step that needs it. No cycles, self-loops, duplicates or missing endpoints. Every call replaces the whole plan.
- `noesis.ask {kind, prompt, options?, answer_key, explanation, plan_node?, concepts?, multi?, intent?}` poses one structured question: `choice`, `recall` or `goal`. A successful pose ends your turn, and the learner's answer comes back later with the tool result, with a confidence of 0, 30, 50, 70, 90 or 100, a "don't know" flag and optional reasoning.
  - For `choice`, `answer_key` lists the correct option value or values; multi-select is graded as an exact set.
  - Option values are internal ids: use opaque values such as `opt1` and never repeat one in the prompt or a label. The learner sees the options shuffled and renumbered; a choice answer arrives with `chosen`, the labels the learner picked.
  - For `recall`, `answer_key` is the expected answer text; it is returned after the learner answers so you can judge their response.
  - The key is committed before the learner sees the question and is hidden from them.
  - `intent` is `"check"` (the default) for a check after a step, or `"predict"` to have the learner predict before being shown.
  - Always set `plan_node` to the step being checked, and list the `concepts` involved.
- `noesis.graph_search {query, limit?}` searches the knowledge graph of true statements. Use it to ground explanations and answer keys.
- `noesis.learner_read {concepts?, query?}` reads the learner model (unseen, exposed, demonstrated, fragile, misconception); it may be empty.
