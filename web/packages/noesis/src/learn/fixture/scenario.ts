// The learning scenario the tests and the screenshot page share: a learner studying how Raft elects
// a leader, with one session mid-lesson, one in practice, a small knowledge graph and every mastery state.

import type { aether } from "@aether-ui/protocol";
import type { FauxNoesisOptions, HeldQuestion } from "./faux-noesis.ts";

const usage = (cost: number, tokens: number): aether.Usage => ({
  input_tokens: tokens, output_tokens: Math.round(tokens / 10), cache_read_tokens: 0, cache_write_tokens: 0, cost_usd: cost, cost_complete: true,
});

export const SETTINGS: aether.NoesisSettingsView = {
  enabled: true, engineering_context: false,
  billing: { allow_api: true },
  budget: { interactive_usd_daily: 5, background_usd_daily: 2, background_tokens_daily: 2_000_000 },
  models: { strong: "anthropic/claude-opus-5-5", cheap: "anthropic/claude-haiku-5-5" },
  extraction: { sessions: "manual", engineering: "off" },
  concurrency: { interactive: 2, background: 1 },
  disable: { grace_seconds: 30 },
  priority: { background: "low" },
  prompts: { overrides: false },
} as aether.NoesisSettingsView;

export function statusOf(enabled: boolean): aether.NoesisStatus {
  const settings = { ...SETTINGS, enabled };
  return {
    enabled, settings, effective: settings, jobs_running: 0, prompt_overrides: [],
    disable_policy: { background: "cancel_at_model_boundary", grace_seconds: 30 },
    spend_today: { interactive: usage(1.85, 420_000), background: usage(0.4, 610_000) },
  };
}

const text = (role: aether.MessageRole, value: string): aether.Message => ({ role, content: [{ type: "text", text: value }] });
const at = (minute: number): string => new Date(Date.UTC(2026, 10, 4, 8, minute, 0)).toISOString();

export const PLAN: aether.PlanTree = {
  revision: 3,
  nodes: [
    { id: "pn_terms", title: "Terms and heartbeats", status: "visited" },
    { id: "pn_timeout", title: "Randomised election timeouts", status: "visited" },
    { id: "pn_vote", title: "Who grants a vote", status: "current" },
    { id: "pn_log", title: "The up-to-date log rule", status: "upcoming" },
    { id: "pn_split", title: "Split votes and retries", status: "upcoming" },
  ],
  edges: [{ from: "pn_terms", to: "pn_timeout" }, { from: "pn_timeout", to: "pn_vote" }, { from: "pn_vote", to: "pn_log" }, { from: "pn_timeout", to: "pn_split" }],
};

export const VOTE_QUESTION: HeldQuestion = {
  question: {
    id: "q_vote", kind: "choice", multi: false, collapsed: false, concepts: ["c_vote"], plan_node: "pn_vote", posed_at: at(14),
    prompt: "A follower in term 7 gets a RequestVote for term 7 and has already voted for another candidate in term 7. What does it do?",
    options: [
      { value: "a", label: "Grants the vote, since the terms match" },
      { value: "b", label: "Refuses: one vote per term" },
      { value: "c", label: "Starts its own election" },
    ],
  },
  key: ["b"],
  explanation: "A server votes at most once per term, first come first served. That is what keeps two leaders from winning the same term.",
};

export const RECALL_QUESTION: HeldQuestion = {
  question: { id: "q_term", kind: "recall", multi: false, collapsed: false, concepts: ["c_term"], posed_at: at(16), prompt: "In one sentence: what does a server do when it sees a higher term?" },
  key: "It adopts the term and becomes a follower.",
  explanation: "A higher term means its own view is stale, so it steps down.",
};

export const MULTI_QUESTION: HeldQuestion = {
  question: {
    id: "q_safe", kind: "choice", multi: true, collapsed: false, concepts: ["c_log"], posed_at: at(18),
    prompt: "Which of these does a voter compare before granting a vote?",
    options: [{ value: "t", label: "The last log entry's term" }, { value: "i", label: "The last log index" }, { value: "n", label: "The candidate's node id" }],
  },
  key: ["i", "t"],
  explanation: "The voter checks that the candidate's log is at least as up to date: last term first, then last index.",
};

export const GOAL_QUESTION: HeldQuestion = {
  question: { id: "q_goal", kind: "goal", multi: false, collapsed: false, concepts: [], posed_at: at(1), prompt: "What do you want to be able to do when we finish?" },
  key: [],
  explanation: "",
};

export const LESSON: aether.NoesisSession = {
  id: "ns_raft", agent: "agt_noesis_raft", goal: "Understand how Raft elects a leader", mode: "teach", phase: "teach",
  created_at: at(0), updated_at: at(14), plan: PLAN, open_question: null,
};

export const PRACTICE: aether.NoesisSession = {
  id: "ns_drill", agent: "agt_noesis_drill", goal: "Practise reasoning about split votes", mode: "practice", phase: "practice",
  created_at: at(30), updated_at: at(40), active_problem: {
    id: "pb_split", mode: "composition", posed_at: at(40), targets: ["c_vote", "c_timeout"],
    statement: "Five servers, two candidates start an election in the same term and each gets two votes. Walk through what happens next, and why it ends.",
  },
};

export const EARLIER: aether.NoesisSession = {
  id: "ns_logs", agent: "agt_noesis_logs", goal: "Log replication basics", mode: "teach", phase: "plan",
  created_at: "2026-11-02T15:00:00Z", updated_at: "2026-11-02T15:40:00Z",
};

export const LESSON_TRANSCRIPT: readonly aether.Message[] = [
  text("user", "I want to understand how Raft picks a leader."),
  text("assistant", "Good place to start. Every server is a follower, a candidate or the leader, and time is cut into numbered terms. Followers expect a heartbeat from the leader; when one stops arriving, a follower stands for election."),
  text("user", "Why randomise the timeout?"),
  text("assistant", "So that one follower usually times out first and wins before anyone else starts. Next: who is allowed to grant that follower a vote."),
];

const proposition = (id: string, name: string, statement: string, source = "ns_raft"): aether.Proposition => ({
  id, name, statement, aliases: [], created_at: at(20), provenance: [{ source }],
});

export const GRAPH = {
  nodes: [
    proposition("p_term", "Terms", "Time is divided into numbered terms, each with at most one leader."),
    proposition("p_heartbeat", "Heartbeats", "A leader sends heartbeats so followers do not start elections."),
    proposition("p_timeout", "Election timeout", "A follower that hears nothing for its timeout becomes a candidate."),
    proposition("p_random", "Randomised timeouts", "Timeouts are randomised so one server usually starts first."),
    proposition("p_vote", "One vote per term", "A server grants at most one vote per term."),
    proposition("p_majority", "Majority wins", "A candidate with votes from a majority becomes leader."),
    proposition("p_one_leader", "Election safety", "At most one leader can be elected in a term."),
    proposition("p_uptodate", "Up-to-date log rule", "A voter refuses candidates whose log is less up to date than its own.", "s_notes"),
    proposition("p_step_down", "Higher term wins", "A server that sees a higher term becomes a follower."),
  ],
  chains: [
    { id: "ch_1", premises: ["p_vote", "p_majority"], conclusion: "p_one_leader", created_at: at(20), provenance: [] },
    { id: "ch_2", premises: ["p_heartbeat", "p_timeout"], conclusion: "p_random", created_at: at(20), provenance: [] },
    { id: "ch_3", premises: ["p_term"], conclusion: "p_step_down", created_at: at(20), provenance: [] },
    { id: "ch_4", premises: ["p_term", "p_timeout"], conclusion: "p_majority", created_at: at(20), provenance: [] },
    { id: "ch_5", premises: ["p_uptodate", "p_vote"], conclusion: "p_majority", created_at: at(20), provenance: [] },
  ],
};

const concept = (ref: string, title: string, state: aether.ConceptStateKind, strength: number, extra: Partial<aether.ConceptState> = {}): aether.ConceptState => ({
  concept: { id: `c_${ref}`, kind: "proposition", ref, title }, state, strength, as_of_evidence: 3, misconceptions: [], ...extra,
});

export const CONCEPTS: readonly aether.ConceptState[] = [
  concept("p_term", "Terms", "demonstrated", 0.92, { review_due_at: "2026-11-09T09:00:00Z", calibration: 0.08 }),
  concept("p_heartbeat", "Heartbeats", "demonstrated", 0.81, { calibration: 0.12 }),
  concept("p_random", "Randomised timeouts", "fragile", 0.46, { review_due_at: "2026-11-05T09:00:00Z" }),
  concept("p_vote", "One vote per term", "misconception", 0.2, { misconceptions: ["Thinks a server can vote again when the terms match"] }),
  concept("p_majority", "Majority wins", "exposed", 0.3),
  concept("p_timeout", "Election timeout", "exposed", 0.35),
  concept("p_uptodate", "Up-to-date log rule", "unseen", 0),
];

/** The whole scenario with Noesis on or off. */
export function scenario(enabled: boolean, extra: Partial<FauxNoesisOptions> = {}): FauxNoesisOptions {
  return {
    status: statusOf(enabled),
    sessions: [LESSON, PRACTICE, EARLIER],
    transcripts: { [LESSON.agent]: LESSON_TRANSCRIPT, [PRACTICE.agent]: [text("user", "Give me a problem on split votes.")] },
    graph: GRAPH,
    concepts: CONCEPTS,
    ...extra,
  };
}
