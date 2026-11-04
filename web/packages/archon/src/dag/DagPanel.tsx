// The DAG's side panel: the selected item and why it waits, or the overview of the critical path,
// held-up items and initiatives. Everything here derives from the graph core stores.

import { routeHref } from "@aether-ui/core";
import { Badge, Button, StatusPill } from "@aether-ui/ui";
import { invariant } from "../queue/model.ts";
import { nodeStatus, reasonsFor, statusLabel, type CriticalPath, type DagData, type HeldEntry, type InitiativeProgress, type TaskNode } from "./model.ts";

/** Lists past this many rows say how many more there are; the canvas and its node list hold them all. */
export const PANEL_ROWS = 30;

interface Selectable { readonly onSelect: (id: string) => void }

function NodeButton({ node, onSelect }: { readonly node: TaskNode } & Selectable) {
  return (
    <button type="button" className="dag-row" onClick={() => onSelect(node.id)}>
      <span className="dag-row-title">{node.title}</span>
      <StatusPill status={nodeStatus(node)} label={statusLabel(node)} />
    </button>
  );
}

function More({ total }: { readonly total: number }) {
  return total > PANEL_ROWS ? <p className="dag-muted">and {(total - PANEL_ROWS).toLocaleString("en")} more</p> : null;
}

function Links({ title, ids, data, onSelect }: { readonly title: string; readonly ids: readonly string[]; readonly data: DagData } & Selectable) {
  if (ids.length === 0) return null;
  return (
    <section className="dag-section" aria-label={title}>
      <h3 className="dag-heading">{title}</h3>
      {ids.slice(0, PANEL_ROWS).map((id) => {
        const node = data.byId.get(id);
        invariant(node !== undefined, `linked node ${id} is in the graph`);
        return <NodeButton key={id} node={node} onSelect={onSelect} />;
      })}
      <More total={ids.length} />
    </section>
  );
}

export function NodeInspector({ data, id, onPath, onSelect, onClose }: { readonly data: DagData; readonly id: string; readonly onPath: boolean; readonly onClose: () => void } & Selectable) {
  const node = data.byId.get(id);
  invariant(node !== undefined, `the inspector shows a node of the graph, not ${id}`);
  const reasons = reasonsFor(data, id);
  const before = (data.incoming.get(id) ?? []).map((edge) => edge.from);
  const after = (data.outgoing.get(id) ?? []).map((edge) => edge.to);
  const meta = [node.pipeline, node.stage ? `stage ${node.stage}` : null, `${node.priority} priority`].filter(Boolean).join(" · ");
  return (
    <div className="dag-inspector" data-item={id}>
      <Button variant="quiet" size="sm" onClick={onClose}>← Overview</Button>
      <h2 className="dag-title">{node.title}</h2>
      <div className="dag-badges">
        <StatusPill status={nodeStatus(node)} label={statusLabel(node)} />
        {onPath ? <Badge tone="accent">Critical path</Badge> : null}
      </div>
      <p className="dag-muted">{meta}</p>
      {reasons.length > 0 ? (
        <section className="dag-callout" aria-label="Why it is held">
          <h3 className="dag-heading">Why it is held</h3>
          <ul className="dag-reasons">{reasons.map((reason) => <li key={reason}>{reason}</li>)}</ul>
        </section>
      ) : null}
      <Links title="After" ids={before} data={data} onSelect={onSelect} />
      <Links title="Before" ids={after} data={data} onSelect={onSelect} />
      <div className="dag-links">
        <a className="ui-link" href={routeHref("#/archon/items/:item", { item: id })}>Open the item</a>
        {node.initiative ? <a className="ui-link" href={routeHref("#/archon/initiatives/:id", { id: node.initiative })}>Open its initiative</a> : null}
      </div>
    </div>
  );
}

export interface OverviewProps extends Selectable {
  readonly data: DagData;
  readonly path: CriticalPath;
  readonly held: readonly HeldEntry[];
  readonly progress: readonly InitiativeProgress[];
}

export function DagOverview({ data, path, held, progress, onSelect }: OverviewProps) {
  return (
    <div className="dag-overview">
      <section className="dag-section" aria-label="Critical path">
        <h3 className="dag-heading">Critical path</h3>
        {path.nodes.length === 0 ? <p className="dag-muted">No open item waits on another.</p> : (
          <>
            <p className="dag-muted">{path.nodes.length} items, one after another</p>
            <ol className="dag-path">
              {path.nodes.slice(0, PANEL_ROWS).map((id) => <li key={id}><NodeButton node={data.byId.get(id) as TaskNode} onSelect={onSelect} /></li>)}
            </ol>
            <More total={path.nodes.length} />
          </>
        )}
        {path.cyclic > 0 ? <p className="dag-warning">{path.cyclic} items wait on each other in a cycle.</p> : null}
      </section>
      <HeldList held={held} onSelect={onSelect} />
      {progress.length > 0 ? <InitiativeList progress={progress} /> : null}
    </div>
  );
}

function HeldList({ held, onSelect }: { readonly held: readonly HeldEntry[] } & Selectable) {
  return (
    <section className="dag-section" aria-label="Held up">
      <h3 className="dag-heading">Held up <span className="dag-count">{held.length}</span></h3>
      {held.length === 0 ? <p className="dag-muted">Nothing is held up.</p> : null}
      {held.slice(0, PANEL_ROWS).map((entry) => (
        <button key={entry.id} type="button" className="dag-row dag-held" data-item={entry.id} onClick={() => onSelect(entry.id)}>
          <span className="dag-row-title">{entry.title}</span>
          <StatusPill status={entry.status} label={entry.status === "operator" ? "Needs you" : entry.status === "blocked" ? "Blocked" : "Waiting"} />
          <span className="dag-reason">{entry.reasons.join(" · ")}</span>
        </button>
      ))}
      <More total={held.length} />
    </section>
  );
}

function InitiativeList({ progress }: { readonly progress: readonly InitiativeProgress[] }) {
  return (
    <section className="dag-section" aria-label="Initiatives">
      <h3 className="dag-heading">Initiatives</h3>
      {progress.slice(0, PANEL_ROWS).map((row) => (
        <a key={row.id} className="dag-row dag-initiative" href={routeHref("#/archon/initiatives/:id", { id: row.id })}>
          <span className="dag-row-title">{row.title}</span>
          <span className="dag-muted">{row.done} of {row.total} done{row.held > 0 ? ` · ${row.held} held` : ""}</span>
          <meter className="dag-meter" min={0} max={Math.max(1, row.total)} value={row.done} aria-label={`${row.title}: ${row.done} of ${row.total} done`} />
        </a>
      ))}
      <More total={progress.length} />
    </section>
  );
}
