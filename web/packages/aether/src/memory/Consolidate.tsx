import { connectionDraftKey, useDraft } from "@aether-ui/core";
import { useScopeSuggestions } from "./Repository.tsx";
// Merge near-duplicate memories (MEMORY-V2.md §2.2): a dry run plans the clusters, and only an
// explicit confirm of that plan's scope writes anything.

import type { aether, ConnId } from "@aether-ui/protocol";
import { Badge, Combo } from "@aether-ui/ui";
import { useState } from "react";
import { invariant } from "../fleet/invariant.ts";
import { Action, RunNote, useRun } from "../controls/kit.tsx";
import { count } from "./format.ts";
import { parseScope, SCOPE_HINT, scopeText } from "./scope.ts";
import { replyData } from "./v2.ts";

type Plan = { readonly scope: aether.MemoryScope | null; readonly result: aether.MemoryConsolidation };
const FIELDS = ["dry_run", "scanned", "clusters"];

export function ConsolidateTool({ conn, known, refresh }: { conn: ConnId; known: ReadonlyMap<string, string>; refresh: () => Promise<unknown> }) {
  const draft = useDraft(connectionDraftKey("memory", conn, "consolidate"), { text: "" });
  const { text } = draft.value;
  const suggestions = useScopeSuggestions(conn);
  const [plan, setPlan] = useState<Plan | null>(null);
  const [done, setDone] = useState<aether.MemoryConsolidation | null>(null);
  const run = useRun(conn);
  const parsed = parseScope(text);
  const scope = "scope" in parsed ? parsed.scope : null;
  const command = (dryRun: boolean, at: aether.MemoryScope | null) => ({ cmd: "consolidate_memories", dry_run: dryRun, ...(at ? { scope: at } : {}) });
  const planIt = async () => {
    const reply = await run.send(command(true, scope), "Plan ready · nothing was written");
    if (!reply) return;
    const result = replyData<aether.MemoryConsolidation>(reply, "memory_consolidation", FIELDS);
    invariant(result.dry_run && result.clusters.every(c => c.outcome === "planned"), "a dry run only plans");
    setDone(null);
    setPlan({ scope, result });
  };
  const apply = async () => {
    invariant(plan !== null && plan.result.clusters.length > 0, "only a non-empty plan is confirmed");
    const reply = await run.send(command(false, plan.scope), "Consolidated");
    if (!reply) return;
    const result = replyData<aether.MemoryConsolidation>(reply, "memory_consolidation", FIELDS);
    invariant(!result.dry_run, "a confirmed run writes");
    setPlan(null);
    draft.clearIfUnchanged();
    setDone(result);
    await refresh();
  };
  const clusters = plan?.result.clusters.length ?? 0;
  return <div className="mem-tool" aria-label="Consolidate">
    <h4 className="mem-tool-title">Consolidate</h4>
    <p className="b3-meta">Merges near-duplicates into one survivor. Plan first; nothing is written until you confirm.</p>
    <Combo label="Consolidation scope" suggestions={suggestions} hint={SCOPE_HINT} value={text} {...("error" in parsed ? { error: parsed.error } : {})}
      onChange={text => { draft.update({ text }); setPlan(null); }} spellCheck={false} autoComplete="off" />
    <div className="b3-row">
      <Action conn={conn} command={command(true, scope)} label="Plan consolidation" busy={run.status.state === "busy"} blocked={"error" in parsed ? parsed.error : null} onRun={planIt} />
      {plan && clusters > 0 ? <Action conn={conn} command={command(false, plan.scope)} label="Consolidate" variant="primary" busy={run.status.state === "busy"} onRun={apply}
        confirm={{ title: "Consolidate memories?", body: `${count(clusters, "cluster", "clusters")} will merge into their survivors; the other members become superseded.`, confirmLabel: "Merge memories" }} /> : null}
    </div>
    <RunNote status={run.status} />
    {plan ? <Clusters result={plan.result} known={known} /> : null}
    {done ? <Clusters result={done} known={known} /> : null}
  </div>;
}

function Clusters({ result, known }: { result: aether.MemoryConsolidation; known: ReadonlyMap<string, string> }) {
  invariant(Number.isInteger(result.scanned) && result.scanned >= 0, "scanned is a count");
  return <div className="mem-plan" data-kind="consolidate" data-dry-run={result.dry_run ? "true" : "false"}>
    <p className="mem-plan-sum">{result.dry_run ? "Planned" : "Ran"} over {count(result.scanned, "memory", "memories")} · {count(result.clusters.length, "cluster", "clusters")}</p>
    {result.clusters.length === 0 ? <p className="b3-meta">Nothing to merge.</p> : <ul className="mem-clusters">
      {result.clusters.map(cluster => <li key={cluster.survivor} className="mem-cluster">
        <div className="b3-row mem-tight">
          <Badge tone={cluster.outcome === "merged" ? "success" : cluster.outcome === "skipped" ? "warning" : "info"}>{cluster.outcome}</Badge>
          {cluster.promoted ? <Badge tone="accent">Promoted</Badge> : null}
          <span className="b3-meta">{scopeText(cluster.scope)} · {count(cluster.members.length, "member", "members")}</span>
        </div>
        <p className="mem-cluster-survivor">{known.get(cluster.survivor) ?? <code className="b3-code">{cluster.survivor}</code>}</p>
        {cluster.reason ? <p className="b3-meta">{cluster.reason}</p> : null}
      </li>)}
    </ul>}
  </div>;
}
