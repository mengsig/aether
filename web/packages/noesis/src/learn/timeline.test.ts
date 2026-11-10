// Which transcript entries Noesis wrote itself, including for a session kept before its history was.

import type { aether } from "@aether-ui/protocol";
import { describe, expect, test } from "vitest";
import { buildTimeline, internalSeqs } from "./timeline.ts";
import { EMPTY_NOESIS } from "@aether-ui/core";
import { LESSON, VOTE_QUESTION } from "./fixture/scenario.ts";

const entry = (seq: number, role: "user" | "assistant", text: string): aether.TranscriptEntry =>
  ({ seq, at: `2026-11-${String(seq).padStart(2, "0")}T09:00:00Z`, message: { role, content: [{ type: "text", text }] } }) as aether.TranscriptEntry;

describe("Noesis's own inputs to an agent", () => {
  test("the seqs the daemon names are the ones held back, and nothing is guessed", () => {
    const agent: aether.SessionAgent = { agent: "agt_t", role: "teacher", internal: [4] };
    const entries = [entry(4, "user", "Prober's assessment"), entry(5, "assistant", "Hello"), entry(6, "user", "My answer")];
    expect([...internalSeqs(agent, entries)]).toEqual([4]);
  });

  test("a legacy agent with no named seqs holds back only its first input, when it came before any reply", () => {
    const legacy: aether.SessionAgent = { agent: "agt_old", role: "teacher", internal: [] };
    const opened = [entry(3, "assistant", "Hi"), entry(2, "user", "Learning goal: Raft"), entry(5, "user", "Why majorities?")];
    expect([...internalSeqs(legacy, opened)]).toEqual([2]);
    expect([...internalSeqs(legacy, [entry(1, "assistant", "Hi"), entry(2, "user", "Mine")])]).toEqual([]);
    expect([...internalSeqs({ agent: "agt_none", role: "prober" }, [])]).toEqual([]);
  });
});

test("a judgment event updates an older ungraded session read", () => {
  const question = { ...VOTE_QUESTION.question, kind: "recall" as const };
  const session = { ...LESSON, open_question: null, questions: [{ question, result: { question: question.id, answer_key: "Paris" } }] };
  const cache = { ...EMPTY_NOESIS, results: { [question.id]: { question: question.id, correct: false, misconception: "Confuses cities" } } };
  const item = buildTimeline(session, [], cache).find((entry) => entry.kind === "question");
  expect(item?.kind === "question" && item.result?.correct).toBe(false);
  expect(item?.kind === "question" && item.result?.misconception).toBe("Confuses cities");
});

test("answer envelopes are hidden while historical and graded question cards remain", () => {
  const agent: aether.SessionAgent = { agent: "agt_t", role: "teacher", internal: [1] };
  const answer = { chosen: ["Refuses"], dont_know: false, confidence: 70, answered_at: "2026-11-04T09:00:00Z" };
  const session = { ...LESSON, open_question: null, questions: [{ question: VOTE_QUESTION.question, answer, result: { question: VOTE_QUESTION.question.id, correct: true } }] };
  const payloads = [
    { question: "q_vote", choices: ["b"], chosen: ["Refuses"], text: null, dont_know: false, confidence: 70, thinking: null, correct: true },
    { question: "q_recall", choices: [], chosen: [], text: "A follower", dont_know: false, confidence: 50, thinking: "reason", answer_key: "Expected" },
    { question: "q_old", choices: [], chosen: [], text: null, dont_know: true, confidence: null, thinking: null },
    { chosen: ["Historical choice"], dont_know: false },
  ];
  const entries = payloads.map((payload, index) => entry(index + 2, "user", `The learner answered: ${JSON.stringify(payload)}`));
  const items = buildTimeline(session, [{ agent, entries }], EMPTY_NOESIS);
  expect(items.filter((item) => item.kind === "message")).toEqual([]);
  expect(items.filter((item) => item.kind === "question")).toMatchObject([{ answer, result: { correct: true } }]);
});

test("ordinary prose, malformed and lookalike envelopes, and teacher messages remain visible", () => {
  const agent: aether.SessionAgent = { agent: "agt_t", role: "teacher", internal: [1] };
  const texts = [
    "My answer is Paris", '{"dont_know":true}', 'The learner answered: {broken',
    'The learner answered: {"topic":"Paris"}', 'The learner answered: {"dont_know":false}',
    'The learner answered: {"dont_know":"true"}', 'The learner answered: {"dont_know":true,"extra":"prose"}',
    'The learner answered: {"dont_know":false,"choices":[4]}',
    ' The learner answered: {"dont_know":true}',
  ];
  const entries = texts.map((text, index) => entry(index + 2, "user", text));
  entries.push(entry(12, "assistant", 'The learner answered: {"dont_know":true}'));
  entries.push({ ...entry(13, "user", ""), message: { role: "user", content: [
    { type: "text", text: 'The learner answered: {"dont_know":true}' }, { type: "text", text: "More context" },
  ] } } as aether.TranscriptEntry);
  const messages = buildTimeline({ ...LESSON, questions: [], open_question: null }, [{ agent, entries }], EMPTY_NOESIS)
    .filter((item) => item.kind === "message");
  expect(messages.map((item) => item.text)).toEqual([...texts.map((text) => text.trim()), 'The learner answered: {"dont_know":true}', 'The learner answered: {"dont_know":true}\n\nMore context']);
  expect(messages.at(-2)?.role).toBe("teacher");
});
