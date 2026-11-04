import { useState } from "react";
import { routeHref, selectConnections, selectEvents, useApp, useReread, useSelector } from "@aether-ui/core";
import type { aether } from "@aether-ui/protocol";
import { Button, EmptyState, Field, VirtualList } from "@aether-ui/ui";
import { Section } from "../controls/kit.tsx";
import { invariant } from "../fleet/invariant.ts";
import { records, ReadNote } from "../memory/shared.tsx";
import { orderedEvents } from "./data.ts";
import { StartAgent } from "./StartAgent.tsx";

export default function ActivityScreen() {
  const connections = Object.values(useSelector(selectConnections)).filter(c => c.daemon === "aether");
  return <div className="b3-screen"><header className="b3-screen-head"><h1 className="b3-screen-title">Activity</h1>
    <p className="b3-screen-sub">Recent work across your connections.</p></header>
    {connections.length === 0 && <EmptyState title="Connect an Aether daemon to see activity" />}
    {connections.map(c => <ConnectionActivity key={c.id} conn={c.id} label={c.label} />)}
  </div>;
}

/** The first history page reads the newest events, so the screen is not empty until something happens. */
const NEWEST = Number.MAX_SAFE_INTEGER;

function ConnectionActivity({ conn, label }: { conn: string; label: string }) {
  const live = useSelector(selectEvents(conn));
  const [cursors, setCursors] = useState<number[]>([NEWEST]);
  const [family, setFamily] = useState("");
  const [agent, setAgent] = useState("");
  const [open, setOpen] = useState(false);
  const reads = useSelector(state => state.reads[conn]);
  const pages = cursors.flatMap(before => records<aether.Event>(reads?.[`activity:before:${before}`]?.value));
  const all = orderedEvents([...pages, ...live]);
  const visible = all.filter(e => (!family || e.kind.type.split(".")[0] === family) && (!agent || e.agent_id === agent));
  const oldest = all.at(-1)?.seq ?? Number.MAX_SAFE_INTEGER;
  const last = cursors.at(-1);
  const lastRead = last === undefined ? undefined : reads?.[`activity:before:${last}`];
  const exhausted = lastRead?.issued && !lastRead.error && records(lastRead.value).length === 0;
  return <Section title={label}><header className="b3-row"><Button onClick={() => setOpen(true)}>Start an agent</Button>
    <a className="ui-link" href={routeHref("#/memory/:conn", { conn })}>Memory</a><a className="ui-link" href="#/inbox">Inbox</a></header>
    <div className="b3-row"><Field label="Kind family" hint="For example agent or invocation; blank for all" value={family} onChange={e => setFamily(e.target.value)} />
      <Field label="Agent filter" hint="Agent ID; blank for all" value={agent} onChange={e => setAgent(e.target.value)} /></div>
    {visible.length === 0 && <EmptyState title="No activity to show" />}
    {visible.length > 200 ? <VirtualList label="Recent events" count={visible.length} rowHeight={96} height={480}
      rowKey={i => visible[i]?.id ?? i} renderRow={i => { const event = visible[i]; invariant(event, "virtual row exists"); return <EventRow conn={conn} event={event} />; }} /> :
      <div className="b3-stack">{visible.map(event => <EventRow key={event.id} conn={conn} event={event} />)}</div>}
    {cursors.map(before => <HistoryPage key={before} conn={conn} before={before} />)}
    <Button disabled={Boolean(exhausted) || (lastRead !== undefined && !lastRead.error && lastRead.requested > lastRead.issued) || cursors.includes(oldest)}
      onClick={() => setCursors([...cursors, oldest])}>{exhausted ? "Beginning of history" : "Show older"}</Button>
    {open && <StartAgent conn={conn} close={() => setOpen(false)} />}
  </Section>;
}

function HistoryPage({ conn, before }: { conn: string; before: number }) {
  const app = useApp();
  invariant(Number.isSafeInteger(before), "history cursor is an integer");
  invariant(before > 0, "history cursor is positive");
  const command = { cmd: "get_events", after: 0, limit: 100, query: { before } };
  const key = `activity:before:${before}`;
  const entry = useReread(conn, key, command, ["events"]);
  return <ReadNote entry={entry} retry={() => app.reread(conn, key, command, ["events"])} />;
}

function EventRow({ conn, event }: { conn: string; event: aether.Event }) {
  invariant(typeof event.kind.type === "string", "event has a kind");
  invariant(Number.isSafeInteger(event.seq), "event has a sequence");
  return <article className="b3-item"><div className="b3-item-head"><strong>{event.kind.type}</strong><time>{event.at}</time></div>
    {event.agent_id ? <div className="b3-row"><a className="ui-link" href={routeHref("#/agents/:conn/:agent", { conn, agent: event.agent_id })}>{event.agent_id}</a>
      <a className="ui-link" href={routeHref("#/controls/:conn/:agent", { conn, agent: event.agent_id })}>Controls</a></div> : <span className="b3-meta">Connection event</span>}</article>;
}
