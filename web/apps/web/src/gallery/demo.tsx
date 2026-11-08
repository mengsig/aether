import { Badge, Button, EmptyState, EntityActionMenu, Field, Menu, PageHeader, Picker, Segmented, StatusPill, STATUSES, Switch, Tabs, Tooltip, VirtualList } from "@aether-ui/ui";
import { useState } from "react";
import { sampleAgent } from "./fixtures.tsx";

const AGENTS = 10_000;

function AgentRow({ index }: { readonly index: number }) {
  const agent = sampleAgent(index);
  return (
    <div className="gallery-row">
      <code className="gallery-agent">{agent.name}</code>
      <StatusPill status={agent.status} />
      <Badge>{agent.turns} turns</Badge>
    </div>
  );
}

const MODELS = [
  { id: "opus", label: "Opus 5.5", group: "Claude", badge: "Default", badgeTone: "accent" as const },
  { id: "sonnet", label: "Sonnet 5.5", group: "Claude" },
  { id: "astra", label: "GPT-6 Astra", group: "Codex", description: "API-billed" },
];

function FormsPanel() {
  const [model, setModel] = useState<string | null>("opus");
  const [notify, setNotify] = useState(true);
  const [mode, setMode] = useState("agent");
  return (
    <section className="gallery-panel" aria-labelledby="g-forms">
      <h2 id="g-forms">Choices</h2>
      <Picker label="Model" value={model} options={MODELS} onChange={setModel} hint="Used for new agents." />
      <Segmented label="Work type" value={mode} onChange={setMode} options={[{ id: "agent", label: "New agent" }, { id: "task", label: "Task" }, { id: "review", label: "Review" }]} />
      <Switch label="Notify me" hint="When an agent asks a question." checked={notify} onChange={setNotify} />
    </section>
  );
}

function TablePanel() {
  const rows = [0, 1, 2, 3].map(sampleAgent);
  return (
    <section className="gallery-panel" aria-labelledby="g-table">
      <h2 id="g-table">Table</h2>
      <div className="ui-table-wrap">
        <table className="ui-table">
          <thead><tr><th scope="col">Agent</th><th scope="col">State</th><th scope="col" data-numeric>Turns</th></tr></thead>
          <tbody>{rows.map((row) => <tr key={row.name}><td><code>{row.name}</code></td><td><StatusPill status={row.status} /></td><td data-numeric>{row.turns}</td></tr>)}</tbody>
        </table>
      </div>
    </section>
  );
}

export function DemoScreen() {
  return (
    <div className="ui-page">
      <PageHeader title="planner-00042" lede="A page header names what the screen shows; the shell names the route." actions={<><Button size="sm">Pause</Button><Button size="sm" variant="primary">Message</Button></>} />
      <DemoPanels />
    </div>
  );
}

function DemoPanels() {
  return (
    <div className="gallery">
      <section className="gallery-panel gallery-wide" aria-labelledby="g-agents">
        <header className="gallery-panel-head">
          <h2 id="g-agents">Agents</h2>
          <Badge tone="accent">{AGENTS.toLocaleString("en")} live</Badge>
          <EntityActionMenu entity={{ kind: "agent", conn: "local", id: "agt_demo" }} />
        </header>
        <VirtualList label="Agents" count={AGENTS} rowHeight={44} height={308} renderRow={(index) => <AgentRow index={index} />} />
      </section>
      <section className="gallery-panel" aria-labelledby="g-states">
        <h2 id="g-states">States</h2>
        <div className="gallery-pills">{STATUSES.map((status) => <StatusPill key={status} status={status} />)}</div>
      </section>
      <section className="gallery-panel" aria-labelledby="g-controls">
        <h2 id="g-controls">Controls</h2>
        <Tabs label="Run views" tabs={[
          { id: "plan", label: "Plan", panel: <Field label="Objective" placeholder="What should this run achieve?" hint="Agents see this first." /> },
          { id: "limits", label: "Limits", panel: <Field label="Budget (USD)" defaultValue="abc" error="Enter a number" /> },
        ]} />
        <div className="gallery-buttons">
          <Button variant="primary">Start run</Button>
          <Button>Pause</Button>
          <Tooltip content="Stops every agent in this run"><Button variant="danger">Cancel run</Button></Tooltip>
          <Menu label="More" items={[{ id: "copy", label: "Copy run id", onSelect: () => undefined }, { id: "drop", label: "Delete", tone: "danger", onSelect: () => undefined }]} />
        </div>
      </section>
      <FormsPanel />
      <TablePanel />
      <section className="gallery-panel gallery-wide" aria-labelledby="g-empty">
        <h2 id="g-empty" className="gallery-hidden">Empty inbox</h2>
        <EmptyState title="Inbox zero" action={<Button>Refresh</Button>}>Nothing waits on you. New questions from agents land here.</EmptyState>
      </section>
    </div>
  );
}

export function FailingScreen(): never {
  throw new Error("archond at studio.local/archon did not answer within 10 s");
}
