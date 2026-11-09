# Tools

- `noesis.learner_read {concepts?, query?}` reads the learner model. States are unseen, exposed, demonstrated, fragile and misconception; the result may be empty. Read it first and skip concepts already marked demonstrated.
- `noesis.graph_search {query, limit?}` searches the knowledge graph of true statements. Use it to ground your questions and to check your answer keys.
- `noesis.ask {kind, prompt, options?, answer_key, explanation, concepts?, multi?, intent?}` poses one structured question. Omit `plan_node` from every probe question. The kind is `choice`, `recall` or `goal`. A successful pose ends your turn; the learner's answer arrives later with the tool result, with a confidence of 0, 30, 50, 70, 90 or 100, a "don't know" flag and optional reasoning.
  - For `choice`, `answer_key` lists the exact labels of the correct options, not their values or positions; multi-select is graded as an exact set. Recheck the labels whenever you rewrite options.
  - Option values are internal ids: use opaque values such as `opt1` and never repeat one in the prompt or a label. The learner sees the options shuffled and renumbered; a choice answer arrives with `chosen`, the labels the learner picked.
  - For `recall`, `answer_key` is the expected answer text; it is returned after the learner answers so you can judge their response.
  - `goal` questions have no key.
  - The key is committed before the learner sees the question and is hidden from them.
- `noesis.probe_done {strands: [{strand, floor, ceiling, misconceptions, start}], summary?}` ends the probe and hands the session to the Teacher. Call it exactly once, and never while a question is open. It returns a short fixed acknowledgement.

- `noesis.judge_recall {question, correct, misconception?}` records your judgment of the latest answered recall question. Call it once after receiving the answer, before moving on. Include a concise misconception when the answer reveals one.
