// What one agent produced and received: its run's report, the runs on this daemon, its artifacts
// with a preview, and the deliveries in its inbox. Read-only; every reply is kept in core's store.

import { useReread } from "@aether-ui/core";
import type { ConnId } from "@aether-ui/protocol";
import { aether } from "@aether-ui/protocol";
import { Badge, Button } from "@aether-ui/ui";
import { useEffect, useMemo, useRef, useState } from "react";
import { invariant } from "../fleet/invariant.ts";
import { isList, ReadView, Section } from "./kit.tsx";

/** A preview reads at most this much of an artifact, as the old PWA did. */
const PREVIEW_BYTES = 262_144;

const isRecord = (value: unknown): value is Record<string, unknown> => value !== null && typeof value === "object" && !Array.isArray(value);

function artifactContent(data: Record<string, unknown>) {
  invariant(typeof data.base64 === "string", "an artifact read carries its bytes as base64");
  invariant(data.truncated === undefined || typeof data.truncated === "boolean", "artifact truncation is a boolean when reported");
  const binary = atob(data.base64);
  const bytes = Uint8Array.from(binary, (char) => char.charCodeAt(0));
  invariant(bytes.length <= PREVIEW_BYTES, "the daemon returns no more than the preview asked for");
  let text: string | null = null;
  try {
    text = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  } catch (error) {
    if (!(error instanceof TypeError)) throw error;
  }
  return { bytes, text, truncated: data.truncated === true, size: bytes.length };
}

/** Decodes a read_artifact reply into text, or says it is binary; the reply carries base64 bytes. */
export function previewText(data: Record<string, unknown>): { readonly text: string | null; readonly truncated: boolean; readonly size: number } {
  const { text, truncated, size } = artifactContent(data);
  return { text, truncated, size };
}

function ArtifactPreview({ data, artifact }: { readonly data: Record<string, unknown>; readonly artifact: aether.Artifact }) {
  invariant(artifact.id.trim() !== "", "an artifact preview identifies its artifact");
  invariant(artifact.name.trim() !== "", "an artifact download has a filename");
  const content = useMemo(() => artifactContent(data), [data]);
  const download = useRef<HTMLAnchorElement>(null);
  useEffect(() => {
    const link = download.current;
    invariant(link !== null, "a download URL belongs to a mounted link");
    const url = URL.createObjectURL(new Blob([content.bytes], { type: "application/octet-stream" }));
    link.href = url;
    return () => URL.revokeObjectURL(url);
  }, [content]);
  const filename = artifact.name.replace(/[\\/]/g, "_");
  return (
    <div className="b3-stack">
      <p className="b3-meta">{content.size} bytes.{content.truncated ? " Partial content: only the received bytes will be downloaded; the artifact was truncated." : ""}</p>
      {content.text === null ? <p className="b3-meta">A binary artifact; it has no text preview.</p> : <pre className="b3-pre">{content.text}</pre>}
      <a ref={download} className="b3-code" download={filename}>{content.truncated ? "Download (partial)" : "Download"}</a>
    </div>
  );
}

function Preview({ conn, artifact }: { readonly conn: ConnId; readonly artifact: aether.Artifact }) {
  invariant(conn.trim() !== "", "an artifact preview targets a daemon connection");
  invariant(artifact.id.trim() !== "", "an artifact read names its artifact");
  const [refresh, setRefresh] = useState(0);
  const id = artifact.id;
  const read = useReread(conn, `artifact_bytes:${id}`, aether.readArtifact({ id, max_bytes: PREVIEW_BYTES }), ["artifact_bytes"], refresh);
  return (
    <ReadView entry={read} what="the artifact" empty="The artifact is empty" guard={isRecord} isEmpty={() => false} retry={() => setRefresh((n) => n + 1)}>
      {(data) => <ArtifactPreview data={data} artifact={artifact} />}
    </ReadView>
  );
}

/** The artifact's stored metadata from get_artifact; it stays readable after the bytes are evicted. */
function Details({ conn, id }: { readonly conn: ConnId; readonly id: string }) {
  invariant(conn.trim() !== "", "artifact details target a daemon connection");
  invariant(id.trim() !== "", "artifact details name their artifact");
  const [refresh, setRefresh] = useState(0);
  const read = useReread(conn, `artifact:${id}`, aether.getArtifact({ id }), ["artifact"], refresh);
  return (
    <ReadView entry={read} what="the artifact details" empty="No details" guard={isRecord} isEmpty={() => false} retry={() => setRefresh((n) => n + 1)}>
      {(data) => {
        const artifact = data as unknown as aether.Artifact;
        invariant(artifact.id === id, "the details answer for the artifact asked about");
        invariant(isRecord(artifact.kind) && typeof artifact.kind.kind === "string", "an artifact's kind is a tagged record");
        return (
          <dl className="b3-facts" aria-label="Artifact details">
            <div><dt>Kind</dt><dd>{artifact.kind.kind.replaceAll("_", " ")}</dd></div>
            <div><dt>Format</dt><dd>{artifact.content_type ?? "Unknown"}</dd></div>
            <div><dt>Size</dt><dd>{artifact.size_bytes} bytes</dd></div>
            <div><dt>Created</dt><dd>{new Date(artifact.created_at).toLocaleString()}</dd></div>
            {artifact.path ? <div><dt>Path</dt><dd>{artifact.path}</dd></div> : null}
            {artifact.digest ? <div><dt>Digest</dt><dd className="b3-code">{artifact.digest}</dd></div> : null}
            {artifact.evicted_at ? <div><dt>Content removed</dt><dd>{new Date(artifact.evicted_at).toLocaleString()}</dd></div> : null}
          </dl>
        );
      }}
    </ReadView>
  );
}

type Open = { readonly id: string; readonly view: "preview" | "details" } | null;

function Artifacts({ conn, agent }: { readonly conn: ConnId; readonly agent: string }) {
  const [refresh, setRefresh] = useState(0);
  const [open, setOpen] = useState<Open>(null);
  const toggle = (id: string, view: "preview" | "details") => setOpen((held) => (held?.id === id && held.view === view ? null : { id, view }));
  const list = useReread(conn, `artifacts:${agent}`, aether.listArtifacts({ query: { agent, limit: 50 } }), ["artifacts"], refresh);
  return (
    <ReadView entry={list} what="the artifacts" empty="No artifacts yet" guard={isList} isEmpty={(items) => items.length === 0} retry={() => setRefresh((n) => n + 1)}>
      {(items) => (
        <ul className="b3-list" aria-label="Artifacts">
          {(items as readonly aether.Artifact[]).map((artifact) => (
            <li key={artifact.id} className="b3-item">
              <div className="b3-item-head">
                <p className="b3-item-title">{artifact.name}</p>
                <span className="b3-row">
                  <Button variant="quiet" size="sm" aria-expanded={open?.id === artifact.id && open.view === "details"}
                    onClick={() => toggle(artifact.id, "details")}>{open?.id === artifact.id && open.view === "details" ? "Hide details" : "Details"}</Button>
                  <Button variant="quiet" size="sm" aria-expanded={open?.id === artifact.id && open.view === "preview"} disabled={artifact.evicted_at != null}
                    onClick={() => toggle(artifact.id, "preview")}>{open?.id === artifact.id && open.view === "preview" ? "Hide" : "Preview"}</Button>
                </span>
              </div>
              <p className="b3-meta">{artifact.summary || artifact.content_type || "No summary"} · {artifact.size_bytes} bytes{artifact.evicted_at ? " · evicted" : ""}</p>
              {open?.id === artifact.id && open.view === "details" ? <Details conn={conn} id={artifact.id} /> : null}
              {open?.id === artifact.id && open.view === "preview" ? <Preview conn={conn} artifact={artifact} /> : null}
            </li>
          ))}
        </ul>
      )}
    </ReadView>
  );
}

function Report({ conn, run }: { readonly conn: ConnId; readonly run: string }) {
  const [refresh, setRefresh] = useState(0);
  const report = useReread(conn, `run_report:${run}`, aether.getRunReport({ run }), ["run_report"], refresh);
  return (
    <ReadView entry={report} what="the run report" empty="No report" guard={isRecord} isEmpty={() => false} retry={() => setRefresh((n) => n + 1)}>
      {(data) => {
        const body = (data as unknown as aether.RunReport).body;
        invariant(isRecord(body), "a run report has a body");
        return (
          <dl className="b3-facts">
            <div><dt>Outcome</dt><dd>{body.outcome}</dd></div>
            <div><dt>Objective</dt><dd>{body.objective ?? "None given"}</dd></div>
            <div><dt>Events</dt><dd>{body.events}</dd></div>
            <div><dt>Duration</dt><dd>{Math.round(body.duration_ms / 1000)} s</dd></div>
            <div><dt>Cost</dt><dd>${body.usage.cost_usd.toFixed(2)}</dd></div>
            <div><dt>Failures</dt><dd>{body.failures.length}</dd></div>
          </dl>
        );
      }}
    </ReadView>
  );
}

function Runs({ conn, current }: { readonly conn: ConnId; readonly current: string }) {
  const [refresh, setRefresh] = useState(0);
  const [open, setOpen] = useState<string | null>(null);
  const runs = useReread(conn, "runs", aether.listRuns({ limit: 20 }), ["runs"], refresh);
  return (
    <ReadView entry={runs} what="the runs" empty="No runs recorded" guard={isList} isEmpty={(items) => items.length === 0} retry={() => setRefresh((n) => n + 1)}>
      {(items) => (
        <ul className="b3-list" aria-label="Runs">
          {(items as readonly aether.RunListing[]).map((run) => (
            <li key={run.run} className="b3-item">
              <div className="b3-item-head">
                <p className="b3-meta">
                  {run.objective ?? run.run} · {run.agents_live}/{run.agents} live · ${run.usage.cost_usd.toFixed(2)}
                  {run.run === current ? <> <Badge tone="accent">This agent's run</Badge></> : null}
                </p>
                <Button variant="quiet" size="sm" aria-expanded={open === run.run}
                  onClick={() => setOpen((held) => (held === run.run ? null : run.run))}>{open === run.run ? "Hide report" : "Report"}</Button>
              </div>
              {open === run.run ? <Report conn={conn} run={run.run} /> : null}
            </li>
          ))}
        </ul>
      )}
    </ReadView>
  );
}

function Deliveries({ conn, agent }: { readonly conn: ConnId; readonly agent: string }) {
  const [refresh, setRefresh] = useState(0);
  const inbox = useReread(conn, `inbox:${agent}`, aether.getInbox({ agent, after: 0 }), ["inbox"], refresh);
  return (
    <ReadView entry={inbox} what="the deliveries" empty="Nothing delivered yet" guard={isList} isEmpty={(items) => items.length === 0} retry={() => setRefresh((n) => n + 1)}>
      {(items) => (
        <ul className="b3-list" aria-label="Deliveries">
          {(items as readonly aether.InboxItem[]).map((item) => (
            <li key={item.id} className="b3-meta">#{item.seq} · {(item.kind as { kind: string }).kind.replaceAll("_", " ")} · {new Date(item.delivered_at).toLocaleString()}</li>
          ))}
        </ul>
      )}
    </ReadView>
  );
}

export function RecordsPanel({ conn, agent }: { readonly conn: ConnId; readonly agent: aether.Agent }) {
  invariant(agent.id !== "" && agent.run !== "", "records name their agent and its run");
  return (
    <>
      <Section title="This run" hint="The report of the run this agent belongs to."><Report conn={conn} run={agent.run} /></Section>
      <Section title="Artifacts" hint="Files and notes the agent published."><Artifacts conn={conn} agent={agent.id} /></Section>
      <Section title="Deliveries" hint="Messages and results that reached the agent's inbox."><Deliveries conn={conn} agent={agent.id} /></Section>
      <Section title="Recent runs" hint="The newest runs on this daemon."><Runs conn={conn} current={agent.run} /></Section>
    </>
  );
}
