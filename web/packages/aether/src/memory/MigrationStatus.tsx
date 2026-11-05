// The store's Memory 2.0 migrations, counts, pre-migration backups and last import
// (MEMORY-V2.md §2.5). An observe read: every device may see it.

import { useReread } from "@aether-ui/core";
import type { aether, ConnId } from "@aether-ui/protocol";
import { Badge } from "@aether-ui/ui";
import { useState } from "react";
import { invariant } from "../fleet/invariant.ts";
import { ReadView, Section } from "../controls/kit.tsx";
import { bytes, when } from "./format.ts";

const COMMAND = { cmd: "memory_migration_status" };
const COUNTS: ReadonlyArray<[keyof aether.MemoryCounts, string]> = [
  ["total", "Memories"], ["admitted", "Admitted"], ["implicit", "Implicit (no admission row)"],
  ["candidates", "Candidates"], ["stale", "Stale"], ["held_out", "Holdout log rows"],
];

export function MigrationStatus({ conn }: { conn: ConnId }) {
  const [again, setAgain] = useState(0);
  const entry = useReread(conn, "memory:migration", COMMAND, ["memory_migration_status"], again);
  return <Section title="Store" hint="Schema, Memory 2.0 migrations and pre-migration backups.">
    <ReadView entry={entry} what="the migration status" empty="No status" guard={isStatus} isEmpty={() => false} retry={() => setAgain(n => n + 1)}>
      {status => <StatusBody status={status} />}
    </ReadView>
  </Section>;
}

function StatusBody({ status }: { status: aether.MemoryMigrationStatus }) {
  invariant(Number.isInteger(status.schema_version) && status.schema_version > 0, "the store has a schema stamp");
  const backups = status.backups ?? [];
  return <div className="b3-stack">
    <div className="b3-row mem-tight">
      <span className="mem-schema">Schema {status.schema_version}</span>
      {status.migrations.map(m => <Badge key={m.version} tone={m.applied ? "success" : "warning"}>V{m.version} {m.applied ? "applied" : "pending"}</Badge>)}
    </div>
    <dl className="b3-facts">{COUNTS.map(([key, label]) => <div key={key}><dt>{label}</dt><dd className="mem-big">{status.memories[key]}</dd></div>)}</dl>
    {status.last_import ? <LastImport last={status.last_import} /> : <p className="b3-meta">Nothing imported yet.</p>}
    <div className="mem-turn-block">
      <h5 className="mem-label">Backups</h5>
      {backups.length === 0 ? <p className="b3-meta">No pre-migration backups.</p> : <ul className="mem-backups">
        {backups.map(b => <li key={b.name}><code className="b3-code">{b.name}</code>
          <span className="b3-meta">V{b.from_version} → V{b.to_version} · {bytes(b.bytes)} · {when(b.at)}</span></li>)}
      </ul>}
    </div>
  </div>;
}

function LastImport({ last }: { last: aether.LastImport }) {
  invariant(last.digest !== "", "an import is named by its digest");
  return <p className="b3-meta">Last import {when(last.at)} · {last.inserted} inserted · {last.unchanged} unchanged · {last.conflicts} conflicts ·
    <code className="b3-code mem-digest"> {last.digest.slice(0, 12)}</code></p>;
}

function isStatus(value: unknown): value is aether.MemoryMigrationStatus {
  if (value === null || typeof value !== "object") return false;
  const status = value as Partial<aether.MemoryMigrationStatus>;
  return typeof status.schema_version === "number" && Array.isArray(status.migrations) && typeof status.memories === "object";
}
