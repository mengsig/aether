import { useState } from "react";
import { readInFlight, relativeTime, routeHref, useNow, selectAgents, useDraft, selectConnections, selectEvents, useApp, useReread, useSelector } from "@aether-ui/core";
import type { aether } from "@aether-ui/protocol";
import { Button, EmptyState, Picker, VirtualList } from "@aether-ui/ui";
import { Section } from "../controls/kit.tsx";
import { invariant } from "../fleet/invariant.ts";
import { records, ReadNote } from "../memory/shared.tsx";
import { familyLabel, kindLabel, orderedEvents } from "./data.ts";
import { StartAgent } from "./StartAgent.tsx";

export default function ActivityScreen() {
  const connections = Object.values(useSelector(selectConnections)).filter(c => c.daemon === "aether");
  return <div className="ui-page b3-screen"><header className="ui-page-head b3-screen-head"><h1 className="ui-visually-hidden b3-screen-title">Activity</h1>
    <p className="ui-page-lede b3-screen-sub">Recent work across your connections.</p></header>
    {connections.length === 0 && <EmptyState title="Connect an Aether daemon to see activity" />}
    {connections.map(c => <ConnectionActivity key={c.id} conn={c.id} label={c.label} />)}
  </div>;
}

/** The first history page reads the newest events, so the screen is not empty until something happens. */
const NEWEST = Number.MAX_SAFE_INTEGER;

function ConnectionActivity({ conn, label }: { conn: string; label: string }) {
  const live = useSelector(selectEvents(conn));
  const [cursors, setCursors] = useState<number[]>([NEWEST]);
  const filter = useDraft("activity/filters", { family: "", agent: "" });
  const { family, agent } = filter.value;
  const agents = Object.values(useSelector(selectAgents(conn)));
  const [open, setOpen] = useState(false);
  const now = useNow();
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
    <div className="b3-row"><Picker label="Kind family" value={family} onChange={family => filter.update({ family })}
      options={[{ id: "", label: "All" }, ...[...new Set(all.map(e => e.kind.type.split(".")[0] ?? e.kind.type))].map(id => ({ id, label: familyLabel(id) }))]} />
      <Picker label="Agent filter" value={agent} onChange={agent => filter.update({ agent })}
        options={[{ id: "", label: "All" }, ...agents.map(a => ({ id: a.id, label: a.name, description: a.id }))]} /></div>
    {visible.length === 0 && <EmptyState title="No activity to show" action={<Button variant="primary" onClick={() => setOpen(true)}>Start an agent</Button>} />}
    {visible.length > 200 ? <VirtualList label="Recent events" count={visible.length} rowHeight={96} height={480}
      rowKey={i => visible[i]?.id ?? i} renderRow={i => { const event = visible[i]; invariant(event, "virtual row exists"); return <EventRow conn={conn} event={event} now={now} />; }} /> :
      <div className="b3-stack">{visible.map(event => <EventRow key={event.id} conn={conn} event={event} now={now} />)}</div>}
    {cursors.map(before => <HistoryPage key={before} conn={conn} before={before} />)}
    <Button disabled={Boolean(exhausted) || (lastRead !== undefined && !lastRead.error && readInFlight(lastRead)) || cursors.includes(oldest)}
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

/** One event in plain words: what happened, how long ago (the exact time on hover), and who. */
function EventRow({ conn, event, now }: { conn: string; event: aether.Event; now: number }) {
  invariant(typeof event.kind.type === "string", "event has a kind");
  invariant(Number.isSafeInteger(event.seq), "event has a sequence");
  const agents = useSelector(selectAgents(conn));
  const date = new Date(event.at);
  const exact = Number.isNaN(date.getTime()) ? event.at : date.toLocaleString();
  return <article className="b3-item" data-kind={event.kind.type}>
    <div className="b3-item-head"><strong title={event.kind.type}>{kindLabel(event.kind.type)}</strong><time dateTime={event.at} title={exact}>{relativeTime(event.at, now)}</time></div>
    {event.agent_id ? <div className="b3-row"><a className="ui-link" href={routeHref("#/agents/:conn/:agent", { conn, agent: event.agent_id })}>{agents[event.agent_id]?.name ?? event.agent_id}</a>
      <a className="ui-link" href={routeHref("#/controls/:conn/:agent", { conn, agent: event.agent_id })}>Controls</a></div> : <span className="b3-meta">From the daemon itself</span>}</article>;
}
