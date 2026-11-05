// Import an export artifact (MEMORY-V2.md §2.3): a dry run reports what would be inserted, what is
// unchanged and what conflicts; only a confirm of that same file writes, and nothing is overwritten.

import type { aether, ConnId } from "@aether-ui/protocol";
import { useId, useState, type ChangeEvent } from "react";
import { invariant } from "../fleet/invariant.ts";
import { Action, RunNote, useRun } from "../controls/kit.tsx";
import { bytes, count } from "./format.ts";
import { replyData } from "./v2.ts";

// The remote listener's frame cap is 64 MiB, and JSON escaping grows the artifact inside the frame.
const MAX_FILE = 32 * 1024 * 1024;
const FIELDS = ["dry_run", "digest", "records", "inserted", "unchanged"];
type Picked = { readonly name: string; readonly artifact: string };

export function ImportTool({ conn, refresh }: { conn: ConnId; refresh: () => Promise<unknown> }) {
  const [picked, setPicked] = useState<Picked | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [plan, setPlan] = useState<aether.MemoryImport | null>(null);
  const [done, setDone] = useState<aether.MemoryImport | null>(null);
  const run = useRun(conn);
  const inputId = useId();
  const choose = async (event: ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0];
    setPlan(null); setDone(null); setPicked(null);
    if (!file) return;
    if (file.size > MAX_FILE) {
      setProblem(`${file.name} is ${bytes(file.size)}; export per scope to stay under ${bytes(MAX_FILE)}`);
      return;
    }
    setProblem(null);
    setPicked({ name: file.name, artifact: await file.text() });
  };
  const send = async (dryRun: boolean) => {
    invariant(picked !== null && picked.artifact !== "", "a file is chosen before it is sent");
    const reply = await run.send({ cmd: "memory_import", artifact: picked.artifact, dry_run: dryRun }, dryRun ? "Dry run done · nothing was written" : "Imported");
    if (!reply) return;
    const result = replyData<aether.MemoryImport>(reply, "memory_import", FIELDS);
    invariant(result.dry_run === dryRun, "the reply is for the run that was asked");
    if (dryRun) {
      setPlan(result);
      return;
    }
    setPlan(null); setDone(result);
    await refresh();
  };
  const command = { cmd: "memory_import", artifact: picked?.artifact ?? "", dry_run: true };
  const empty = picked !== null && picked.artifact.trim() === "" ? `${picked.name} is empty` : null;
  return <div className="mem-tool" aria-label="Import">
    <h4 className="mem-tool-title">Import</h4>
    <p className="b3-meta">Adds what an export holds and this store lacks. A differing memory is reported, never overwritten.</p>
    <label className="b3-field" htmlFor={inputId}><span className="ui-field-label">Export file (.jsonl)</span>
      <input id={inputId} className="mem-file" type="file" accept=".jsonl,.ndjson,application/jsonl,application/x-ndjson,text/plain" onChange={e => { void choose(e); }} /></label>
    {problem ? <p className="b3-note" role="alert" data-state="failed">{problem}</p> : null}
    <div className="b3-row">
      <Action conn={conn} command={command} label="Dry run import" busy={run.status.state === "busy"} blocked={picked === null ? "Choose an export file" : empty} onRun={() => send(true)} />
      {plan ? <Action conn={conn} command={{ ...command, dry_run: false }} label="Import" variant="primary" busy={run.status.state === "busy"} onRun={() => send(false)}
        blocked={plan.inserted === 0 ? "Nothing new to insert" : null}
        confirm={{ title: "Import memories?", body: `${count(plan.inserted, "memory", "memories")} will be inserted from ${picked?.name ?? "the file"} in one commit. ${count(plan.conflicts?.length ?? 0, "conflict stays", "conflicts stay")} as stored.`, confirmLabel: "Import memories" }} /> : null}
    </div>
    <RunNote status={run.status} />
    {plan ? <ImportCounts result={plan} /> : null}
    {done ? <ImportCounts result={done} /> : null}
  </div>;
}

function ImportCounts({ result }: { result: aether.MemoryImport }) {
  const conflicts = result.conflicts ?? [];
  invariant(result.inserted + result.unchanged + conflicts.length <= result.records, "every counted entry is one of the artifact's records");
  return <div className="mem-plan" data-kind="import" data-dry-run={result.dry_run ? "true" : "false"}>
    <p className="mem-plan-sum">{result.dry_run ? "Would import" : "Imported"} from {count(result.records, "record", "records")}</p>
    <dl className="b3-facts">
      <div><dt>{result.dry_run ? "Would insert" : "Inserted"}</dt><dd className="mem-big">{result.inserted}</dd></div>
      <div><dt>Unchanged</dt><dd className="mem-big">{result.unchanged}</dd></div>
      <div><dt>Conflicts</dt><dd className="mem-big" data-warn={conflicts.length > 0 ? "true" : undefined}>{conflicts.length}</dd></div>
    </dl>
    {conflicts.length > 0 ? <ul className="mem-conflicts" aria-label="Conflicts">
      {conflicts.map(c => <li key={c.memory}><code className="b3-code">{c.memory}</code> <span className="b3-meta">{c.reason}</span></li>)}
    </ul> : null}
    <p className="b3-meta">Digest <code className="b3-code mem-digest">{result.digest}</code></p>
  </div>;
}
