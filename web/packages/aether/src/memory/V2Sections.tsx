// The Memory 2.0 part of the screen, shown only when the daemon serves `memory_v2`: the retrieval
// explain, the host-only maintenance tools and the store's migration status.

import type { ConnId } from "@aether-ui/protocol";
import { invariant } from "../fleet/invariant.ts";
import { Section } from "../controls/kit.tsx";
import { ConsolidateTool } from "./Consolidate.tsx";
import { ExplainSection } from "./ExplainSection.tsx";
import { ExportTool } from "./Export.tsx";
import { ImportTool } from "./Import.tsx";
import { MigrationStatus } from "./MigrationStatus.tsx";
import { useHostGate, useMemoryV2 } from "./v2.ts";

export function V2Sections({ conn, known, refresh }: { conn: ConnId; known: ReadonlyMap<string, string>; refresh: () => Promise<unknown> }) {
  invariant(conn !== "", "the memory screen names its connection");
  if (!useMemoryV2(conn)) return null;
  return <>
    <ExplainSection conn={conn} known={known} />
    <Maintenance conn={conn} known={known} refresh={refresh} />
    <MigrationStatus conn={conn} />
  </>;
}

/** Consolidate, export and import are host-only alike, so one refusal stands for all three. */
function Maintenance({ conn, known, refresh }: { conn: ConnId; known: ReadonlyMap<string, string>; refresh: () => Promise<unknown> }) {
  const gate = useHostGate(conn, { cmd: "consolidate_memories", dry_run: true });
  invariant(gate.access === "host_only", "the maintenance tools are host-only");
  return <Section title="Maintenance" hint="Host only · Every write runs as a dry run first and waits for your confirm.">
    {gate.refusal ? <p role="note" className="b3-note">{gate.refusal}</p> : <div className="mem-tools">
      <ConsolidateTool conn={conn} known={known} refresh={refresh} />
      <ExportTool conn={conn} />
      <ImportTool conn={conn} refresh={refresh} />
    </div>}
  </Section>;
}
