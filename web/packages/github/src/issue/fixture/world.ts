// The GitHub world the D4 tests and screenshot page read: issues and pull requests in Archon's cache, the
// items linked to them with runs and stages, and the Aether agents those stages ran on.

type Json = Record<string, unknown>;

export interface FauxItem { summary: Json; detail: Json }

export interface GithubWorld {
  readonly items: Map<string, FauxItem>;
  readonly objects: Json[];
  readonly links: Json[];
  readonly agents: Json[];
}

const AT = "2026-11-04T09:00:00Z";
export const FETCHED = "2026-11-04T09:55:00Z";

export function ref(repo: string, number: number, kind: "issue" | "pull_request"): Json {
  return { host: "github.com", repo, number, kind };
}

export function object(at: Json, title: string, state: string, extra: Json = {}): Json {
  return { ref: at, title, state, labels: [], updated_at: AT, fetched_at: FETCHED, ...extra };
}

export function link(item: string, at: Json, relation: string, source = "watcher"): Json {
  return { item, ref: at, relation, source, created_at: AT };
}

function stage(id: string, run: string, stageId: string, agent: string | null, state: Json): Json {
  return { id, run, stage_id: stageId, attempt: 1, fan_index: 0, choice: { model: "claude-code/opus", effort: "high" }, cost_usd: 0.4,
    started_at: AT, state, ...(agent ? { aether_agent: agent } : {}) };
}

export function item(id: string, title: string, state: Json, extra: { pr?: Json; issue?: Json; runs?: Json[]; stages?: Json[] } = {}): FauxItem {
  const summary: Json = {
    id, title, brief: `Brief of ${title}`, state, repo: "repo_site", pipeline: "adaptive", priority: "normal", created_at: AT, updated_at: AT,
    dependencies: [], tags: [], model_plan: {}, spent_usd: 1.2, ...(extra.pr ? { pr: extra.pr } : {}), ...(extra.issue ? { issue: extra.issue } : {}),
  };
  const { pr: _pr, issue: _issue, ...queued } = summary;
  return { summary, detail: { item: queued, runs: extra.runs ?? [], stages: extra.stages ?? [], gates: [], spent_usd: 1.2 } };
}

function run(id: string, itemId: string, state: Json): Json {
  return { id, item: itemId, pipeline: "adaptive", branch: `archon/${itemId}`, workspace: `/w/${itemId}`, cost_usd: 1.2, started_at: AT, state };
}

export function agent(id: string, name: string, state: string): Json {
  return { id, name, status: { state }, role: "worker", run: "run_a", scope: {}, model: { provider: "claude-code", model: "opus" },
    config: {}, created_at: AT, updated_at: AT };
}

export const ISSUE_DARK = ref("acme/site", 41, "issue");
export const PR_DARK = ref("acme/site", 57, "pull_request");
export const ISSUE_CHANGELOG = ref("acme/site", 38, "issue");
export const PR_CHANGELOG = ref("acme/site", 52, "pull_request");
export const ISSUE_NEW = ref("acme/site", 77, "issue");

export function demoWorld(): GithubWorld {
  const dark = item("qi_dark_mode", "Dark mode for the dashboard", { state: "running", run: "run_dark" }, {
    issue: ISSUE_DARK, pr: { ...PR_DARK, state: "draft" },
    runs: [run("run_dark", "qi_dark_mode", { state: "running", stage_id: "review", attempt: 1 })],
    stages: [stage("st_1", "run_dark", "build", "agt_builder", { state: "completed" }), stage("st_2", "run_dark", "review", "agt_reviewer", { state: "running" })],
  });
  const changelog = item("qi_changelog", "Write the changelog", { state: "succeeded", run: "run_log", summary: "Changelog drafted and delivered." }, {
    issue: ISSUE_CHANGELOG, pr: { ...PR_CHANGELOG, state: "merged" },
    runs: [run("run_log", "qi_changelog", { state: "succeeded", summary: "Delivered" })],
    stages: [stage("st_3", "run_log", "build", "agt_writer", { state: "completed" })],
  });
  return {
    items: new Map([["qi_dark_mode", dark], ["qi_changelog", changelog]]),
    objects: [
      object(ISSUE_DARK, "Dark mode for the dashboard", "open", { labels: ["ui", "enhancement"], author: "mira" }),
      object(PR_DARK, "Add a dark theme", "open", { draft: true, review_decision: "review_required", checks: { state: "pending", total: 4, failed: 0 }, author: "archon-bot" }),
      object(ISSUE_CHANGELOG, "Changelog for 1.5", "closed", { state_reason: "completed", author: "mira" }),
      object(PR_CHANGELOG, "Changelog for 1.5", "merged", { review_decision: "approved", checks: { state: "success", total: 6, failed: 0 } }),
      object(ISSUE_NEW, "Export the dashboard as CSV", "open", { labels: ["feature"], author: "jun" }),
    ],
    links: [
      link("qi_dark_mode", ISSUE_DARK, "origin"), link("qi_dark_mode", PR_DARK, "delivered", "ship"),
      link("qi_changelog", ISSUE_CHANGELOG, "origin"), link("qi_changelog", PR_CHANGELOG, "delivered", "ship"),
    ],
    agents: [agent("agt_builder", "site-builder", "completed"), agent("agt_reviewer", "site-reviewer", "running")],
  };
}
