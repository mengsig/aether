// A small demo queue for tests and screenshots: every state, an operator gate with its question, a steered
// run, and GitHub links whose issue, PR and execution states differ. Nothing here names real work.

type Json = Record<string, unknown>;

export interface FauxItem {
  summary: Json;
  detail: Json;
  links: Json;
  artifacts: Json[];
}

export interface World {
  items: Map<string, FauxItem>;
  questions: Json[];
  reports: Json[];
  repos: Json[];
  plans: Map<string, Json>;
  pause: { global: boolean; repos: string[]; pipelines: string[] };
  said: string[];
}

const T0 = Date.parse("2026-11-04T09:00:00Z");
const at = (minutes: number): string => new Date(T0 + minutes * 60_000).toISOString();
const ref = (repo: string, number: number, kind: "issue" | "pull_request") => ({ host: "github.com", repo, number, kind });

const STAGES = [
  { id: "plan", kind: "work", gate: "none", fan_out: 1, max_loops: 0, timeout_minutes: 30 },
  { id: "build", kind: "work", gate: "none", fan_out: 1, max_loops: 2, timeout_minutes: 120, effort: "high" },
  { id: "review", kind: "review", gate: "all_pass", fan_out: 2, loops_to: "build", max_loops: 2, timeout_minutes: 60 },
  { id: "ship", kind: "ship", gate: "none", fan_out: 1, max_loops: 0, timeout_minutes: 20 },
];

function stageRun(run: string, stage: string, state: Json, minutes: number, extra: Json = {}): Json {
  return {
    id: `sr_${run}_${stage}_${String(extra.fan_index ?? 0)}`, run, stage_id: stage, attempt: 1, fan_index: 0, state, cost_usd: 0.42,
    started_at: at(minutes), finished_at: state.state === "running" ? null : at(minutes + 6),
    choice: { provider: "codex", model: "gpt-6-astra", effort: "medium" }, ...extra,
  };
}

interface Spec {
  id: string; title: string; state: Json; repo: string | null; pipeline: string; priority: string; minutes: number; spent: number;
  stage?: string; waitingReason?: Json; waitingOn?: Json; issue?: Json; pr?: Json; deps?: string[]; stages?: Json[]; runs?: Json[]; gates?: Json[];
}

function item(spec: Spec): FauxItem {
  const summary: Json = {
    id: spec.id, title: spec.title, brief: `${spec.title}.\n\nAcceptance: the change is tested and documented.`, state: spec.state,
    repo: spec.repo, pipeline: spec.pipeline, priority: spec.priority, created_at: at(spec.minutes - 30), updated_at: at(spec.minutes),
    dependencies: spec.deps ?? [], tags: [], model_plan: {}, spent_usd: spec.spent, autonomy: { review: "standard" },
    stage: spec.stage ?? null, waiting_reason: spec.waitingReason ?? null, waiting_on: spec.waitingOn ?? null,
    issue: spec.issue ?? null, pr: spec.pr ?? null, pr_url: spec.pr ? `https://github.com/${String(spec.pr.repo)}/pull/${String(spec.pr.number)}` : null,
  };
  const summaryOnly = new Set(["spent_usd", "stage", "waiting_reason", "waiting_on", "issue", "pr", "pr_url"]);
  const queueItem = Object.fromEntries(Object.entries(summary).filter(([key]) => !summaryOnly.has(key)));
  const detail: Json = {
    item: queueItem, runs: spec.runs ?? [], stages: spec.stages ?? [], gates: spec.gates ?? [],
    spent_usd: summary.spent_usd, waiting_reason: summary.waiting_reason, waiting_on: summary.waiting_on,
  };
  const links: Json[] = [];
  const objects: Json[] = [];
  if (spec.issue) {
    links.push({ item: spec.id, ref: spec.issue, relation: "origin", source: "watcher", created_at: at(spec.minutes - 30) });
    objects.push({ ref: spec.issue, title: `Issue: ${spec.title}`, state: "open", labels: ["archon"], updated_at: at(spec.minutes), fetched_at: at(spec.minutes) });
  }
  if (spec.pr) {
    links.push({ item: spec.id, ref: { host: "github.com", repo: spec.pr.repo, number: spec.pr.number, kind: "pull_request" }, relation: "delivered", source: "ship", created_at: at(spec.minutes) });
  }
  const report = `# ${spec.title}\n\nAll checks passed on the final head.\n`;
  const artifacts = spec.runs?.length ? [{ name: "report.md", bytes: report.length, text: report }] : [];
  return { summary, detail, links: { links, objects }, artifacts };
}

function run(id: string, itemId: string, state: Json, minutes: number, cost: number): Json {
  return { id, item: itemId, pipeline: "adaptive", branch: `archon/${itemId}`, workspace: `/work/${itemId}`, cost_usd: cost, state, started_at: at(minutes), finished_at: null };
}

export function demoWorld(): World {
  const gateQuestion = "q_review_gate";
  const items: FauxItem[] = [
    item({
      id: "qi_dark_mode", title: "Ship dark mode for the docs", state: { state: "running", run: "run_dark" }, repo: "repo_site", pipeline: "adaptive",
      priority: "high", minutes: 50, spent: 3.18, stage: "review", issue: ref("acme/site", 41, "issue"),
      pr: { ...ref("acme/site", 57, "pull_request"), state: "draft", checks: { state: "pending", total: 6, failed: 0 } },
      runs: [run("run_dark", "qi_dark_mode", { state: "running", stage_id: "review", attempt: 1 }, 20, 3.18)],
      stages: [
        stageRun("run_dark", "plan", { state: "completed" }, 20, { verdict: "pass" }),
        stageRun("run_dark", "build", { state: "completed" }, 27, { summary: "Added a theme toggle and tokens for both themes." }),
        stageRun("run_dark", "review", { state: "running" }, 40),
        stageRun("run_dark", "review", { state: "completed" }, 40, { fan_index: 1, verdict: "pass" }),
      ],
    }),
    item({
      id: "qi_pairing", title: "Shorten the pairing code", state: { state: "running", run: "run_pair" }, repo: "repo_api", pipeline: "adaptive",
      priority: "normal", minutes: 45, spent: 1.07, stage: "plan", waitingReason: { code: "operator_gate", message: "Waiting on your gate decision for the proposal", next_action: "answer" },
      waitingOn: { subject: "Proposal", prompt: "Approve the proposal to shorten pairing codes to eight characters?", stage: "plan", next_action: "answer", question: gateQuestion },
      runs: [run("run_pair", "qi_pairing", { state: "awaiting_operator", stage_id: "plan", attempt: 1, subject: "proposal" }, 35, 1.07)],
      stages: [stageRun("run_pair", "plan", { state: "completed" }, 35, { verdict: "pass", summary: "Proposes 8-character codes." })],
      gates: [{ item: "qi_pairing", run: "run_pair", stage_id: "plan", attempt: 1, subject: "proposal", summary: "Shorter codes", opened_at: at(41), question: gateQuestion }],
    }),
    item({
      id: "qi_landing", title: "Refresh the landing page copy", state: { state: "blocked", reason: "Dependency qi_assets failed" }, repo: "repo_site",
      pipeline: "direct", priority: "normal", minutes: 30, spent: 0, deps: ["qi_assets"],
      waitingReason: { code: "dependency_failed", message: "Dependency qi_assets failed", next_action: "retry" },
    }),
    item({
      id: "qi_rate_limit", title: "Rate-limit the claim endpoint", state: { state: "queued" }, repo: "repo_api", pipeline: "adaptive", priority: "critical",
      minutes: 25, spent: 0, waitingReason: { code: "capacity", message: "Waiting for a free agent slot (4 of 4 busy)", next_action: "wait" },
    }),
    item({
      id: "qi_changelog", title: "Write the 1.5 changelog", state: { state: "succeeded", run: "run_log", summary: "Changelog drafted and delivered." },
      repo: "repo_site", pipeline: "adaptive", priority: "low", minutes: 15, spent: 0.84, issue: ref("acme/site", 38, "issue"),
      pr: { ...ref("acme/site", 52, "pull_request"), state: "open", review_decision: "review_required", checks: { state: "success", total: 6, failed: 0 } },
      runs: [run("run_log", "qi_changelog", { state: "succeeded", summary: "Delivered" }, 0, 0.84)],
      stages: STAGES.map((stage, n) => stageRun("run_log", stage.id, { state: "completed" }, n * 3, stage.kind === "review" ? { verdict: "pass" } : {})),
    }),
    item({
      id: "qi_assets", title: "Compress the hero images", state: { state: "failed", error: "The build stage timed out after 120 minutes.", run: "run_assets" },
      repo: "repo_site", pipeline: "adaptive", priority: "normal", minutes: 10, spent: 2.4,
      runs: [run("run_assets", "qi_assets", { state: "failed", error: "timed out", stage_id: "build" }, -60, 2.4)],
      stages: [stageRun("run_assets", "plan", { state: "completed" }, -60), stageRun("run_assets", "build", { state: "timed_out" }, -50)],
    }),
    item({ id: "qi_old_flag", title: "Remove the legacy flag", state: { state: "cancelled", reason: "Superseded" }, repo: null, pipeline: "direct", priority: "low", minutes: 5, spent: 0 }),
  ];
  const plans = new Map<string, Json>([
    ["run_dark", { run: "run_dark", pipeline: "adaptive", revision: "r2", edited: true, stages: [...STAGES.slice(0, 3), { id: "docs", kind: "work", gate: "none", fan_out: 1, max_loops: 0, timeout_minutes: 30 }, STAGES[3]] }],
    ["run_log", { run: "run_log", pipeline: "adaptive", revision: "r1", edited: false, stages: STAGES }],
    ["run_pair", { run: "run_pair", pipeline: "adaptive", revision: "r1", edited: false, stages: STAGES }],
    ["run_assets", { run: "run_assets", pipeline: "adaptive", revision: "r1", edited: false, stages: STAGES }],
  ]);
  return {
    items: new Map(items.map((entry) => [String(entry.summary.id), entry])),
    questions: [
      { id: gateQuestion, number: 3, item: "qi_pairing", text: "Approve the proposal to shorten pairing codes to eight characters?", options: ["Approve", "Revise"], asked_at: at(41) },
      { id: "q_old", number: 2, item: "qi_changelog", text: "Include the deprecations?", options: [], asked_at: at(1), answer: "Yes, at the top.", answered_at: at(2) },
    ],
    reports: [{ report: "rp_1", item: "qi_changelog", title: "Changelog report", state: "published", path: "/reports/rp_1.md", location: "https://example.com/reports/rp_1", updated_at: at(15) }],
    repos: [{ id: "repo_site", name: "site" }, { id: "repo_api", name: "api" }],
    plans,
    pause: { global: false, repos: [], pipelines: [] },
    said: [],
  };
}
