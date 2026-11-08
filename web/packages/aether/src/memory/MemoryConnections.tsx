import { routeHref, selectConnections, useSelector } from "@aether-ui/core";
import { EmptyState } from "@aether-ui/ui";

export default function MemoryConnections() {
  const connections = Object.values(useSelector(selectConnections)).filter(info => info.daemon === "aether");
  return <div className="ui-page b3-screen"><header className="ui-page-head"><h1>Memory</h1>
    <p className="ui-page-lede">Choose a connection to review its memories.</p></header>
    {connections.length === 0 ? <EmptyState title="Connect an Aether daemon to see memory" /> :
      <ul>{connections.map(info => <li key={info.id}><a className="ui-link" href={routeHref("#/memory/:conn", { conn: info.id })}>{info.label}</a></li>)}</ul>}
  </div>;
}
