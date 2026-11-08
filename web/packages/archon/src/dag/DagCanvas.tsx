// The DAG on its canvas: layered, grouped by initiative, with the critical path in the accent. The
// layout follows the structure only; statuses, titles and the path repaint over the layout it has.

import { TaskDetails, taskNodeStyle, TASK_LEGEND, DagEmpty } from "./presentation.tsx";
import { GraphCanvas } from "@aether-ui/graph";
import { Button, ErrorState, Skeleton } from "@aether-ui/ui";
import { useCallback, useMemo, useState } from "react";
import { useDagLayout, type LayoutFn } from "./layout.ts";
import { dagDisplay, dagStructure, initiativeOfGroup, type CriticalPath, type DagData } from "./model.ts";

export interface DagCanvasProps {
  readonly data: DagData;
  readonly path: CriticalPath;
  readonly layout: LayoutFn;
  readonly label: string;
  readonly selected: string | undefined;
  readonly onSelect: (id: string) => void;
}

const NO_GROUPS: ReadonlySet<string> = new Set();

export function DagCanvas({ data, path, layout, label, selected, onSelect }: DagCanvasProps) {
  // Which initiative groups are folded is view state; folding changes the structure, so it relayouts.
  const [collapsed, setCollapsed] = useState<ReadonlySet<string>>(NO_GROUPS);
  const structure = useMemo(() => dagStructure(data, collapsed), [data, collapsed]);
  const placed = useDagLayout(structure, layout);
  const model = useMemo(() => dagDisplay(data, structure.model), [data, structure]);
  const onToggleGroup = useCallback((group: string) => {
    if (initiativeOfGroup(group) === null) return;
    setCollapsed((held) => {
      const next = new Set(held);
      if (!next.delete(group)) next.add(group);
      return next;
    });
  }, []);
  const onActivate = useCallback((id: string) => { if (data.byId.has(id)) onSelect(id); }, [data, onSelect]);
  if (placed.error !== null && placed.layout === undefined) {
    return <ErrorState title="The graph could not be laid out" message={placed.error} onRetry={placed.retry} className="dag-state" />;
  }
  return (
    <div className="dag-canvas" data-pending={placed.pending ? "true" : undefined} aria-busy={placed.layout === undefined}>
      <GraphCanvas model={model} layout={placed.layout} label={label} shape="box" {...(selected === undefined ? {} : { selected })}
        emphasis={path.emphasis} onActivate={onActivate} onToggleGroup={onToggleGroup}
        nodeStyle={taskNodeStyle} legend={TASK_LEGEND} emptyState={<DagEmpty />}
        details={(node) => { const task = data.byId.get(node.id); return task === undefined ? <strong>{node.label}</strong> : <TaskDetails node={task} />; }} />
      {placed.layout === undefined ? (
        <div className="dag-placing" aria-hidden="true">
          <Skeleton width="12rem" height="var(--space-3)" />
          <span>Laying out {data.nodes.length.toLocaleString("en")} items…</span>
        </div>
      ) : null}
      {placed.error !== null ? (
        <div className="dag-relayout-failed" role="alert">
          <span>The graph could not be laid out again, so it shows the last layout: {placed.error}</span>
          <Button size="sm" onClick={placed.retry}>Try again</Button>
        </div>
      ) : null}

    </div>
  );
}
