import { connectionDraftKey, useApp, useDraft } from "@aether-ui/core";
import { useScopeSuggestions } from "./Repository.tsx";
// Export every memory of a scope, or of the store, as the JSON Lines artifact (MEMORY-V2.md §2.4),
// offered as a download with its record count and SHA-256 digest.

import type { aether, ConnId, DesktopMemoryExport } from "@aether-ui/protocol";
import { Combo } from "@aether-ui/ui";
import { useEffect, useMemo, useState } from "react";
import { invariant } from "../fleet/invariant.ts";
import { Action, RunNote, useRun } from "../controls/kit.tsx";
import { count } from "./format.ts";
import { parseScope, SCOPE_HINT } from "./scope.ts";
import { replyData } from "./v2.ts";

export function ExportTool({ conn }: { conn: ConnId }) {
  const draft = useDraft(connectionDraftKey("memory", conn, "export"), { text: "" });
  const { text } = draft.value;
  const suggestions = useScopeSuggestions(conn);
  const [result, setResult] = useState<aether.MemoryExport | null>(null);
  const run = useRun(conn);
  const parsed = parseScope(text);
  const scope = "scope" in parsed ? parsed.scope : null;
  const command = { cmd: "memory_export", ...(scope ? { scope } : {}) };
  const exportIt = async () => {
    const reply = await run.send(command, "Export ready");
    if (!reply) return;
    const data = replyData<aether.MemoryExport>(reply, "memory_export", ["artifact", "records", "digest"]);
    invariant(/^[0-9a-f]{64}$/.test(data.digest), "the digest is SHA-256 hex");
    draft.clearIfUnchanged();
    setResult(data);
  };
  return <div className="mem-tool" aria-label="Export">
    <h4 className="mem-tool-title">Export</h4>
    <p className="b3-meta">Every memory in the scope, candidates and forgotten ones included, as one JSON Lines file.</p>
    <Combo label="Export scope" suggestions={suggestions} hint={SCOPE_HINT} value={text} {...("error" in parsed ? { error: parsed.error } : {})}
      onChange={text => { draft.update({ text }); setResult(null); }} spellCheck={false} autoComplete="off" />
    <div className="b3-row">
      <Action conn={conn} command={command} label="Export memories" busy={run.status.state === "busy"} blocked={"error" in parsed ? parsed.error : null} onRun={exportIt} />
    </div>
    <RunNote status={run.status} />
    {result ? <Download result={result} /> : null}
  </div>;
}

function Download({ result }: { result: aether.MemoryExport }) {
  invariant(Number.isInteger(result.records) && result.records >= 0, "records is a count");
  const app = useApp();
  const desktop = app.host.desktop as (typeof app.host.desktop & Partial<DesktopMemoryExport>);
  const save = desktop?.saveMemoryExport;
  const [error, setError] = useState("");
  const url = useMemo(() => save ? null : URL.createObjectURL(new Blob([result.artifact], { type: "application/jsonl" })), [result.artifact, save]);
  useEffect(() => () => { if (url !== null) URL.revokeObjectURL(url); }, [url]);
  const download = async () => {
    if (!save) return;
    setError("");
    try { await save(result.digest); }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); }
  };
  const name = `aether-memories-${result.digest.slice(0, 12)}.jsonl`;
  return <div className="mem-plan" data-kind="export">
    <p className="mem-plan-sum">{count(result.records, "record", "records")} exported</p>
    <p className="b3-meta">SHA-256 <code className="b3-code mem-digest">{result.digest}</code></p>
    {url === null ? <button className="mem-download" type="button" onClick={() => { void download(); }}>Download {name}</button>
      : <a className="mem-download" href={url} download={name}>Download {name}</a>}
    {error ? <p role="alert">{error}</p> : null}
  </div>;
}
